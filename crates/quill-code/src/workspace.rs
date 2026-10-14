use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use futures::StreamExt;
use gpui_kit::{Context, Entity, Task as GpuiTask};
use quill_agent::{AgentEvent, Approval, Decision, Publisher, Task, system_prompt};
use quill_llama::Message;

use crate::attachments::{self, Attachment};
use crate::backend::Backend;
use crate::config::worktrees_dir;
use crate::config::{Config, read_personal_instructions};
use crate::git;
use crate::mentions::mentioned;
use crate::publish::GitPublisher;
use crate::store;
use crate::thread::{Item, Thread, ToolItem, ToolStatus, Turn, TurnState, now_ms, repair_history};

#[derive(Clone)]
pub struct Prompt {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

fn isolated_copy(thread: &Thread) -> Result<PathBuf, String> {
    let project = thread.project.as_ref().ok_or("An isolated copy needs a project.")?;
    let path = worktrees_dir().join(format!("{}-{}", crate::project::name(project), thread.id));
    git::add_worktree(project, &path)?;
    Ok(path)
}

enum Update {
    Event(AgentEvent),
    Finished(Result<(), String>),
}

pub struct Pending {
    pub tool_id: String,
    pub approval: Approval,
    reply: Sender<Decision>,
}

struct Run {
    turn: usize,
    cancel: Arc<AtomicBool>,
    pending: Option<Pending>,
}

pub struct Workspace {
    pub threads: Vec<Thread>,
    pub active: Option<u64>,
    pub draft_project: Option<PathBuf>,
    pub draft_isolated: bool,
    runs: HashMap<u64, Run>,
    backend: Entity<Backend>,
    ticker: Option<GpuiTask<()>>,
}

impl Workspace {
    pub fn new(backend: Entity<Backend>) -> Self {
        Workspace {
            threads: store::load_threads(),
            active: None,
            draft_project: None,
            draft_isolated: false,
            runs: HashMap::new(),
            backend,
            ticker: None,
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
        git::remove_worktree(&project, &worktree)?;
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

    pub fn toggle_tool(&mut self, id: u64, turn: usize, tool: &str, cx: &mut Context<Self>) {
        if let Some(tool) = self.thread_mut(id).and_then(|t| t.turns.get_mut(turn)).and_then(|t| t.tool_mut(tool)) {
            tool.open = !tool.open;
        }
        cx.notify();
    }

    pub fn send(&mut self, prompt: Prompt, cx: &mut Context<Self>) {
        let Prompt { text, attachments } = prompt;
        let id = match self.active {
            Some(id) if self.running(id) => return,
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
        let model = Backend::selected_id(cx);
        let vision = self.backend.read(cx).selected(cx).is_some_and(|m| m.vision);
        let thinking = self.backend.read(cx).request_thinking(cx);
        let url = Backend::url(cx);
        let access = Config::get(cx).access;
        let personal = read_personal_instructions();
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
        let failure = match (&model, isolate.then(|| isolated_copy(thread)).transpose()) {
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
        let model = model.unwrap_or_default();
        thread.model = Some(model.clone());
        thread.turns[index].model = Some(model.clone());
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
        thread.history.push(Message::user_with_images(content, prepared.images));
        let publisher: Option<Arc<dyn Publisher>> =
            root.as_deref().filter(|root| git::snapshot(root).is_some()).map(|_| Arc::new(GitPublisher) as Arc<dyn Publisher>);
        let task = Task {
            server_url: url,
            system: system_prompt(&model, root.as_deref(), publisher.is_some(), &personal),
            model: model.clone(),
            thinking,
            root,
            access,
            history: thread.history.clone(),
            publisher,
        };
        store::save(thread);
        self.backend.update(cx, |backend, cx| backend.mark_used(&model, cx));
        let cancel = Arc::new(AtomicBool::new(false));
        self.runs.insert(id, Run { turn: index, cancel: cancel.clone(), pending: None });
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        cx.background_executor()
            .spawn(async move {
                let result = quill_agent::run(task, &cancel, |event| {
                    let _ = tx.unbounded_send(Update::Event(event));
                });
                let _ = tx.unbounded_send(Update::Finished(result.map_err(|e| e.to_string())));
            })
            .detach();
        cx.spawn(async move |this, cx| {
            while let Some(update) = rx.next().await {
                if this.update(cx, |workspace, cx| workspace.apply(id, index, update, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        self.ensure_ticker(cx);
        cx.notify();
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

    fn apply(&mut self, id: u64, index: usize, update: Update, cx: &mut Context<Self>) {
        if self.runs.get(&id).is_none_or(|run| run.turn != index) {
            return;
        }
        let Some(thread) = self.threads.iter_mut().find(|t| t.id == id) else { return };
        if let Update::Event(AgentEvent::Usage { prompt, completion }) = update {
            thread.context_used = Some(prompt + completion);
            cx.notify();
            return;
        }
        let Some(turn) = thread.turns.get_mut(index) else { return };
        match update {
            Update::Event(AgentEvent::Reasoning(text)) => turn.push_reasoning(&text),
            Update::Event(AgentEvent::Text(text)) => turn.push_text(&text),
            Update::Event(AgentEvent::StepDone) | Update::Event(AgentEvent::Usage { .. }) => {}
            Update::Event(AgentEvent::ToolStarted { id: tool_id, kind, target }) => {
                turn.items.push(Item::Tool(ToolItem {
                    id: tool_id,
                    kind,
                    target,
                    status: ToolStatus::Running,
                    output: String::new(),
                    diff: None,
                    open: false,
                }));
            }
            Update::Event(AgentEvent::ToolFinished { id: tool_id, ok, output, diff }) => {
                if let Some(tool) = turn.tool_mut(&tool_id) {
                    tool.status = if ok { ToolStatus::Done } else { ToolStatus::Failed };
                    tool.output = output;
                    tool.diff = diff;
                }
                if let Some(run) = self.runs.get_mut(&id) {
                    run.pending = None;
                }
            }
            Update::Event(AgentEvent::NeedsApproval { id: tool_id, approval, reply }) => {
                if let Some(tool) = turn.tool_mut(&tool_id) {
                    tool.status = ToolStatus::Waiting;
                }
                if let Some(run) = self.runs.get_mut(&id) {
                    run.pending = Some(Pending { tool_id, approval, reply });
                }
            }
            Update::Event(AgentEvent::Message(message)) => thread.history.push(message),
            Update::Finished(result) => {
                turn.finish(match result {
                    Ok(()) => TurnState::Done,
                    Err(error) => TurnState::Failed(error),
                });
                thread.updated = now_ms();
                repair_history(&mut thread.history);
                store::save(thread);
                self.runs.remove(&id);
                self.backend.update(cx, |backend, cx| backend.refresh(cx));
            }
        }
        cx.notify();
    }

    pub fn stop(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(run) = self.runs.remove(&id) else { return };
        run.cancel.store(true, Ordering::Relaxed);
        if let Some(pending) = run.pending {
            let _ = pending.reply.send(Decision::Decline);
        }
        if let Some(thread) = self.thread_mut(id) {
            if let Some(turn) = thread.turns.get_mut(run.turn) {
                turn.finish(TurnState::Stopped);
            }
            repair_history(&mut thread.history);
            store::save(thread);
        }
        cx.notify();
    }

    pub fn decide(&mut self, id: u64, decision: Decision, cx: &mut Context<Self>) {
        let Some(pending) = self.runs.get_mut(&id).and_then(|run| run.pending.take()) else { return };
        if let Some(tool) = self.thread_mut(id).and_then(|t| t.turns.last_mut()).and_then(|t| t.tool_mut(&pending.tool_id)) {
            tool.status = ToolStatus::Running;
        }
        let _ = pending.reply.send(decision);
        cx.notify();
    }
}