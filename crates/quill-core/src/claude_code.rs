use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use quill_providers::{Message, Tokens, ToolCall, image_block};
use serde_json::{Value, json};

use crate::context;
use crate::diff::{FileDiff, diff_text};
use crate::instructions::project_instructions;
use crate::policy::{Access, Effect};
use crate::protocol::{Approval, Category, ClaudeLink, Event, Outcome, PlanStep, StepStatus, ToolView};
use crate::session::{Input, Session, TurnContext};
use crate::tools::{hide_window, similar};
use crate::turn::{Request, STOPPED, permit};

const HANDOFF: &str = "You are picking up a conversation that started with another model in Quill. Here is what happened so far:";
const IDENTITY: &str = "You are Quill, the coding agent in the Quill desktop app, running on Claude Code. The user follows your steps in Quill's transcript and answers permission prompts there.";

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeCode {
    pub binary: PathBuf,
    pub resume: Option<ClaudeLink>,
}

pub fn find_claude(configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = configured.filter(|path| path.is_file()) {
        return Some(path.to_path_buf());
    }
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd"] } else { &["claude"] };
    let dirs = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect::<Vec<_>>()).unwrap_or_default();
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    dirs.into_iter()
        .chain([home.join(".local").join("bin"), home.join(".claude").join("local")])
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

pub fn claude_version(binary: &Path) -> Option<String> {
    let mut command = Command::new(binary);
    command.arg("--version").stdin(Stdio::null()).stderr(Stdio::null());
    hide_window(&mut command);
    let output = command.output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.split_whitespace().next()?.to_string();
    output.status.success().then_some(version)
}

pub fn complete(binary: &Path, model: &str, prompt: &str) -> Result<String, String> {
    let mut command = Command::new(binary);
    command
        .args(["-p", "--output-format", "text", "--max-turns", "1", "--model", model])
        .current_dir(scratch_dir())
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut command);
    let mut child = command.spawn().map_err(|e| format!("Claude Code could not start: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !text.is_empty() {
        Ok(text)
    } else {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if error.is_empty() { text } else { error })
    }
}

pub(crate) struct Process {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    errors: Arc<Mutex<String>>,
    key: String,
    spent: f64,
}

enum Next {
    Line(String),
    Idle,
    Closed,
}

impl Process {
    fn start(claude: &ClaudeCode, ctx: &TurnContext, root: &Path, resume: Option<&str>, key: String) -> Result<Process, String> {
        let mut command = Command::new(&claude.binary);
        command.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--forward-subagent-text",
            "--permission-prompts",
            "host",
            "--permission-prompt-tool",
            "stdio",
        ]);
        command.arg("--model").arg(&ctx.model).arg("--permission-mode").arg(mode(ctx.access));
        command.arg("--max-turns").arg(ctx.limits.max_steps.to_string());
        if let Some(effort) = ctx.effort {
            command.arg("--effort").arg(effort.wire());
        }
        if let Some(session) = resume {
            command.arg("--resume").arg(session);
        }
        if let Some(prompt) = append_prompt(ctx, root) {
            command.arg("--append-system-prompt-file").arg(prompt);
        }
        command.current_dir(root).env_remove("ANTHROPIC_API_KEY").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        hide_window(&mut command);
        let mut child = command.spawn().map_err(|e| format!("Claude Code could not start: {e}"))?;
        let stdin = child.stdin.take().ok_or("Claude Code has no input pipe.")?;
        let stdout = child.stdout.take().ok_or("Claude Code has no output pipe.")?;
        let stderr = child.stderr.take();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let errors = Arc::new(Mutex::new(String::new()));
        if let Some(mut stderr) = stderr {
            let errors = errors.clone();
            std::thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                while let Ok(read) = stderr.read(&mut buffer) {
                    if read == 0 {
                        break;
                    }
                    let mut text = errors.lock().expect("stderr lock");
                    text.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    if text.len() > 8000 {
                        let mut cut = text.len() - 4000;
                        while !text.is_char_boundary(cut) {
                            cut += 1;
                        }
                        text.drain(..cut);
                    }
                }
            });
        }
        Ok(Process { child, stdin, lines, errors, key, spent: 0.0 })
    }

    fn send(&mut self, value: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{value}").and_then(|_| self.stdin.flush()).map_err(|_| self.failure())
    }

    fn next(&self, wait: Duration) -> Next {
        match self.lines.recv_timeout(wait) {
            Ok(line) => Next::Line(line),
            Err(RecvTimeoutError::Timeout) => Next::Idle,
            Err(RecvTimeoutError::Disconnected) => Next::Closed,
        }
    }

    fn failure(&self) -> String {
        let errors = self.errors.lock().expect("stderr lock");
        let detail = errors.trim();
        if detail.is_empty() { "Claude Code stopped unexpectedly.".into() } else { format!("Claude Code stopped: {detail}") }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn mode(access: Access) -> &'static str {
    match access {
        Access::Supervised => "manual",
        Access::AutoEdits | Access::Full => "acceptEdits",
    }
}

fn append_prompt(ctx: &TurnContext, root: &Path) -> Option<PathBuf> {
    let files: Vec<String> = ctx.instructions.iter().filter(|name| !name.eq_ignore_ascii_case("CLAUDE.md")).cloned().collect();
    let mut parts = vec![IDENTITY.to_string()];
    if !ctx.personal.trim().is_empty() {
        parts.push(format!("# The user's personal instructions\n\n{}", ctx.personal.trim()));
    }
    let project = ctx.root.as_ref().map(|_| project_instructions(root, &files)).unwrap_or_default();
    if !project.is_empty() {
        parts.push(format!("# Project instructions\n\n{project}"));
    }
    let dir = std::env::temp_dir().join("quill-claude");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("prompt-{}-{}.md", std::process::id(), name_hash(&parts.join("\n"))));
    std::fs::write(&path, parts.join("\n\n")).ok()?;
    Some(path)
}

fn name_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

fn spawn_key(ctx: &TurnContext, root: &Path) -> String {
    format!("{}|{:?}|{}|{}|{}", ctx.model, ctx.effort, mode(ctx.access), ctx.limits.max_steps, root.display())
}

fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("quill-chat");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn stored_session(root: &Path, session: &str) -> bool {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
        home.join(".claude")
    });
    let folder: String = root.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    config.join("projects").join(folder).join(format!("{session}.jsonl")).is_file()
}

fn user_message(text: &str, images: &[String]) -> Value {
    let mut content: Vec<Value> = images.iter().map(|url| image_block(url)).collect();
    content.push(json!({ "type": "text", "text": text }));
    json!({ "type": "user", "message": { "role": "user", "content": content } })
}

pub(crate) fn run(
    session: &mut Session,
    turn: usize,
    input: Input,
    ctx: &TurnContext,
    claude: &ClaudeCode,
    cancel: &AtomicBool,
) -> Outcome {
    let root = ctx.root.clone().unwrap_or_else(scratch_dir);
    if session.link.is_none() {
        session.link = claude.resume.clone();
    }
    let synced = session.link.as_ref().is_some_and(|link| link.synced == session.history.len());
    let key = spawn_key(ctx, &root);
    if session.claude.as_ref().is_some_and(|process| process.key != key || !synced) {
        session.claude = None;
    }
    let mut process = match session.claude.take() {
        Some(process) => process,
        None => {
            let resume =
                session.link.as_ref().filter(|link| synced && stored_session(&root, &link.session)).map(|link| link.session.clone());
            let fresh = resume.is_none();
            match Process::start(claude, ctx, &root, resume.as_deref(), key) {
                Ok(process) => {
                    if fresh {
                        session.link = None;
                    }
                    process
                }
                Err(error) => return Outcome::Failed(error),
            }
        }
    };
    let handoff = session.link.is_none() && !session.history.is_empty();
    let mut text = if handoff {
        format!("{HANDOFF}\n\n{}\n\n---\n\n{}", context::transcript(&session.history, 60_000), input.text)
    } else {
        input.text.clone()
    };
    let steers = session.inbox.lock().expect("inbox lock").steers();
    for steer in &steers {
        session.push(Message::user(steer.clone()));
    }
    if !steers.is_empty() {
        text = format!("{}\n\n{text}", steers.join("\n\n"));
    }
    session.push(Message::user_with_images(input.text, input.images.clone()));
    if let Err(error) = process.send(&user_message(&text, &input.images)) {
        return Outcome::Failed(error);
    }
    let mut mapper = Mapper { turn, root, ..Mapper::default() };
    let outcome = converse(session, &mut process, &mut mapper, ctx, cancel);
    mapper.flush(session);
    if let Some(id) = mapper.session.or_else(|| session.link.as_ref().map(|link| link.session.clone())) {
        let link = ClaudeLink { session: id, synced: session.history.len() };
        session.link = Some(link.clone());
        session.emit(Event::ClaudeSession { link });
    }
    if mapper.finished {
        session.claude = Some(process);
    }
    outcome
}

fn converse(session: &mut Session, process: &mut Process, mapper: &mut Mapper, ctx: &TurnContext, cancel: &AtomicBool) -> Outcome {
    let mut deadline: Option<Instant> = None;
    loop {
        if cancel.load(Ordering::Relaxed) && deadline.is_none() {
            let interrupt = json!({ "type": "control_request", "request_id": format!("quill-stop-{}", mapper.turn), "request": { "subtype": "interrupt" } });
            let _ = process.send(&interrupt);
            deadline = Some(Instant::now() + Duration::from_secs(8));
        }
        if deadline.is_some_and(|deadline| Instant::now() > deadline) {
            return Outcome::Stopped;
        }
        let line = match process.next(Duration::from_millis(100)) {
            Next::Line(line) => line,
            Next::Idle => continue,
            Next::Closed => return if deadline.is_some() { Outcome::Stopped } else { Outcome::Failed(process.failure()) },
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
        match value["type"].as_str().unwrap_or_default() {
            "control_request" => {
                let reply = control(session, mapper, ctx, &value["request"], cancel);
                let response = json!({ "type": "control_response", "response": { "subtype": "success", "request_id": value["request_id"], "response": reply } });
                if let Err(error) = process.send(&response) {
                    return Outcome::Failed(error);
                }
            }
            "result" => {
                mapper.flush(session);
                mapper.result(session, process, &value);
                let steers = session.inbox.lock().expect("inbox lock").steers();
                if deadline.is_none() && !steers.is_empty() {
                    for text in &steers {
                        session.push(Message::user(text.clone()));
                    }
                    if let Err(error) = process.send(&user_message(&steers.join("\n\n"), &[])) {
                        return Outcome::Failed(error);
                    }
                    continue;
                }
                mapper.finished = true;
                return outcome(&value, deadline.is_some());
            }
            _ => mapper.apply(session, &value),
        }
    }
}

fn outcome(result: &Value, stopped: bool) -> Outcome {
    if stopped {
        return Outcome::Stopped;
    }
    if result["subtype"] == "success" && !result["is_error"].as_bool().unwrap_or(false) {
        return Outcome::Done;
    }
    let errors: Vec<&str> = result["errors"].as_array().map(|list| list.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let detail = result["result"].as_str().filter(|text| !text.trim().is_empty()).map(str::to_string).unwrap_or_else(|| errors.join(" "));
    match result["subtype"].as_str() {
        Some("error_max_turns") => Outcome::Failed("Claude Code reached its step limit. Send a follow-up to let it continue.".into()),
        _ if detail.is_empty() => Outcome::Failed("Claude Code could not finish this turn.".into()),
        _ => Outcome::Failed(detail),
    }
}

fn control(session: &mut Session, mapper: &mut Mapper, ctx: &TurnContext, request: &Value, cancel: &AtomicBool) -> Value {
    if request["subtype"] != "can_use_tool" {
        return json!({});
    }
    let name = request["tool_name"].as_str().unwrap_or_default();
    let input = &request["input"];
    let id = request["tool_use_id"].as_str().map(str::to_string).unwrap_or_else(|| format!("claude-{}", mapper.tools.len()));
    if name == "AskUserQuestion" {
        return ask(session, mapper.turn, &id, input, cancel);
    }
    let check = classify(name, input, &mapper.root);
    let request = Request { id: &id, tool: check.rule, effect: check.effect, subject: &check.subject, approval: &check.approval };
    match permit(session, mapper.turn, ctx, &request, cancel) {
        Ok(()) => json!({ "behavior": "allow", "updatedInput": input }),
        Err(reason) => json!({ "behavior": "deny", "message": reason }),
    }
}

fn ask(session: &mut Session, turn: usize, id: &str, input: &Value, cancel: &AtomicBool) -> Value {
    let mut answers = serde_json::Map::new();
    for (n, item) in input["questions"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().enumerate() {
        let question = item["question"].as_str().unwrap_or_default().to_string();
        let options: Vec<String> = item["options"]
            .as_array()
            .map(|list| list.iter().filter_map(|option| option["label"].as_str().or(option.as_str()).map(str::to_string)).collect())
            .unwrap_or_default();
        let ask_id = format!("{id}-{n}");
        session.emit(Event::QuestionAsked { turn, id: ask_id.clone(), question: question.clone(), options });
        let answer = session.inbox.lock().expect("inbox lock").answer(&ask_id, cancel);
        match answer {
            Some(answer) => {
                answers.insert(question, json!(answer));
            }
            None => return json!({ "behavior": "deny", "message": STOPPED }),
        }
    }
    let mut updated = input.clone();
    updated["answers"] = Value::Object(answers);
    json!({ "behavior": "allow", "updatedInput": updated })
}

struct Check {
    effect: Effect,
    rule: &'static str,
    subject: String,
    approval: Approval,
}

fn classify(name: &str, input: &Value, root: &Path) -> Check {
    let text = |key: &str| input[key].as_str().unwrap_or_default().to_string();
    let path = |key: &str| relative(root, input[key].as_str().unwrap_or_default());
    let approval = |title: &str, subject: &str| Approval { title: title.into(), subject: subject.into(), diffs: Vec::new(), similar: None };
    let (effect, rule, subject, mut approval) = match name {
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            let key = if name == "NotebookEdit" { "notebook_path" } else { "file_path" };
            let subject = path(key);
            let title = if name == "Write" { "Wants to write a file" } else { "Wants to change a file" };
            let rule = if name == "Write" { "write_file" } else { "edit_file" };
            (Effect::Write, rule, subject.clone(), approval(title, &subject))
        }
        "Bash" | "PowerShell" => {
            let command = text("command");
            let mut asked = approval("Wants to run a command", &command);
            asked.similar = similar(&command);
            (Effect::Execute, "shell", command, asked)
        }
        "WebFetch" => (Effect::Network, "fetch_url", text("url"), approval("Wants to fetch a web page", &text("url"))),
        "WebSearch" => (Effect::Network, "web_search", text("query"), approval("Wants to search the web", &text("query"))),
        "Read" | "Glob" | "Grep" | "LS" => {
            let subject = [input["file_path"].as_str(), input["path"].as_str(), input["pattern"].as_str()]
                .into_iter()
                .flatten()
                .next()
                .unwrap_or_default();
            (Effect::ReadOutside, "read_file", subject.to_string(), approval("Wants to read outside the project", subject))
        }
        "Task" | "Agent" => {
            (Effect::Delegate, "agent", text("description"), approval("Wants to start a helper agent", &text("description")))
        }
        other => {
            let shown = other.trim_start_matches("mcp__").replace("__", " · ");
            (Effect::Execute, "claude_tool", other.to_string(), approval(&format!("Wants to use {shown}"), &shown))
        }
    };
    if effect == Effect::Write {
        approval.diffs = planned_diff(name, input, root).into_iter().collect();
    }
    Check { effect, rule, subject, approval }
}

fn planned_diff(name: &str, input: &Value, root: &Path) -> Option<FileDiff> {
    let file = input["file_path"].as_str()?;
    let before = std::fs::read_to_string(file).ok();
    let after = match name {
        "Write" => input["content"].as_str()?.to_string(),
        "Edit" => apply_edit(before.clone().unwrap_or_default(), input),
        "MultiEdit" => input["edits"].as_array()?.iter().fold(before.clone().unwrap_or_default(), apply_edit),
        _ => return None,
    };
    Some(diff_text(&relative(root, file), before.as_deref(), &after))
}

fn apply_edit(text: String, edit: &Value) -> String {
    let old = edit["old_string"].as_str().unwrap_or_default();
    let new = edit["new_string"].as_str().unwrap_or_default();
    if old.is_empty() {
        return text;
    }
    if edit["replace_all"].as_bool().unwrap_or(false) { text.replace(old, new) } else { text.replacen(old, new, 1) }
}

fn relative(root: &Path, path: &str) -> String {
    let full = Path::new(path);
    full.strip_prefix(root).map(|rel| rel.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| path.replace('\\', "/"))
}

fn todos(input: &Value) -> Vec<PlanStep> {
    input["todos"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|todo| PlanStep {
                    text: todo["content"].as_str().unwrap_or_default().to_string(),
                    status: match todo["status"].as_str() {
                        Some("completed") => StepStatus::Done,
                        Some("in_progress") => StepStatus::Active,
                        _ => StepStatus::Pending,
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

fn view(id: &str, name: &str, input: &Value, root: &Path) -> ToolView {
    let text = |key: &str| input[key].as_str().unwrap_or_default().to_string();
    let path = |key: &str| relative(root, input[key].as_str().unwrap_or_default());
    let (tool, category, active, done, target) = match name {
        "Read" => ("read_file", Category::Explore, "Reading", "Read", path("file_path")),
        "Glob" => ("find_files", Category::Explore, "Looking for", "Looked for", text("pattern")),
        "Grep" => ("grep", Category::Explore, "Searching for", "Searched for", text("pattern")),
        "LS" => ("list_dir", Category::Explore, "Looking through", "Looked through", path("path")),
        "Edit" | "MultiEdit" => ("edit_file", Category::Edit, "Editing", "Edited", path("file_path")),
        "NotebookEdit" => ("edit_file", Category::Edit, "Editing", "Edited", path("notebook_path")),
        "Write" => ("write_file", Category::Edit, "Writing", "Wrote", path("file_path")),
        "Bash" | "PowerShell" => ("shell", Category::Command, "Running", "Ran", text("command")),
        "WebFetch" => ("fetch_url", Category::Web, "Fetching", "Fetched", text("url")),
        "WebSearch" => ("web_search", Category::Web, "Searching the web for", "Searched the web for", text("query")),
        "Task" | "Agent" => ("agent", Category::Agent, "Handing off", "Handed off", text("description")),
        "Skill" => ("skill", Category::Other, "Using the skill", "Used the skill", text("skill")),
        other => {
            let shown = other.trim_start_matches("mcp__").replace("__", " · ");
            return ToolView {
                id: id.into(),
                tool: other.into(),
                category: Category::Other,
                active: "Using".into(),
                done: "Used".into(),
                target: shown,
            };
        }
    };
    ToolView { id: id.into(), tool: tool.into(), category, active: active.into(), done: done.into(), target }
}

fn result_content(content: &Value) -> (String, Option<String>) {
    match content {
        Value::String(text) => (text.clone(), None),
        Value::Array(blocks) => {
            let text = blocks.iter().filter_map(|block| block["text"].as_str()).collect::<Vec<_>>().join("\n");
            let image = blocks.iter().find(|block| block["type"] == "image").and_then(|block| {
                let source = &block["source"];
                Some(format!("data:{};base64,{}", source["media_type"].as_str()?, source["data"].as_str()?))
            });
            (text, image)
        }
        _ => (String::new(), None),
    }
}

fn edit_diffs(block: &Value, detail: &Value, root: &Path) -> Vec<FileDiff> {
    let name = block["name"].as_str().unwrap_or_default();
    if !matches!(name, "Edit" | "MultiEdit" | "Write") {
        return Vec::new();
    }
    let Some(file) = detail["filePath"].as_str().or_else(|| block["input"]["file_path"].as_str()) else { return Vec::new() };
    let before = detail["originalFile"].as_str();
    let after = match name {
        "Write" => block["input"]["content"].as_str().map(str::to_string),
        _ => std::fs::read_to_string(file).ok(),
    };
    after.map(|after| vec![diff_text(&relative(root, file), before, &after)]).unwrap_or_default()
}

#[derive(Default)]
struct Mapper {
    turn: usize,
    root: PathBuf,
    tools: HashMap<String, Value>,
    hidden: HashSet<String>,
    message: Option<String>,
    text: String,
    calls: Vec<ToolCall>,
    session: Option<String>,
    finished: bool,
}

impl Mapper {
    fn apply(&mut self, session: &mut Session, value: &Value) {
        let parent = value["parent_tool_use_id"].as_str();
        match value["type"].as_str().unwrap_or_default() {
            "system" if value["subtype"] == "init" => {
                self.session = value["session_id"].as_str().map(str::to_string);
                if let Some(model) = value["model"].as_str() {
                    session.emit(Event::ModelResolved { turn: self.turn, model: model.to_string() });
                }
            }
            "stream_event" if parent.is_none() => {
                let event = &value["event"];
                if event["type"] == "message_delta" {
                    let tokens = Tokens::from_anthropic(&event["usage"]);
                    if tokens.prompt > 0 {
                        session.emit(Event::Usage { turn: self.turn, tokens });
                    }
                }
                if event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta" {
                    let text = event["delta"]["text"].as_str().unwrap_or_default();
                    if !text.is_empty() {
                        session.emit(Event::Text { turn: self.turn, text: text.to_string() });
                    }
                }
            }
            "assistant" => match parent {
                Some(parent) => self.helper(session, parent, &value["message"]),
                None => self.assistant(session, &value["message"]),
            },
            "user" if parent.is_none() => {
                let blocks = value["message"]["content"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                let results: Vec<&Value> = blocks.iter().filter(|block| block["type"] == "tool_result").collect();
                if results.is_empty() {
                    return;
                }
                self.flush(session);
                let detail = (results.len() == 1).then_some(&value["tool_use_result"]);
                for result in results {
                    self.end(session, result, detail);
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, session: &mut Session, message: &Value) {
        let id = message["id"].as_str().map(str::to_string);
        if id != self.message {
            self.flush(session);
            self.message = id;
        }
        for block in message["content"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            match block["type"].as_str() {
                Some("text") => {
                    if !self.text.is_empty() {
                        self.text.push_str("\n\n");
                    }
                    self.text.push_str(block["text"].as_str().unwrap_or_default());
                }
                Some("tool_use") => self.begin(session, block),
                _ => {}
            }
        }
    }

    fn helper(&self, session: &Session, parent: &str, message: &Value) {
        for block in message["content"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let chunk = match block["type"].as_str() {
                Some("text") => format!("{}\n", block["text"].as_str().unwrap_or_default().trim()),
                Some("tool_use") => {
                    let view = view("", block["name"].as_str().unwrap_or_default(), &block["input"], &self.root);
                    format!("{} {}\n", view.done, view.target)
                }
                _ => continue,
            };
            session.emit(Event::ToolOutput { turn: self.turn, id: parent.to_string(), chunk });
        }
    }

    fn begin(&mut self, session: &mut Session, block: &Value) {
        let id = block["id"].as_str().unwrap_or_default().to_string();
        let name = block["name"].as_str().unwrap_or_default();
        let input = &block["input"];
        self.calls.push(ToolCall::new(id.clone(), name, input.to_string()));
        self.tools.insert(id.clone(), block.clone());
        match name {
            "TodoWrite" => {
                self.hidden.insert(id);
                session.emit(Event::PlanUpdated { turn: self.turn, steps: todos(input), explanation: None });
            }
            "AskUserQuestion" => {
                self.hidden.insert(id);
            }
            _ => session.emit(Event::ToolBegin { turn: self.turn, view: view(&id, name, input, &self.root) }),
        }
    }

    fn end(&mut self, session: &mut Session, result: &Value, detail: Option<&Value>) {
        let id = result["tool_use_id"].as_str().unwrap_or_default().to_string();
        let (text, image) = result_content(&result["content"]);
        let ok = !result["is_error"].as_bool().unwrap_or(false);
        session.push(Message::tool(id.clone(), if ok { text.clone() } else { format!("Error: {text}") }));
        if self.hidden.contains(&id) {
            return;
        }
        let diffs = match (self.tools.get(&id), detail) {
            (Some(block), Some(detail)) if ok => edit_diffs(block, detail, &self.root),
            _ => Vec::new(),
        };
        session.emit(Event::ToolEnd { turn: self.turn, id, ok, output: text, diffs, image });
    }

    fn flush(&mut self, session: &mut Session) {
        if self.text.trim().is_empty() && self.calls.is_empty() {
            self.text.clear();
            return;
        }
        session.push(Message::assistant(std::mem::take(&mut self.text), std::mem::take(&mut self.calls)));
    }

    fn result(&self, session: &Session, process: &mut Process, value: &Value) {
        if let Some(total) = value["total_cost_usd"].as_f64() {
            let usd = (total - process.spent).max(0.0);
            process.spent = total;
            session.emit(Event::Cost { turn: self.turn, usd });
        }
        let window = value["modelUsage"].as_object().and_then(|models| models.values().filter_map(|m| m["contextWindow"].as_u64()).max());
        if let Some(tokens) = window {
            session.emit(Event::ContextWindow { tokens });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_tools_look_like_quill_tools() {
        let root = Path::new("C:\\work\\app");
        let read = view("t1", "Read", &json!({ "file_path": "C:\\work\\app\\src\\main.rs" }), root);
        assert_eq!((read.tool.as_str(), read.category, read.target.as_str()), ("read_file", Category::Explore, "src/main.rs"));
        let run = view("t2", "PowerShell", &json!({ "command": "Get-Content a.txt" }), root);
        assert_eq!((run.tool.as_str(), run.category), ("shell", Category::Command));
        let mcp = view("t3", "mcp__github__create_issue", &json!({}), root);
        assert_eq!(mcp.target, "github · create_issue");
    }

    #[test]
    fn permission_prompts_map_onto_quill_policy() {
        let root = Path::new("C:\\work\\app");
        let command = classify("Bash", &json!({ "command": "npm test -- --watch" }), root);
        assert_eq!((command.effect, command.rule), (Effect::Execute, "shell"));
        assert_eq!(command.approval.similar.as_deref(), Some("npm test"));
        let fetch = classify("WebFetch", &json!({ "url": "https://example.com" }), root);
        assert_eq!(fetch.effect, Effect::Network);
        let write = classify("Write", &json!({ "file_path": "C:\\work\\app\\new.txt", "content": "hi" }), root);
        assert_eq!((write.effect, write.subject.as_str()), (Effect::Write, "new.txt"));
        assert_eq!(write.approval.diffs.len(), 1);
        assert!(write.approval.diffs[0].created);
    }

    #[test]
    fn edits_apply_for_previews() {
        let edit = json!({ "old_string": "a", "new_string": "b" });
        assert_eq!(apply_edit("a a".into(), &edit), "b a");
        let all = json!({ "old_string": "a", "new_string": "b", "replace_all": true });
        assert_eq!(apply_edit("a a".into(), &all), "b b");
    }

    #[test]
    fn todos_become_plan_steps() {
        let steps = todos(&json!({ "todos": [
            { "content": "Read the code", "status": "completed", "activeForm": "Reading" },
            { "content": "Fix it", "status": "in_progress" },
            { "content": "Test", "status": "pending" }
        ]}));
        let states: Vec<StepStatus> = steps.iter().map(|s| s.status).collect();
        assert_eq!(states, vec![StepStatus::Done, StepStatus::Active, StepStatus::Pending]);
    }

    #[test]
    fn results_decide_the_outcome() {
        assert_eq!(outcome(&json!({ "subtype": "success", "is_error": false }), false), Outcome::Done);
        assert_eq!(outcome(&json!({ "subtype": "error_during_execution", "is_error": true }), true), Outcome::Stopped);
        assert_eq!(
            outcome(&json!({ "subtype": "success", "is_error": true, "result": "Not logged in · Please run /login" }), false),
            Outcome::Failed("Not logged in · Please run /login".into())
        );
        assert!(matches!(outcome(&json!({ "subtype": "error_max_turns", "is_error": true }), false), Outcome::Failed(_)));
    }

    #[test]
    fn tool_results_keep_text_and_images() {
        let (text, image) = result_content(&json!([
            { "type": "text", "text": "a screenshot" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" } }
        ]));
        assert_eq!(text, "a screenshot");
        assert_eq!(image.as_deref(), Some("data:image/png;base64,AAAA"));
    }

    #[test]
    fn session_folders_follow_claude_naming() {
        let folder: String = "D:\\repos\\quill-code".chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        assert_eq!(folder, "D--repos-quill-code");
    }
}