use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt;
use gpui_kit::{App, Context, Entity, Task as GpuiTask};
use quill_config::ModelRef;
use quill_core::{Approval, Decision, Engine, Event, Input, Op, Outcome, SessionHandle, Summarizer, Toolsets, TurnContext, spawn};

use crate::attachments::{self, Attachment};
use crate::backend::Backend;
use crate::config::{Config, read_personal_instructions};
use crate::mentions::mentioned;
use crate::store;
use crate::thread::{Item, Thread, ToolItem, ToolStatus, Turn, TurnState, now_ms};
use crate::usage;

const MAX_LIVE_OUTPUT: usize = 200_000;

#[derive(Clone)]
pub struct Prompt {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

fn isolated_copy(thread: &Thread, base: &Path) -> Result<PathBuf, String> {
    let project = thread.project.as_ref().ok_or("An isolated copy needs a project.")?;
    let path = base.join(format!("{}-{}", crate::project::name(project), thread.id));
    quill_git::add_worktree(project, &path)?;
    Ok(path)
}

pub struct Pending {
    pub id: String,
    pub tool: String,
    pub approval: Approval,
}

pub struct Asked {
    pub id: String,
    pub question: String,
    pub options: Vec<String>,
}

struct Run {
    turn: usize,
    cancel: Arc<AtomicBool>,
    pending: Option<Pending>,
    asked: Option<Asked>,
}

pub struct Workspace {
    pub threads: Vec<Thread>,
    pub active: Option<u64>,
    pub draft_project: Option<PathBuf>,
    pub draft_isolated: bool,
    sessions: HashMap<u64, SessionHandle>,
    runs: HashMap<u64, Run>,
    backend: Entity<Backend>,
    ticker: Option<GpuiTask<()>>,
    pub usage: Vec<usage::Record>,
}

impl Workspace {
    pub fn new(backend: Entity<Backend>) -> Self {
        let threads = store::load_threads();
        let usage = usage::load(&threads);
        Workspace {
            threads,
            active: None,
            draft_project: None,
            draft_isolated: false,
            sessions: HashMap::new(),
            runs: HashMap::new(),
            backend,
            ticker: None,
            usage,
        }
    }

    pub fn active_thread(&self) -> Option<&Thread> {
        self.active.and_then(|id| self.threads.iter().find(|t| t.id == id))
    }

    pub fn thread_mut(&mut self, id: u64) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    pub fn project(&self) -> Option<PathBuf> {
        match self.active_thread() {
            Some(thread) => thread.project.clone(),
            None => self.draft_project.clone(),
        }
    }

    pub fn workdir(&self) -> Option<PathBuf> {
        match self.active_thread() {
            Some(thread) => thread.workdir(),
            None => self.draft_project.clone(),
        }
    }

    pub fn isolated(&self) -> bool {
        match self.active_thread() {
            Some(thread) => thread.worktree.is_some(),
            None => self.draft_isolated,
        }
    }

    pub fn set_isolated(&mut self, isolated: bool, cx: &mut Context<Self>) {
        if self.active.is_none() {
            self.draft_isolated = isolated;
            cx.notify();
        }
    }

    pub fn remove_isolated(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
        self.stop(id, cx);
        let thread = self.thread_mut(id).ok_or("That thread no longer exists.")?;
        let (Some(project), Some(worktree)) = (thread.project.clone(), thread.worktree.clone()) else { return Ok(()) };
        quill_git::remove_worktree(&project, &worktree)?;
        thread.worktree = None;
        store::save(thread);
        cx.notify();
        Ok(())
    }

    pub fn running_count(&self) -> usize {
        self.runs.len()
    }

    pub fn running(&self, id: u64) -> bool {
        self.runs.contains_key(&id)
    }

    pub fn pending(&self, id: u64) -> Option<&Pending> {
        self.runs.get(&id).and_then(|run| run.pending.as_ref())
    }

    pub fn asked(&self, id: u64) -> Option<&Asked> {
        self.runs.get(&id).and_then(|run| run.asked.as_ref())
    }

    pub fn waiting(&self, id: u64) -> bool {
        self.runs.get(&id).is_some_and(|run| run.pending.is_some() || run.asked.is_some())
    }

    pub fn start_new(&mut self, project: Option<PathBuf>, cx: &mut Context<Self>) {
        self.active = None;
        self.draft_project = project;
        self.draft_isolated = false;
        cx.notify();
    }

    pub fn activate(&mut self, id: u64, cx: &mut Context<Self>) {
        self.active = Some(id);
        cx.notify();
    }

    pub fn rename(&mut self, id: u64, title: String, cx: &mut Context<Self>) {
        if let Some(thread) = self.thread_mut(id) {
            thread.title = title;
            store::save(thread);
        }
        cx.notify();
    }

    pub fn delete(&mut self, id: u64, cx: &mut Context<Self>) {
        self.stop(id, cx);
        if let Some(session) = self.sessions.remove(&id) {
            session.send(Op::Shutdown);
        }
        self.threads.retain(|t| t.id != id);
        store::delete(id);
        if self.active == Some(id) {
            self.active = None;
        }
        cx.notify();
    }

    pub fn toggle_turn(&mut self, id: u64, turn: usize, cx: &mut Context<Self>) {
        if let Some(turn) = self.thread_mut(id).and_then(|t| t.turns.get_mut(turn)) {
            turn.expanded = !turn.expanded;
        }
        cx.notify();
    }

    fn session(&mut self, id: u64, cx: &mut Context<Self>) -> SessionHandle {
        if let Some(session) = self.sessions.get(&id) {
            return session.clone();
        }
        let history = self.threads.iter().find(|t| t.id == id).map(|t| t.history.clone()).unwrap_or_default();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let session = spawn(history, move |event| {
            let _ = tx.unbounded_send(event);
        });
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this.update(cx, |workspace, cx| workspace.apply(id, event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        self.sessions.insert(id, session.clone());
        session
    }

    fn steer(&mut self, id: u64, text: String, cx: &mut Context<Self>) {
        let Some(turn) = self.runs.get(&id).map(|run| run.turn) else { return };
        if let Some(turn) = self.thread_mut(id).and_then(|thread| thread.turns.get_mut(turn)) {
            turn.items.push(Item::Steer(text.clone()));
        }
        if let Some(session) = self.sessions.get(&id) {
            session.send(Op::Steer(text));
        }
        cx.notify();
    }

    pub fn send(&mut self, prompt: Prompt, cx: &mut Context<Self>) {
        let Prompt { text, attachments } = prompt;
        let id = match self.active {
            Some(id) if self.running(id) => return self.steer(id, text, cx),
            Some(id) => id,
            None => {
                let thread = Thread::new(self.draft_project.take(), &text);
                let id = thread.id;
                self.threads.insert(0, thread);
                self.active = Some(id);
                id
            }
        };
        let isolate = std::mem::take(&mut self.draft_isolated);
        let model = Backend::selected_ref(cx);
        let found = model.as_ref().and_then(|model| self.backend.read(cx).effective(model, cx));
        let vision = found.as_ref().is_some_and(|m| m.vision);
        let label = found.as_ref().map(|m| m.label().to_string());
        let settings = Config::get(cx).clone();
        let Some(thread) = self.thread_mut(id) else { return };
        let index = thread.turns.len();
        let mut names = Vec::new();
        let mut images = Vec::new();
        for (n, attachment) in attachments.iter().enumerate() {
            match attachment.image.then(|| store::keep_image(id, n, &attachment.path)).flatten() {
                Some(path) => images.push(path),
                None => names.push(attachment.name.clone()),
            }
        }
        let mut turn = Turn::new(text.clone(), names);
        turn.images = images;
        thread.turns.push(turn);
        thread.updated = now_ms();
        let failure = match (&model, isolate.then(|| isolated_copy(thread, &settings.worktrees_dir())).transpose()) {
            (None, _) => Some("Choose a model from the model menu in the prompt box first.".to_string()),
            (_, Err(error)) => Some(error),
            (Some(_), Ok(worktree)) => {
                if worktree.is_some() {
                    thread.worktree = worktree;
                }
                None
            }
        };
        if let Some(error) = failure {
            if let Some(turn) = thread.turns.last_mut() {
                turn.finish(TurnState::Failed(error));
            }
            store::save(thread);
            cx.notify();
            return;
        }
        let Some(model) = model else { return };
        thread.model = Some(model.model.clone());
        thread.provider = Some(model.provider.clone());
        thread.turns[index].model = label.or_else(|| Some(model.model.clone()));
        let root = thread.workdir().filter(|p| p.is_dir());
        let mentions: Vec<(String, PathBuf)> = match &root {
            Some(root) => mentioned(&text)
                .into_iter()
                .map(|rel| (rel.clone(), root.join(&rel)))
                .filter(|(_, path)| path.is_file() && path.starts_with(root))
                .collect(),
            None => Vec::new(),
        };
        let prepared = attachments::prepare(&attachments, &mentions, vision);
        let mut content = text;
        content.push_str(&prepared.context);
        if !prepared.skipped.is_empty() {
            content.push_str(&format!("\n\n(Not attached: {}.)", prepared.skipped.join(", ")));
        }
        store::save(thread);
        let context = match self.threads.iter().find(|t| t.id == id).map(|thread| self.context(&model, thread, cx)) {
            Some(Ok(context)) => context,
            Some(Err(error)) => {
                if let Some(thread) = self.thread_mut(id) {
                    if let Some(turn) = thread.turns.last_mut() {
                        turn.finish(TurnState::Failed(error));
                    }
                    store::save(thread);
                }
                cx.notify();
                return;
            }
            None => return,
        };
        self.backend.update(cx, |backend, cx| backend.mark_used(&model, cx));
        let cancel = Arc::new(AtomicBool::new(false));
        self.runs.insert(id, Run { turn: index, cancel: cancel.clone(), pending: None, asked: None });
        let session = self.session(id, cx);
        session.send(Op::Turn { turn: index, input: Input { text: content, images: prepared.images }, context, cancel });
        self.ensure_ticker(cx);
        cx.notify();
    }

    fn context(&self, model: &ModelRef, thread: &Thread, cx: &App) -> Result<TurnContext, String> {
        let backend = self.backend.read(cx);
        let mut engine = backend.engine(model)?;
        if let Engine::ClaudeCode(claude) = &mut engine {
            claude.resume = thread.claude.clone();
        }
        let found = backend.effective(model, cx);
        let root = thread.workdir().filter(|p| p.is_dir());
        let settings = Config::get(cx).for_project(root.as_deref());
        let git = settings.tools.git && root.as_deref().is_some_and(|root| quill_git::snapshot(root).is_some());
        let summarizer =
            settings.models.roles.compaction.as_ref().filter(|role| *role != model).and_then(|role| match backend.engine(role) {
                Ok(Engine::Model(provider)) => Some(Summarizer { provider, model: role.model.clone() }),
                _ => None,
            });
        Ok(TurnContext {
            engine,
            model: model.model.clone(),
            summarizer,
            thinking: backend.request_thinking(cx),
            effort: backend.request_effort(cx),
            max_output: settings.models.override_for(model).and_then(|o| o.max_output),
            server_compaction: found.as_ref().is_some_and(|m| m.server_compaction),
            context_window: found.and_then(|m| m.context).or(thread.context_window),
            root,
            access: settings.agent.access,
            toolsets: Toolsets { git },
            personal: read_personal_instructions(),
            instructions: settings.agent.instruction_files.clone(),
            limits: settings.limits(),
            rules: settings.permissions.rules.clone(),
        })
    }

    pub fn compact(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.running(id) {
            return;
        }
        let Some(model) = Backend::selected_ref(cx) else { return };
        let Some(thread) = self.threads.iter().find(|t| t.id == id).filter(|t| !t.history.is_empty()) else { return };
        let Ok(context) = self.context(&model, thread, cx) else { return };
        let session = self.session(id, cx);
        session.send(Op::Compact { context, cancel: Arc::default() });
    }

    pub fn retry(&mut self, id: u64, turn: usize, cx: &mut Context<Self>) {
        if self.running(id) {
            return;
        }
        let Some(text) = self.threads.iter().find(|t| t.id == id).and_then(|t| t.turns.get(turn)).map(|t| t.prompt.clone()) else { return };
        self.active = Some(id);
        self.send(Prompt { text, attachments: Vec::new() }, cx);
    }

    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = this
                    .update(cx, |workspace, cx| {
                        cx.notify();
                        let keep = !workspace.runs.is_empty();
                        if !keep {
                            workspace.ticker = None;
                        }
                        keep
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    fn apply(&mut self, id: u64, event: Event, cx: &mut Context<Self>) {
        let Some(thread) = self.threads.iter_mut().find(|t| t.id == id) else { return };
        match event {
            Event::Message { message } => {
                thread.history.push(message);
                return;
            }
            Event::ClaudeSession { link } => {
                thread.claude = Some(link);
                return;
            }
            Event::ContextWindow { tokens } => {
                thread.context_window = Some(tokens);
                if let (Some(provider), Some(model)) = (thread.provider.clone(), thread.model.clone()) {
                    self.backend.update(cx, |backend, cx| backend.learn_window(&ModelRef::new(provider, model), tokens, cx));
                }
                cx.notify();
                return;
            }
            Event::Compacted { history } => {
                thread.history = history;
                let index = self.runs.get(&id).map(|run| run.turn).or(thread.turns.len().checked_sub(1));
                if let Some(turn) = index.and_then(|index| thread.turns.get_mut(index)) {
                    turn.items.push(Item::Note("Earlier conversation was summarised to make room.".into()));
                }
                store::save(thread);
                cx.notify();
                return;
            }
            _ => {}
        }
        let Some(run) = self.runs.get_mut(&id) else { return };
        if event_turn(&event) != Some(run.turn) {
            return;
        }
        let Some(turn) = thread.turns.get_mut(run.turn) else { return };
        match event {
            Event::Reasoning { text, .. } => turn.push_reasoning(&text),
            Event::Text { text, .. } => turn.push_text(&text),
            Event::ToolBegin { view, .. } => turn.items.push(Item::Tool(ToolItem::new(view))),
            Event::ToolOutput { id: tool_id, chunk, .. } => {
                if let Some(tool) = turn.tool_mut(&tool_id)
                    && tool.output.len() < MAX_LIVE_OUTPUT
                {
                    tool.output.push_str(&chunk);
                }
            }
            Event::ToolEnd { id: tool_id, ok, output, diffs, image, .. } => {
                if let Some(tool) = turn.tool_mut(&tool_id) {
                    tool.status = if ok { ToolStatus::Done } else { ToolStatus::Failed };
                    tool.output = output;
                    tool.diffs = diffs;
                    tool.image = image;
                    tool.ended = Some(now_ms());
                }
                if run.pending.as_ref().is_some_and(|pending| pending.id == tool_id) {
                    run.pending = None;
                }
            }
            Event::ApprovalRequested { id: tool_id, approval, .. } => {
                let tool = match turn.tool_mut(&tool_id) {
                    Some(tool) => {
                        tool.status = ToolStatus::Waiting;
                        tool.tool.clone()
                    }
                    None => String::new(),
                };
                run.pending = Some(Pending { id: tool_id, tool, approval });
            }
            Event::QuestionAsked { id: tool_id, question, options, .. } => {
                turn.items.push(Item::Question { id: tool_id.clone(), question: question.clone(), options: options.clone(), answer: None });
                run.asked = Some(Asked { id: tool_id, question, options });
            }
            Event::PlanUpdated { steps, explanation, .. } => turn.set_plan(steps, explanation),
            Event::Usage { tokens, .. } => {
                thread.context_used = Some(tokens.total());
                turn.usage.add(tokens);
                let model = thread.provider.clone().zip(thread.model.clone()).map(|(provider, model)| ModelRef::new(provider, model));
                if let Some(price) = model.and_then(|model| self.backend.read(cx).price(&model, cx)) {
                    turn.cost = Some(turn.cost.unwrap_or(0.0) + price.cost(tokens));
                }
            }
            Event::Cost { usd, .. } => turn.cost = Some(turn.cost.unwrap_or(0.0) + usd),
            Event::ModelResolved { model, .. } => {
                turn.model = Some(crate::backend::claude_label(&model));
                if let (Some(provider), Some(alias)) = (thread.provider.clone(), thread.model.clone()) {
                    self.backend.update(cx, |backend, cx| backend.learn_model(&ModelRef::new(provider, alias), &model, cx));
                }
            }
            Event::TurnFinished { outcome, .. } => {
                turn.finish(match outcome {
                    Outcome::Done => TurnState::Done,
                    Outcome::Stopped => TurnState::Stopped,
                    Outcome::Failed(error) => TurnState::Failed(error),
                });
                if let Some(record) = usage::record(thread.id, thread.provider.as_deref(), turn) {
                    usage::append(&record);
                    self.usage.push(record);
                }
                thread.updated = now_ms();
                store::save(thread);
                self.runs.remove(&id);
                self.backend.update(cx, |backend, cx| backend.refresh(cx));
            }
            _ => {}
        }
        cx.notify();
    }

    pub fn stop(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(run) = self.runs.remove(&id) else { return };
        run.cancel.store(true, Ordering::Relaxed);
        let mut stopped = None;
        if let Some(thread) = self.threads.iter_mut().find(|t| t.id == id) {
            if let Some(turn) = thread.turns.get_mut(run.turn) {
                turn.finish(TurnState::Stopped);
                stopped = usage::record(thread.id, thread.provider.as_deref(), turn);
            }
            store::save(thread);
        }
        if let Some(record) = stopped {
            usage::append(&record);
            self.usage.push(record);
        }
        cx.notify();
    }

    pub fn decide(&mut self, id: u64, decision: Decision, cx: &mut Context<Self>) {
        let Some(run) = self.runs.get_mut(&id) else { return };
        let Some(pending) = run.pending.take() else { return };
        let turn = run.turn;
        if let Some(tool) = self.thread_mut(id).and_then(|t| t.turns.get_mut(turn)).and_then(|t| t.tool_mut(&pending.id)) {
            tool.status = ToolStatus::Running;
        }
        if let Some(session) = self.sessions.get(&id) {
            session.send(Op::Decide { id: pending.id, decision });
        }
        cx.notify();
    }

    pub fn answer(&mut self, id: u64, text: String, cx: &mut Context<Self>) {
        let Some(run) = self.runs.get_mut(&id) else { return };
        let Some(asked) = run.asked.take() else { return };
        let turn = run.turn;
        if let Some(turn) = self.thread_mut(id).and_then(|t| t.turns.get_mut(turn)) {
            turn.answer(&asked.id, &text);
        }
        if let Some(session) = self.sessions.get(&id) {
            session.send(Op::Answer { id: asked.id, text });
        }
        cx.notify();
    }
}

fn event_turn(event: &Event) -> Option<usize> {
    match event {
        Event::TurnStarted { turn }
        | Event::Reasoning { turn, .. }
        | Event::Text { turn, .. }
        | Event::ToolBegin { turn, .. }
        | Event::ToolOutput { turn, .. }
        | Event::ToolEnd { turn, .. }
        | Event::ApprovalRequested { turn, .. }
        | Event::QuestionAsked { turn, .. }
        | Event::PlanUpdated { turn, .. }
        | Event::Usage { turn, .. }
        | Event::Cost { turn, .. }
        | Event::ModelResolved { turn, .. }
        | Event::TurnFinished { turn, .. } => Some(*turn),
        Event::Message { .. } | Event::Compacted { .. } | Event::ContextWindow { .. } | Event::ClaudeSession { .. } => None,
    }
}