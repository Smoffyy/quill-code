mod diff;
mod instructions;
mod prompt;
mod tools;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::Duration;

use quill_llama::{ChatRequest, Error, Event, Message, Server, ToolCall};
use serde::{Deserialize, Serialize};

pub use diff::{DiffLine, FileDiff, LineKind, diff_text};
pub use instructions::project_instructions;
pub use prompt::system_prompt;
pub use tools::ToolKind;

const MAX_STEPS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    #[serde(alias = "manual")]
    Supervised,
    AutoEdits,
    #[default]
    #[serde(alias = "auto")]
    Full,
}

impl Access {
    pub const ALL: [Access; 3] = [Access::Supervised, Access::AutoEdits, Access::Full];

    pub fn label(self) -> &'static str {
        match self {
            Access::Supervised => "Ask first",
            Access::AutoEdits => "Trust edits",
            Access::Full => "Autopilot",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Access::Supervised => "Checks with you before every edit and command",
            Access::AutoEdits => "Applies edits on its own, checks before running commands",
            Access::Full => "Edits and runs commands without stopping to ask",
        }
    }

    fn asks_for_edits(self) -> bool {
        self == Access::Supervised
    }

    fn asks_for_commands(self) -> bool {
        self != Access::Full
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Approval {
    FileChange(FileDiff),
    Command(String),
    Commit(String),
    PullRequest(String),
}

pub trait Publisher: Send + Sync {
    fn commit(&self, root: &Path, message: &str, branch: Option<&str>) -> Result<String, String>;
    fn open_pull_request(&self, root: &Path, title: &str, body: &str) -> Result<String, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Decline,
    AlwaysAllow,
}

#[derive(Debug)]
pub enum AgentEvent {
    Reasoning(String),
    Text(String),
    StepDone,
    ToolStarted { id: String, kind: ToolKind, target: String },
    ToolFinished { id: String, ok: bool, output: String, diff: Option<FileDiff> },
    NeedsApproval { id: String, approval: Approval, reply: Sender<Decision> },
    Message(Message),
    Usage { prompt: u64, completion: u64 },
}

pub struct Task {
    pub server_url: String,
    pub model: String,
    pub thinking: Option<bool>,
    pub root: Option<PathBuf>,
    pub access: Access,
    pub system: String,
    pub history: Vec<Message>,
    pub publisher: Option<Arc<dyn Publisher>>,
}

#[derive(Default)]
struct PendingCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

pub fn run(task: Task, cancel: &AtomicBool, mut emit: impl FnMut(AgentEvent)) -> Result<(), Error> {
    let server = Server::new(&task.server_url);
    let tool_schemas = if task.root.is_some() { tools::schemas(task.publisher.is_some()) } else { Vec::new() };
    let mut messages = vec![Message::system(task.system.clone())];
    messages.extend(task.history.iter().cloned());
    let mut access = task.access;
    for step in 0..MAX_STEPS {
        let mut text = String::new();
        let mut pending: Vec<PendingCall> = Vec::new();
        let request =
            ChatRequest { model: task.model.clone(), messages: messages.clone(), thinking: task.thinking, tools: tool_schemas.clone() };
        server.stream_chat(&request, cancel, |event| match event {
            Event::Reasoning(chunk) => emit(AgentEvent::Reasoning(chunk)),
            Event::Content(chunk) => {
                text.push_str(&chunk);
                emit(AgentEvent::Text(chunk));
            }
            Event::ToolCall { index, id, name, arguments } => {
                if pending.len() <= index {
                    pending.resize_with(index + 1, PendingCall::default);
                }
                let call = &mut pending[index];
                if id.is_some() {
                    call.id = id;
                }
                if let Some(name) = name {
                    call.name.push_str(&name);
                }
                call.arguments.push_str(&arguments);
            }
            Event::Usage { prompt, completion } => emit(AgentEvent::Usage { prompt, completion }),
        })?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        emit(AgentEvent::StepDone);
        let calls: Vec<ToolCall> = pending
            .into_iter()
            .enumerate()
            .filter(|(_, call)| !call.name.is_empty())
            .map(|(index, call)| ToolCall::new(call.id.unwrap_or_else(|| format!("call_{step}_{index}")), call.name, call.arguments))
            .collect();
        let assistant = Message::assistant(text, calls.clone());
        messages.push(assistant.clone());
        emit(AgentEvent::Message(assistant));
        if calls.is_empty() {
            return Ok(());
        }
        for call in &calls {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let output = match task.root.as_deref() {
                Some(root) => execute(root, task.publisher.as_deref(), call, &mut access, cancel, &mut emit),
                None => "No project is open, so tools are unavailable. Answer directly.".into(),
            };
            let result = Message::tool(call.id.clone(), output);
            messages.push(result.clone());
            emit(AgentEvent::Message(result));
        }
    }
    Err(Error(format!("Stopped after {MAX_STEPS} steps. Send a follow-up to let the agent continue.")))
}

fn ask(id: &str, approval: Approval, cancel: &AtomicBool, emit: &mut impl FnMut(AgentEvent)) -> Decision {
    let (reply, answer) = channel();
    emit(AgentEvent::NeedsApproval { id: id.to_string(), approval, reply });
    loop {
        match answer.recv_timeout(Duration::from_millis(200)) {
            Ok(decision) => return decision,
            Err(RecvTimeoutError::Timeout) if cancel.load(Ordering::Relaxed) => return Decision::Decline,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Decision::Decline,
        }
    }
}

fn execute(
    root: &Path,
    publisher: Option<&dyn Publisher>,
    call: &ToolCall,
    access: &mut Access,
    cancel: &AtomicBool,
    emit: &mut impl FnMut(AgentEvent),
) -> String {
    let git = publisher.is_some();
    let Some(kind) = ToolKind::resolve(&call.function.name).filter(|kind| git || !matches!(kind, ToolKind::Commit | ToolKind::PullRequest))
    else {
        let names: Vec<String> = tools::schemas(git).iter().filter_map(|t| t["function"]["name"].as_str().map(str::to_string)).collect();
        return format!("Error: there is no tool called \"{}\". The tools are {}.", call.function.name, names.join(", "));
    };
    let args = tools::parse_args(&call.function.arguments);
    let id = call.id.clone();
    emit(AgentEvent::ToolStarted { id: id.clone(), kind, target: tools::describe(kind, &args) });
    let (ok, output, diff) = if let Some(raw) = args.get("__invalid").and_then(|v| v.as_str()) {
        (false, format!("The arguments were not valid JSON: {raw}. Send a JSON object, for example {{\"path\": \"src/main.rs\"}}."), None)
    } else {
        match kind {
            ToolKind::List => settle(tools::list(root, &args)),
            ToolKind::Read => settle(tools::read(root, &args)),
            ToolKind::Search => settle(tools::search(root, &args)),
            ToolKind::Write | ToolKind::Edit => change(root, kind, &args, &id, access, cancel, emit),
            ToolKind::Run => command(root, &args, &id, access, cancel, emit),
            ToolKind::Commit | ToolKind::PullRequest => match publisher {
                Some(publisher) => publish(root, publisher, kind, &args, &id, access, cancel, emit),
                None => (false, "Git tools are not available here.".into(), None),
            },
        }
    };
    emit(AgentEvent::ToolFinished { id, ok, output: output.clone(), diff });
    if ok { output } else { format!("Error: {output}") }
}

fn settle(result: Result<String, String>) -> (bool, String, Option<FileDiff>) {
    match result {
        Ok(output) => (true, output, None),
        Err(error) => (false, error, None),
    }
}

fn change(
    root: &Path,
    kind: ToolKind,
    args: &serde_json::Value,
    id: &str,
    access: &mut Access,
    cancel: &AtomicBool,
    emit: &mut impl FnMut(AgentEvent),
) -> (bool, String, Option<FileDiff>) {
    let change = match tools::plan_change(root, kind, args) {
        Ok(change) => change,
        Err(error) => return (false, error, None),
    };
    if access.asks_for_edits() {
        match ask(id, Approval::FileChange(change.diff.clone()), cancel, emit) {
            Decision::Decline => return (false, "The user declined this change. Ask what they would like instead.".into(), None),
            Decision::AlwaysAllow => *access = Access::AutoEdits,
            Decision::Approve => {}
        }
    }
    if let Err(error) = tools::apply(&change) {
        return (false, error, None);
    }
    let verb = if change.diff.created { "Created" } else { "Updated" };
    let summary = format!("{verb} {} (+{} -{})", change.diff.path, change.diff.added, change.diff.removed);
    (true, summary, Some(change.diff))
}

fn command(
    root: &Path,
    args: &serde_json::Value,
    id: &str,
    access: &mut Access,
    cancel: &AtomicBool,
    emit: &mut impl FnMut(AgentEvent),
) -> (bool, String, Option<FileDiff>) {
    let Some(command) = tools::arg(args, &tools::COMMAND_KEYS).map(str::trim).filter(|c| !c.is_empty()) else {
        return (false, "run_command needs a \"command\".".into(), None);
    };
    if access.asks_for_commands() {
        match ask(id, Approval::Command(command.to_string()), cancel, emit) {
            Decision::Decline => return (false, "The user declined running this command. Ask what they would like instead.".into(), None),
            Decision::AlwaysAllow => *access = Access::Full,
            Decision::Approve => {}
        }
    }
    let (ok, output) = tools::run(root, command, tools::command_timeout(args), cancel);
    (ok, output, None)
}

#[allow(clippy::too_many_arguments)]
fn publish(
    root: &Path,
    publisher: &dyn Publisher,
    kind: ToolKind,
    args: &serde_json::Value,
    id: &str,
    access: &mut Access,
    cancel: &AtomicBool,
    emit: &mut impl FnMut(AgentEvent),
) -> (bool, String, Option<FileDiff>) {
    let text = |keys: &[&str]| tools::arg(args, keys).map(str::trim).filter(|t| !t.is_empty());
    let branch = text(&tools::BRANCH_KEYS);
    let (approval, asks, widened) = if kind == ToolKind::Commit {
        let Some(message) = text(&tools::MESSAGE_KEYS) else {
            return (false, "commit_changes needs a \"message\".".into(), None);
        };
        let summary = branch.map_or_else(|| message.to_string(), |branch| format!("{message} (on {branch})"));
        (Approval::Commit(summary), access.asks_for_edits(), Access::AutoEdits)
    } else {
        let Some(title) = text(&tools::TITLE_KEYS) else {
            return (false, "open_pull_request needs a \"title\".".into(), None);
        };
        (Approval::PullRequest(title.to_string()), access.asks_for_commands(), Access::Full)
    };
    if asks {
        match ask(id, approval, cancel, emit) {
            Decision::Decline => return (false, "The user declined this. Ask what they would like instead.".into(), None),
            Decision::AlwaysAllow => *access = widened,
            Decision::Approve => {}
        }
    }
    let result = match kind {
        ToolKind::Commit => publisher.commit(root, text(&tools::MESSAGE_KEYS).unwrap_or_default(), branch),
        _ => publisher.open_pull_request(root, text(&tools::TITLE_KEYS).unwrap_or_default(), text(&tools::BODY_KEYS).unwrap_or_default()),
    };
    settle(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_reads_old_settings() {
        let auto: Access = serde_json::from_str("\"auto\"").unwrap();
        let manual: Access = serde_json::from_str("\"manual\"").unwrap();
        assert_eq!((auto, manual), (Access::Full, Access::Supervised));
        assert_eq!(serde_json::to_string(&Access::AutoEdits).unwrap(), "\"auto_edits\"");
    }
}