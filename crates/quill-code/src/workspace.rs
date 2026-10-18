use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use quill_config::{ModelRef, Settings};
use quill_core::{
    Access, Approval, Decision, Event, Input, Op, Outcome, Question, SessionHandle, Summarizer, Toolsets, TurnContext, spawn,
};

use crate::Msg;
use crate::attachments::{self, Attachment};
use crate::backend::Backend;
use crate::config::read_personal_instructions;
use crate::mentions::mentioned;
use crate::project;
use crate::store;
use crate::thread::{Item, Thread, ToolItem, ToolStatus, Turn, TurnState, now_ms};
use crate::usage;

const MAX_LIVE_OUTPUT: usize = 200_000;

#[derive(Clone, Default)]
pub struct Prompt {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

pub struct Pending {
    pub id: String,
    pub tool: String,
    pub approval: Approval,
}

pub struct Asked {
    pub id: String,
    pub questions: Vec<Question>,
    pub current: usize,
    pub highlight: Vec<usize>,
    pub picked: Vec<Option<usize>>,
    pub typed: Vec<String>,
    pub answers: Vec<Option<String>>,
}

fn open_ended(option: &str) -> bool {
    let option = option.trim().trim_end_matches(['.', ':', '…']).to_lowercase();
    ["something else", "other", "other answer", "none of these", "type my own", "custom"].contains(&option.as_str())
}

impl Asked {
    pub fn new(id: String, questions: Vec<Question>) -> Asked {
        let questions: Vec<Question> = questions
            .into_iter()
            .map(|question| Question { options: question.options.into_iter().filter(|option| !open_ended(option)).collect(), ..question })
            .collect();
        let count = questions.len();
        Asked {
            id,
            questions,
            current: 0,
            highlight: vec![0; count],
            picked: vec![None; count],
            typed: vec![String::new(); count],
            answers: vec![None; count],
        }
    }

    pub fn question(&self) -> &Question {
        &self.questions[self.current]
    }

    pub fn other_row(&self) -> usize {
        self.question().options.len()
    }

    pub fn finished(&self) -> bool {
        self.answers.iter().all(Option::is_some)
    }
}

struct Run {
    thread: u64,
    turn: usize,
    cancel: Arc<AtomicBool>,
    pending: Option<Pending>,
    asked: Option<Asked>,
    compacting: bool,
}

pub struct Workspace {
    pub threads: Vec<Thread>,
    pub active: Option<u64>,
    pub usage: Vec<usage::Record>,
    pub access: Option<Access>,
    session: Option<(u64, SessionHandle)>,
    run: Option<Run>,
    tx: Sender<Msg>,
}

fn isolated_copy(thread: &Thread, base: &Path) -> Result<PathBuf, String> {
    let project = thread.project.as_ref().ok_or("An isolated copy needs a project.")?;
    let path = base.join(format!("{}-{}", project::name(project), thread.id));
    quill_git::add_worktree(project, &path)?;
    Ok(path)
}

impl Workspace {
    pub fn new(tx: Sender<Msg>) -> Workspace {
        let threads = store::load_threads();
        let usage = usage::load(&threads);
        Workspace { threads, active: None, usage, access: None, session: None, run: None, tx }
    }

    pub fn active_thread(&self) -> Option<&Thread> {
        self.active.and_then(|id| self.threads.iter().find(|t| t.id == id))
    }

    pub fn thread_mut(&mut self, id: u64) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    pub fn running(&self) -> bool {
        self.run.is_some()
    }

    pub fn running_turn(&self) -> Option<(u64, usize)> {
        self.run.as_ref().map(|run| (run.thread, run.turn))
    }

    pub fn pending(&self) -> Option<&Pending> {
        self.run.as_ref().and_then(|run| run.pending.as_ref())
    }

    pub fn asked(&self) -> Option<&Asked> {
        self.run.as_ref().and_then(|run| run.asked.as_ref())
    }

    pub fn compacting(&self) -> bool {
        self.run.as_ref().is_some_and(|run| run.compacting)
    }

    pub fn asked_mut(&mut self) -> Option<&mut Asked> {
        self.run.as_mut().and_then(|run| run.asked.as_mut())
    }

    pub fn touch(&mut self, id: u64) {
        if let Some(thread) = self.thread_mut(id) {
            thread.updated = now_ms();
            store::save(thread);
        }
    }

    pub fn threads_in(&self, root: &Path) -> Vec<&Thread> {
        self.threads.iter().filter(|thread| thread.project.as_deref().is_some_and(|project| project::same(project, root))).collect()
    }

    pub fn open(&mut self, id: u64) {
        if self.session.as_ref().is_some_and(|(session, _)| *session != id)
            && let Some((_, handle)) = self.session.take()
        {
            handle.send(Op::Shutdown);
        }
        self.active = Some(id);
    }

    pub fn start_new(&mut self) {
        if let Some((_, handle)) = self.session.take() {
            handle.send(Op::Shutdown);
        }
        self.active = None;
    }

    pub fn rename(&mut self, id: u64, title: String) {
        if let Some(thread) = self.thread_mut(id) {
            thread.title = title;
            store::save(thread);
        }
    }

    pub fn delete(&mut self, id: u64) {
        if self.run.as_ref().is_some_and(|run| run.thread == id) {
            self.stop();
        }
        if self.session.as_ref().is_some_and(|(session, _)| *session == id)
            && let Some((_, handle)) = self.session.take()
        {
            handle.send(Op::Shutdown);
        }
        self.threads.retain(|t| t.id != id);
        store::delete(id);
        if self.active == Some(id) {
            self.active = None;
        }
    }

    fn session(&mut self, id: u64) -> SessionHandle {
        if let Some((session, handle)) = &self.session
            && *session == id
        {
            return handle.clone();
        }
        if let Some((_, handle)) = self.session.take() {
            handle.send(Op::Shutdown);
        }
        let history = self.threads.iter().find(|t| t.id == id).map(|t| t.history.clone()).unwrap_or_default();
        let tx = self.tx.clone();
        let handle = spawn(history, move |event| {
            let _ = tx.send(Msg::Session(id, event));
        });
        self.session = Some((id, handle.clone()));
        handle
    }

    fn steer(&mut self, text: String) {
        let Some((id, turn)) = self.running_turn() else { return };
        if let Some(turn) = self.thread_mut(id).and_then(|thread| thread.turns.get_mut(turn)) {
            turn.items.push(Item::Steer(text.clone()));
        }
        if let Some((session, handle)) = &self.session
            && *session == id
        {
            handle.send(Op::Steer(text));
        }
    }

    pub fn send(&mut self, prompt: Prompt, root: &Path, isolate: bool, backend: &mut Backend, settings: &Settings) -> Option<usize> {
        let Prompt { text, mut attachments } = prompt;
        if self.running() {
            self.steer(text);
            return None;
        }
        let id = match self.active {
            Some(id) => id,
            None => {
                let thread = Thread::new(Some(root.to_path_buf()), &text);
                let id = thread.id;
                self.threads.insert(0, thread);
                self.active = Some(id);
                id
            }
        };
        let model = settings.models.selected.clone();
        let found = model.as_ref().and_then(|model| backend.effective(model, settings));
        let vision = found.as_ref().is_some_and(|m| m.vision);
        let label = found.as_ref().map(|m| m.label().to_string());
        let thread = self.thread_mut(id)?;
        let mut mentions: Vec<(String, PathBuf)> = Vec::new();
        if let Some(root) = thread.workdir().filter(|p| p.is_dir()) {
            for rel in mentioned(&text) {
                let path = root.join(&rel);
                if !path.is_file() || !path.starts_with(&root) {
                    continue;
                }
                match Attachment::from_path(path.clone()).filter(|attachment| attachment.image) {
                    Some(image) => attachments.push(image),
                    None => mentions.push((rel, path)),
                }
            }
        }
        let index = thread.turns.len();
        let names: Vec<String> = attachments.iter().map(|attachment| attachment.name.clone()).collect();
        let images: Vec<PathBuf> = attachments
            .iter()
            .enumerate()
            .filter(|(_, attachment)| attachment.image)
            .filter_map(|(n, attachment)| store::keep_image(id, n, &attachment.path))
            .collect();
        let mut turn = Turn::new(text.clone(), names);
        turn.images = images;
        thread.turns.push(turn);
        thread.updated = now_ms();
        let isolated = (isolate && index == 0 && thread.worktree.is_none()).then(|| isolated_copy(thread, &settings.worktrees_dir()));
        let failure = match (&model, isolated.transpose()) {
            (None, _) => Some("Choose a model first with /model.".to_string()),
            (_, Err(error)) => Some(error),
            (Some(_), Ok(worktree)) => {
                if worktree.is_some() {
                    thread.worktree = worktree;
                }
                None
            }
        };
        if let Some(error) = failure {
            return self.fail(id, index, error);
        }
        let model = model?;
        let thread = self.thread_mut(id)?;
        thread.model = Some(model.model.clone());
        thread.provider = Some(model.provider.clone());
        thread.turns[index].model = label.or_else(|| Some(model.model.clone()));
        let prepared = attachments::prepare(&attachments, &mentions, vision);
        let mut content = text;
        content.push_str(&prepared.context);
        if !prepared.skipped.is_empty() {
            content.push_str(&format!("\n\n(Not attached: {}.)", prepared.skipped.join(", ")));
        }
        store::save(thread);
        let context = match self.threads.iter().find(|t| t.id == id).map(|thread| self.context(&model, thread, backend, settings)) {
            Some(Ok(context)) => context,
            Some(Err(error)) => return self.fail(id, index, error),
            None => return None,
        };
        backend.mark_used(&model);
        let cancel = Arc::new(AtomicBool::new(false));
        self.run = Some(Run { thread: id, turn: index, cancel: cancel.clone(), pending: None, asked: None, compacting: false });
        let session = self.session(id);
        session.send(Op::Turn { turn: index, input: Input { text: content, images: prepared.images }, context, cancel });
        Some(index)
    }

    fn fail(&mut self, id: u64, index: usize, error: String) -> Option<usize> {
        let thread = self.thread_mut(id)?;
        if let Some(turn) = thread.turns.get_mut(index) {
            turn.finish(TurnState::Failed(error));
        }
        store::save(thread);
        Some(index)
    }

    fn context(&self, model: &ModelRef, thread: &Thread, backend: &Backend, settings: &Settings) -> Result<TurnContext, String> {
        let provider = backend.provider(model)?;
        let root = thread.workdir().filter(|p| p.is_dir());
        let project = settings.for_project(root.as_deref());
        let git = project.tools.git && root.as_deref().is_some_and(|root| quill_git::snapshot(root).is_some());
        let summarizer = project.models.roles.compaction.as_ref().filter(|role| *role != model).and_then(|role| {
            let provider = backend.provider(role).ok()?;
            Some(Summarizer { provider, model: role.model.clone(), tuning: backend.tuning(role, &project) })
        });
        Ok(TurnContext {
            provider,
            model: model.model.clone(),
            summarizer,
            tuning: backend.tuning(model, &project),
            context_window: backend.effective(model, settings).and_then(|m| m.context),
            root,
            access: self.access.unwrap_or(project.agent.access),
            toolsets: Toolsets { git },
            personal: read_personal_instructions(),
            instructions: project.agent.instruction_files.clone(),
            limits: project.limits(),
            rules: project.permissions.rules.clone(),
        })
    }

    pub fn compact(&mut self, backend: &Backend, settings: &Settings) -> Result<(), String> {
        if self.running() {
            return Err("Wait for the reply to finish. A thread can be compacted between replies.".into());
        }
        let model = settings.models.selected.clone().ok_or("Choose a model first with /model.")?;
        let id = self.active.ok_or("Nothing to compact yet.")?;
        let thread = self.threads.iter().find(|t| t.id == id).filter(|t| !t.history.is_empty()).ok_or("Nothing to compact yet.")?;
        let context = self.context(&model, thread, backend, settings)?;
        self.session(id).send(Op::Compact { context, cancel: Arc::default() });
        Ok(())
    }

    pub fn last_prompt(&self) -> Option<String> {
        self.active_thread().and_then(|thread| thread.turns.last()).map(|turn| turn.prompt.clone())
    }

    pub fn apply(&mut self, id: u64, event: Event) -> bool {
        let Some(thread) = self.threads.iter_mut().find(|t| t.id == id) else { return false };
        match event {
            Event::Message { message } => {
                thread.history.push(message);
                return false;
            }
            Event::Compacting => {
                if let Some(run) = self.run.as_mut() {
                    run.compacting = true;
                }
                return false;
            }
            Event::CompactFailed { .. } => {
                if let Some(run) = self.run.as_mut() {
                    run.compacting = false;
                }
                return false;
            }
            Event::Compacted { history } => {
                thread.history = history;
                thread.context_used = None;
                let index = self.run.as_ref().filter(|run| run.thread == id).map(|run| run.turn).or(thread.turns.len().checked_sub(1));
                if let Some(turn) = index.and_then(|index| thread.turns.get_mut(index)) {
                    turn.items.push(Item::Note("The conversation was summarised to free up the model's context.".into()));
                }
                if let Some(run) = self.run.as_mut() {
                    run.compacting = false;
                }
                store::save(thread);
                return false;
            }
            _ => {}
        }
        let Some(run) = self.run.as_mut().filter(|run| run.thread == id) else { return false };
        if event_turn(&event) != Some(run.turn) {
            return false;
        }
        let Some(turn) = thread.turns.get_mut(run.turn) else { return false };
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
            Event::QuestionAsked { id: tool_id, questions, .. } => {
                for asked in &questions {
                    let (question, options) = (asked.question.clone(), asked.options.clone());
                    turn.items.push(Item::Question { id: tool_id.clone(), question, options, answer: None });
                }
                run.asked = Some(Asked::new(tool_id, questions));
            }
            Event::PlanUpdated { steps, explanation, .. } => turn.set_plan(steps, explanation),
            Event::Usage { tokens, .. } => {
                thread.context_used = Some(tokens.total());
                turn.usage.add(tokens);
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
                self.run = None;
                return true;
            }
            _ => {}
        }
        false
    }

    pub fn stop(&mut self) {
        let Some(run) = self.run.take() else { return };
        run.cancel.store(true, Ordering::Relaxed);
        let Some(thread) = self.threads.iter_mut().find(|t| t.id == run.thread) else { return };
        if let Some(turn) = thread.turns.get_mut(run.turn) {
            turn.finish(TurnState::Stopped);
            if let Some(record) = usage::record(thread.id, thread.provider.as_deref(), turn) {
                usage::append(&record);
                self.usage.push(record);
            }
        }
        store::save(thread);
    }

    pub fn decide(&mut self, decision: Decision) {
        let Some(run) = self.run.as_mut() else { return };
        let Some(pending) = run.pending.take() else { return };
        let (id, turn) = (run.thread, run.turn);
        if let Some(tool) = self.thread_mut(id).and_then(|t| t.turns.get_mut(turn)).and_then(|t| t.tool_mut(&pending.id)) {
            tool.status = ToolStatus::Running;
            tool.started = now_ms();
        }
        if let Some((_, handle)) = &self.session {
            handle.send(Op::Decide { id: pending.id, decision });
        }
    }

    pub fn redirect(&mut self, instead: String) {
        self.decide(Decision::Decline);
        self.steer(instead);
    }

    pub fn answer(&mut self, answers: Vec<String>) {
        let Some(run) = self.run.as_mut() else { return };
        let Some(asked) = run.asked.take() else { return };
        let (id, turn) = (run.thread, run.turn);
        if let Some(turn) = self.thread_mut(id).and_then(|t| t.turns.get_mut(turn)) {
            turn.answer(&asked.id, &answers);
        }
        if let Some((_, handle)) = &self.session {
            handle.send(Op::Answer { id: asked.id, answers });
        }
    }

    pub fn shutdown(&mut self) {
        self.stop();
        if let Some((_, handle)) = self.session.take() {
            handle.send(Op::Shutdown);
        }
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
        | Event::TurnFinished { turn, .. } => Some(*turn),
        Event::Message { .. } | Event::Compacting | Event::Compacted { .. } | Event::CompactFailed { .. } => None,
    }
}