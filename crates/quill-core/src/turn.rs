use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use quill_providers::{ChatRequest, Error, Event as Stream, Message, Provider, StopReason, ToolCall, ToolSpec};
use serde_json::Value;

use crate::context;
use crate::policy::{self, Access, Effect, Rule, Verdict};
use crate::prompt::{PromptContext, system_prompt};
use crate::protocol::{Approval, Category, Decision, Event, Outcome, PlanStep, Question, StepStatus};
use crate::session::{Inbox, Input, Session, Sink, TurnContext};
use crate::tools::{self, Ctx, Host, Output, Prepared, Tool, Toolset};

pub(crate) const STOPPED: &str = "Stopped by the user before this ran.";
const REFUSED: &str = "The model declined to continue with this request. Try rephrasing it, or pick another model.";

const MAX_COMPACTIONS: usize = 3;

const MALFORMED_RETRIES: usize = 2;

const UNASKED_GIT: &str = "The user has not asked for a commit or a pull request, so leave the changes uncommitted and finish with your reply. The user commits when they choose to.";

const CRAMPED: &str = "The context filled up three times in this turn, so Quill stopped compacting it. The task may need a model with a bigger context window, or smaller steps.";

const REPEATED: &str = "This is the same call, with the same arguments, that already failed earlier in this turn. Do not send it again: change the arguments or take a different approach.";

const UNCHANGED: &str = "Unchanged: this is the same result as the identical call earlier in this turn, and nothing has changed since. Use that earlier result and go on to the next step instead of calling again.";

const STUCK_AFTER: usize = 3;

const STUCK: &str = "You keep repeating searches and reads you already made. Stop exploring now: make the change with what you know, or give the user your answer.";

const NUDGE_AFTER: usize = 6;

const NUDGE: &str = "You are going in circles: your recent searches and reads returned nothing new. Stop exploring. Using what you already found, do the task now, choosing sensible values yourself where the code does not decide them, then finish with your reply.";

const NUDGED: &str = "Quill asked the model to stop repeating searches and get on with the task.";

const PLAN_LEFT: &str = "Your plan still has steps that are not marked completed:";

const PLAN_NEXT: &str = "If you finished them, call update_plan to mark them completed, then stop without repeating your summary. If any are not done, do them now, or say in one sentence why you are leaving them.";

const REMINDED: &str = "Quill reminded the model that its plan had unfinished steps.";

#[derive(Default)]
struct Reply {
    text: String,
    reasoning: String,
    calls: Vec<ToolCall>,
    stop: Option<StopReason>,
}

#[derive(Default)]
struct PendingCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

struct CallHost<'a> {
    sink: &'a Sink,
    inbox: &'a Mutex<Inbox>,
    turn: usize,
    id: String,
    cancel: &'a AtomicBool,
}

impl Host for CallHost<'_> {
    fn output(&self, chunk: &str) {
        (self.sink)(Event::ToolOutput { turn: self.turn, id: self.id.clone(), chunk: chunk.to_string() });
    }

    fn plan(&self, steps: Vec<PlanStep>, explanation: Option<String>) {
        (self.sink)(Event::PlanUpdated { turn: self.turn, steps, explanation });
    }

    fn ask(&self, questions: &[Question]) -> Option<Vec<String>> {
        (self.sink)(Event::QuestionAsked { turn: self.turn, id: self.id.clone(), questions: questions.to_vec() });
        self.inbox.lock().expect("inbox lock").answer(&self.id, self.cancel)
    }
}

pub(crate) fn run(
    session: &mut Session,
    turn: usize,
    input: Input,
    ctx: &TurnContext,
    provider: &dyn Provider,
    cancel: &AtomicBool,
) -> Outcome {
    let tools = if ctx.root.is_some() { tools::enabled(ctx.toolsets, &ctx.limits) } else { Vec::new() };
    let system = system_prompt(&PromptContext {
        model: &ctx.model,
        root: ctx.root.as_deref(),
        tools: &tools,
        shell: ctx.limits.shell,
        personal: &ctx.personal,
        instructions: &ctx.instructions,
        final_newline: ctx.limits.final_newline,
    });
    let specs: Vec<ToolSpec> = tools.iter().map(|tool| tools::spec_for(tool.as_ref())).collect();
    let described: usize = specs.iter().map(|spec| spec.name.len() + spec.description.len() + spec.parameters.to_string().len()).sum();
    let overhead = ((system.len() + described) / 4) as u64;
    session.take_steers();
    session.asked_for_git = tools::asks_for_git(&input.text);
    session.push(Message::user_with_images(input.text, input.images));
    let start = session.history.len();
    let mut reminded = false;
    let mut retried = false;
    let mut malformed = 0;
    let mut floor = 0;
    let mut compactions = 0;
    let mut warned = false;
    let mut recall = Recall::default();
    let growth = ctx.context_window.map_or(0, |window| context::limit(window) / 4);
    for _ in 0..ctx.limits.max_steps {
        if cancel.load(Ordering::Relaxed) {
            return Outcome::Stopped;
        }
        session.take_steers();
        let used = session.used(overhead);
        if ctx.limits.auto_compact && context::over(used, ctx.context_window) && used.saturating_sub(floor) > growth {
            recall.seen.clear();
            if compactions < MAX_COMPACTIONS {
                compactions += usize::from(make_room(session, ctx, cancel, overhead));
            } else if !warned {
                warned = true;
                session.emit(Event::CompactFailed { reason: CRAMPED.into() });
            }
            floor = session.used(overhead);
        }
        let reply = match step(session, turn, ctx, provider, &system, &specs, cancel) {
            Ok(reply) => reply,
            Err(error) if malformed < MALFORMED_RETRIES && malformed_output(&error.0) => {
                malformed += 1;
                continue;
            }
            Err(error) => {
                let again = !retried && ctx.limits.auto_compact && compactions < MAX_COMPACTIONS && context::overflowed(&error.0);
                if !again || !compact(session, ctx, cancel) {
                    return Outcome::Failed(error.0);
                }
                recall.seen.clear();
                compactions += 1;
                retried = true;
                continue;
            }
        };
        retried = false;
        malformed = 0;
        if cancel.load(Ordering::Relaxed) {
            if !reply.text.trim().is_empty() || !reply.reasoning.trim().is_empty() {
                session.push(Message::assistant(reply.text, Vec::new()).with_reasoning(reply.reasoning));
            }
            return Outcome::Stopped;
        }
        let truncated = reply.stop == Some(StopReason::MaxTokens);
        let refused = reply.stop == Some(StopReason::Refusal);
        let calls = if truncated || refused { Vec::new() } else { reply.calls };
        let answer_is_reasoning = calls.is_empty() && reply.text.trim().is_empty();
        let content = if answer_is_reasoning { reply.reasoning.trim().to_string() } else { reply.text };
        let thinking = if answer_is_reasoning { String::new() } else { reply.reasoning };
        session.push(Message::assistant(content, calls.clone()).with_reasoning(thinking));
        if calls.is_empty() {
            if session.take_steers() {
                continue;
            }
            if refused {
                return Outcome::Failed(REFUSED.into());
            }
            if truncated {
                return Outcome::Failed(
                    "The reply hit the model's output limit before it finished. Ask it to continue, or raise the limit on the server."
                        .into(),
                );
            }
            if !reminded && let Some(open) = unfinished(session.history.get(start..).unwrap_or_default()) {
                reminded = true;
                session.emit(Event::Note { turn, text: REMINDED.into() });
                session.push(Message::user(format!("{PLAN_LEFT}\n{open}\n\n{PLAN_NEXT}")));
                continue;
            }
            return Outcome::Done;
        }
        run_calls(session, turn, ctx, &tools, &calls, cancel, &mut recall);
        if recall.repeats >= NUDGE_AFTER && !recall.nudged {
            recall.nudged = true;
            session.emit(Event::Note { turn, text: NUDGED.into() });
            session.push(Message::user(NUDGE));
        }
    }
    Outcome::Failed(format!("Stopped after {} steps. Send a follow-up to let the agent continue.", ctx.limits.max_steps))
}

fn compact(session: &mut Session, ctx: &TurnContext, cancel: &AtomicBool) -> bool {
    match session.compact(ctx, cancel, true) {
        Ok(()) => true,
        Err(reason) => {
            if !cancel.load(Ordering::Relaxed) {
                session.emit(Event::CompactFailed { reason });
            }
            false
        }
    }
}

fn make_room(session: &mut Session, ctx: &TurnContext, cancel: &AtomicBool, overhead: u64) -> bool {
    let full = session.history.clone();
    let measured = session.measured;
    if context::trim_outputs(&mut session.history, 6) {
        session.measured = None;
        if !context::over(session.used(overhead), ctx.context_window) {
            return false;
        }
        session.history = full;
        session.measured = measured;
    }
    compact(session, ctx, cancel)
}

fn malformed_output(error: &str) -> bool {
    error.contains("peg-native") || error.contains("Failed to parse input")
}

fn step(
    session: &mut Session,
    turn: usize,
    ctx: &TurnContext,
    provider: &dyn Provider,
    system: &str,
    specs: &[ToolSpec],
    cancel: &AtomicBool,
) -> Result<Reply, Error> {
    let mut messages = Vec::with_capacity(session.history.len() + 1);
    messages.push(Message::system(system));
    messages.extend(session.history.iter().cloned());
    let request = ChatRequest {
        model: ctx.model.clone(),
        messages,
        reasoning: ctx.tuning.reasoning.clone(),
        kwargs: ctx.tuning.kwargs.clone(),
        tools: specs.to_vec(),
        context: ctx.context_window,
    };
    let sink = session.sink.clone();
    let mut reply = Reply::default();
    let mut pending: Vec<PendingCall> = Vec::new();
    let mut used = None;
    provider.stream(&request, cancel, &mut |event| match event {
        Stream::Reasoning(text) => {
            reply.reasoning.push_str(&text);
            sink(Event::Reasoning { turn, text });
        }
        Stream::Content(text) => {
            reply.text.push_str(&text);
            sink(Event::Text { turn, text });
        }
        Stream::ToolCall { index, id, name, arguments } => {
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
        Stream::Usage(tokens) => {
            used = Some(tokens.total());
            sink(Event::Usage { turn, tokens });
        }
        Stream::Stop(reason) => reply.stop = Some(reason),
    })?;
    if let Some(tokens) = used {
        session.measured = Some((tokens, session.history.len() + 1));
    }
    let base = session.history.len();
    reply.calls = pending
        .into_iter()
        .enumerate()
        .filter(|(_, call)| !call.name.trim().is_empty())
        .map(|(index, call)| {
            ToolCall::new(call.id.filter(|id| !id.is_empty()).unwrap_or_else(|| format!("call_{base}_{index}")), call.name, call.arguments)
        })
        .collect();
    Ok(reply)
}

fn run_calls(
    session: &mut Session,
    turn: usize,
    ctx: &TurnContext,
    tools: &[Box<dyn Tool>],
    calls: &[ToolCall],
    cancel: &AtomicBool,
    recall: &mut Recall,
) {
    let mut index = 0;
    while index < calls.len() {
        if cancel.load(Ordering::Relaxed) {
            for call in &calls[index..] {
                session.push(Message::tool(call.id.clone(), STOPPED));
            }
            return;
        }
        let batch = if ctx.limits.parallel_tools {
            calls[index..]
                .iter()
                .take_while(|call| tools::find(tools, &call.function.name).is_some_and(|tool| tool.spec().parallel))
                .count()
        } else {
            0
        };
        let end = index + batch.max(1);
        let outputs = run_batch(session, turn, ctx, tools, &calls[index..end], cancel);
        for (call, output) in calls[index..end].iter().zip(outputs) {
            let explore = tools::find(tools, &call.function.name).is_some_and(|tool| tool.spec().category == Category::Explore);
            let text = recall.reply(call, &output, explore);
            finish(session, turn, call, output, text);
        }
        index = end;
    }
}

fn run_batch(
    session: &Session,
    turn: usize,
    ctx: &TurnContext,
    tools: &[Box<dyn Tool>],
    calls: &[ToolCall],
    cancel: &AtomicBool,
) -> Vec<Output> {
    let hosts: Vec<CallHost<'_>> =
        calls.iter().map(|call| CallHost { sink: &session.sink, inbox: &session.inbox, turn, id: call.id.clone(), cancel }).collect();
    let admitted: Vec<Result<Prepared, Output>> =
        calls.iter().zip(&hosts).map(|(call, host)| admit(session, turn, ctx, tools, call, host, cancel)).collect();
    let Some(root) = ctx.root.as_deref() else {
        return admitted.into_iter().map(|entry| entry.err().unwrap_or_else(|| Output::fail(STOPPED))).collect();
    };
    let run = |prepared: Prepared, host: &CallHost<'_>| {
        if cancel.load(Ordering::Relaxed) {
            return Output::fail(STOPPED);
        }
        let cx = Ctx { root, limits: &ctx.limits, cancel, host };
        (prepared.action)(&cx)
    };
    if admitted.len() == 1 {
        let entry = admitted.into_iter().next().expect("one call");
        return vec![entry.map_or_else(|output| output, |prepared| run(prepared, &hosts[0]))];
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = admitted
            .into_iter()
            .zip(&hosts)
            .map(|(entry, host)| match entry {
                Ok(prepared) => Ok(scope.spawn(move || run(prepared, host))),
                Err(output) => Err(output),
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| match handle {
                Ok(handle) => handle.join().unwrap_or_else(|_| Output::fail("The tool stopped unexpectedly.")),
                Err(output) => output,
            })
            .collect()
    })
}

fn admit(
    session: &Session,
    turn: usize,
    ctx: &TurnContext,
    tools: &[Box<dyn Tool>],
    call: &ToolCall,
    host: &CallHost<'_>,
    cancel: &AtomicBool,
) -> Result<Prepared, Output> {
    let args = tools::parse_args(&call.function.arguments);
    let Some(tool) = tools::find(tools, &call.function.name) else {
        session.emit(Event::ToolBegin { turn, view: tools::unknown_view(&call.id, &call.function.name) });
        return Err(Output::fail(match ctx.root {
            Some(_) => format!("There is no tool called \"{}\". The tools are {}.", call.function.name, tools::names(tools)),
            None => "No project is open, so tools are unavailable. Answer directly.".into(),
        }));
    };
    session.emit(Event::ToolBegin { turn, view: tools::view(tool, &call.id, &args) });
    if let Some(raw) = tools::invalid(&args) {
        return Err(Output::fail(format!(
            "The arguments were not valid JSON: {raw}. Send a JSON object, for example {{\"path\": \"src/main.rs\"}}."
        )));
    }
    if tool.spec().toolset == Toolset::Git && !session.asked_for_git {
        return Err(Output::fail(UNASKED_GIT));
    }
    let root = ctx.root.as_deref().ok_or_else(|| Output::fail("No project is open, so tools are unavailable."))?;
    let cx = Ctx { root, limits: &ctx.limits, cancel, host };
    let prepared = tool.prepare(&args, &cx).map_err(Output::fail)?;
    let request =
        Request { id: &call.id, tool: tool.spec().name, effect: prepared.effect, subject: &prepared.subject, approval: &prepared.approval };
    permit(session, turn, ctx, &request, cancel).map_err(Output::fail)?;
    Ok(prepared)
}

pub(crate) struct Request<'a> {
    pub id: &'a str,
    pub tool: &'a str,
    pub effect: Effect,
    pub subject: &'a str,
    pub approval: &'a Approval,
}

pub(crate) fn permit(session: &Session, turn: usize, ctx: &TurnContext, request: &Request<'_>, cancel: &AtomicBool) -> Result<(), String> {
    let access = if session.raised.load(Ordering::Relaxed) { Access::Full } else { ctx.access };
    let mut rules: Vec<Rule> = ctx.rules.clone();
    rules.extend(session.rules.lock().expect("rules lock").iter().cloned());
    let name = request.tool;
    match policy::verdict(access, request.effect, name, request.subject, &rules, &ctx.limits.safe_commands) {
        Verdict::Allow => Ok(()),
        Verdict::Deny(reason) => Err(reason),
        Verdict::Ask => {
            session.emit(Event::ApprovalRequested { turn, id: request.id.to_string(), approval: request.approval.clone() });
            let decision = session.inbox.lock().expect("inbox lock").decision(request.id, cancel);
            if cancel.load(Ordering::Relaxed) {
                return Err(STOPPED.into());
            }
            match decision {
                Decision::Decline => Err("The user declined this. Ask what they would like instead.".into()),
                Decision::Approve => Ok(()),
                Decision::AllowSimilar => {
                    let pattern = request.approval.similar.clone().unwrap_or_else(|| request.subject.to_string());
                    session.rules.lock().expect("rules lock").push(Rule::allow(name, &pattern));
                    Ok(())
                }
                Decision::AllowTool => {
                    session.rules.lock().expect("rules lock").push(Rule::allow(name, ""));
                    Ok(())
                }
                Decision::AllowAll => {
                    session.raised.store(true, Ordering::Relaxed);
                    Ok(())
                }
            }
        }
    }
}

fn unfinished(history: &[Message]) -> Option<String> {
    let steps = history
        .iter()
        .rev()
        .flat_map(|message| message.tool_calls.iter().rev())
        .find_map(|call| tools::plan_of(&call.function.name, &call.function.arguments))?;
    let open: Vec<String> = steps.iter().filter(|step| step.status != StepStatus::Done).map(|step| format!("- {}", step.text)).collect();
    (!open.is_empty()).then(|| open.join("\n"))
}

#[derive(Default)]
struct Recall {
    failed: Vec<(String, Value)>,
    seen: Vec<(String, Value, String)>,
    repeats: usize,
    nudged: bool,
}

impl Recall {
    fn reply(&mut self, call: &ToolCall, output: &Output, explore: bool) -> String {
        let (name, args) = (call.function.name.trim().to_ascii_lowercase(), tools::parse_args(&call.function.arguments));
        if !output.ok {
            let key = (name, args);
            if self.failed.contains(&key) {
                return format!("Error: {}\n\n{REPEATED}", output.text);
            }
            self.failed.push(key);
            return format!("Error: {}", output.text);
        }
        if explore {
            if self.seen.iter().any(|(seen, given, text)| *seen == name && *given == args && *text == output.text) {
                self.repeats += 1;
                return if self.repeats >= STUCK_AFTER { format!("{UNCHANGED}\n\n{STUCK}") } else { UNCHANGED.into() };
            }
            self.seen.push((name, args, output.text.clone()));
        }
        output.text.clone()
    }
}

fn finish(session: &mut Session, turn: usize, call: &ToolCall, output: Output, text: String) {
    session.emit(Event::ToolEnd {
        turn,
        id: call.id.clone(),
        ok: output.ok,
        output: output.text,
        diffs: output.diffs,
        image: output.image,
    });
    session.push(Message::tool(call.id.clone(), text));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{Limits, Toolsets};
    use crate::session::{Op, spawn};
    use quill_providers::{Model, Provider};
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    struct Script {
        replies: Mutex<Vec<Vec<Stream>>>,
        seen: Mutex<Vec<ChatRequest>>,
    }

    impl Script {
        fn new(replies: Vec<Vec<Stream>>) -> Arc<Script> {
            Arc::new(Script { replies: Mutex::new(replies.into_iter().rev().collect()), seen: Mutex::new(Vec::new()) })
        }
    }

    impl Provider for Script {
        fn models(&self) -> Result<Vec<Model>, Error> {
            Ok(Vec::new())
        }

        fn stream(&self, request: &ChatRequest, _: &AtomicBool, on_event: &mut dyn FnMut(Stream)) -> Result<(), Error> {
            self.seen.lock().unwrap().push(request.clone());
            let reply = self.replies.lock().unwrap().pop().unwrap_or_else(|| vec![Stream::Content("done".into())]);
            reply.into_iter().for_each(on_event);
            Ok(())
        }
    }

    fn call(id: &str, name: &str, arguments: &str) -> Stream {
        Stream::ToolCall { index: 0, id: Some(id.into()), name: Some(name.into()), arguments: arguments.into() }
    }

    fn context(script: Arc<Script>, root: &std::path::Path, access: Access) -> TurnContext {
        TurnContext {
            provider: script,
            summarizer: None,
            model: "test".into(),
            tuning: quill_providers::Tuning::default(),
            context_window: None,
            root: Some(root.to_path_buf()),
            access,
            toolsets: Toolsets { git: false },
            personal: String::new(),
            instructions: Vec::new(),
            limits: Limits::default(),
            rules: Vec::new(),
        }
    }

    fn start(root: &std::path::Path) -> (crate::session::SessionHandle, Receiver<Event>) {
        let (tx, rx) = channel();
        let _ = std::fs::create_dir_all(root);
        let handle = spawn(Vec::new(), move |event| {
            let _ = tx.send(event);
        });
        (handle, rx)
    }

    fn until_finished(rx: &Receiver<Event>, mut each: impl FnMut(&Event)) -> Outcome {
        loop {
            let event = rx.recv_timeout(Duration::from_secs(20)).expect("an event");
            each(&event);
            if let Event::TurnFinished { outcome, .. } = event {
                return outcome;
            }
        }
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-turn-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_turn_runs_tools_and_finishes() {
        let root = temp("run");
        let script = Script::new(vec![
            vec![call("c1", "write_file", r#"{"path":"a.txt","content":"hi"}"#), Stream::Stop(StopReason::ToolUse)],
            vec![Stream::Content("All done.".into())],
        ]);
        let (handle, rx) = start(&root);
        let input = Input { text: "make a file".into(), images: Vec::new() };
        handle.send(Op::Turn { turn: 0, input, context: context(script.clone(), &root, Access::Full), cancel: Arc::default() });
        let mut ended = Vec::new();
        let outcome = until_finished(&rx, |event| {
            if let Event::ToolEnd { ok, diffs, .. } = event {
                ended.push((*ok, diffs.len()));
            }
        });
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(ended, vec![(true, 1)]);
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "hi\n");
        let second = &script.seen.lock().unwrap()[1];
        assert_eq!(second.messages.last().unwrap().tool_call_id.as_deref(), Some("c1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn approvals_wait_for_a_decision() {
        let root = temp("ask");
        let script =
            Script::new(vec![vec![call("c1", "write_file", r#"{"path":"b.txt","content":"x"}"#)], vec![Stream::Content("ok".into())]]);
        let (handle, rx) = start(&root);
        let input = Input { text: "go".into(), images: Vec::new() };
        handle.send(Op::Turn { turn: 0, input, context: context(script.clone(), &root, Access::Supervised), cancel: Arc::default() });
        let decider = handle.clone();
        let outcome = until_finished(&rx, |event| {
            if let Event::ApprovalRequested { id, .. } = event {
                decider.send(Op::Decide { id: id.clone(), decision: Decision::Decline });
            }
        });
        assert_eq!(outcome, Outcome::Done);
        assert!(!root.join("b.txt").exists());
        let second = &script.seen.lock().unwrap()[1];
        assert!(second.messages.last().unwrap().content.contains("declined"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_call_sent_again_is_called_out() {
        let root = temp("repeat");
        let script = Script::new(vec![
            vec![call("c1", "read_file", r#"{"path":"missing.txt"}"#)],
            vec![call("c2", "read_file", r#"{ "path": "missing.txt" }"#)],
            vec![Stream::Content("ok".into())],
        ]);
        let (handle, rx) = start(&root);
        let input = Input { text: "go".into(), images: Vec::new() };
        handle.send(Op::Turn { turn: 0, input, context: context(script.clone(), &root, Access::Full), cancel: Arc::default() });
        assert_eq!(until_finished(&rx, |_| {}), Outcome::Done);
        let seen = script.seen.lock().unwrap();
        assert!(!seen[1].messages.last().unwrap().content.contains(REPEATED));
        assert!(seen[2].messages.last().unwrap().content.ends_with(REPEATED));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reading_the_same_unchanged_lines_again_says_so() {
        let root = temp("reread");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        let read = r#"{"path":"a.txt","start_line":2}"#;
        let script = Script::new(vec![
            vec![call("c1", "read_file", read)],
            vec![call("c2", "read_file", read)],
            vec![call("c3", "edit_file", r#"{"path":"a.txt","old_text":"two","new_text":"2"}"#)],
            vec![call("c4", "read_file", read)],
            vec![call("c5", "read_file", read)],
            vec![call("c6", "read_file", read)],
            vec![Stream::Content("ok".into())],
        ]);
        let (handle, rx) = start(&root);
        let input = Input { text: "go".into(), images: Vec::new() };
        handle.send(Op::Turn { turn: 0, input, context: context(script.clone(), &root, Access::Full), cancel: Arc::default() });
        assert_eq!(until_finished(&rx, |_| {}), Outcome::Done);
        let seen = script.seen.lock().unwrap();
        let last = |index: usize| seen[index].messages.last().unwrap().content.clone();
        assert!(last(1).contains("two"));
        assert_eq!(last(2), UNCHANGED);
        assert!(last(4).contains("2"));
        assert_eq!(last(5), UNCHANGED);
        assert!(last(6).ends_with(STUCK));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn going_in_circles_gets_a_nudge_from_the_user_side() {
        let root = temp("circles");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        let mut replies: Vec<Vec<Stream>> = (1..=7).map(|n| vec![call(&format!("c{n}"), "read_file", r#"{"path":"a.txt"}"#)]).collect();
        replies.push(vec![Stream::Content("ok".into())]);
        let script = Script::new(replies);
        let (handle, rx) = start(&root);
        let input = Input { text: "go".into(), images: Vec::new() };
        handle.send(Op::Turn { turn: 0, input, context: context(script.clone(), &root, Access::Full), cancel: Arc::default() });
        let mut notes = 0;
        assert_eq!(until_finished(&rx, |event| notes += usize::from(matches!(event, Event::Note { .. }))), Outcome::Done);
        let seen = script.seen.lock().unwrap();
        assert_eq!(notes, 1);
        assert_ne!(seen[6].messages.last().unwrap().content, NUDGE);
        assert_eq!(seen[7].messages.last().unwrap().content, NUDGE);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parser_failures_are_the_ones_retried() {
        assert!(malformed_output("The model produced output that does not match the expected peg-native format"));
        assert!(malformed_output("Failed to parse input at pos 12"));
        assert!(!malformed_output("The server answered 500"));
    }

    #[test]
    fn reasoning_stays_in_the_history_for_the_next_step() {
        let root = temp("reasoning-history");
        let plan = call("c1", "update_plan", r#"{"plan":[{"step":"Write it","status":"in_progress"}]}"#);
        let script = Script::new(vec![
            vec![Stream::Reasoning("Plan the parser first.".into()), plan, Stream::Stop(StopReason::ToolUse)],
            vec![Stream::Content("Done.".into())],
        ]);
        let (handle, rx) = start(&root);
        let input = Input { text: "write the parser".into(), images: Vec::new() };
        handle.send(Op::Turn { turn: 0, input, context: context(script.clone(), &root, Access::Full), cancel: Arc::default() });
        assert_eq!(until_finished(&rx, |_| {}), Outcome::Done);
        let seen = script.seen.lock().unwrap();
        let assistant = seen[1].messages.iter().find(|m| m.role == quill_providers::Role::Assistant).unwrap();
        assert_eq!(assistant.reasoning.as_deref(), Some("Plan the parser first."));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unfinished_plan_gets_one_reminder() {
        let root = temp("plan-left");
        let open =
            call("c1", "update_plan", r#"{"plan":[{"step":"Write it","status":"completed"},{"step":"Test it","status":"in_progress"}]}"#);
        let done =
            call("c2", "update_plan", r#"{"plan":[{"step":"Write it","status":"completed"},{"step":"Test it","status":"completed"}]}"#);
        let script =
            Script::new(vec![vec![open], vec![Stream::Content("All done.".into())], vec![done], vec![Stream::Content(String::new())]]);
        let (handle, rx) = start(&root);
        let input = Input { text: "go".into(), images: Vec::new() };
        handle.send(Op::Turn { turn: 0, input, context: context(script.clone(), &root, Access::Full), cancel: Arc::default() });
        let mut notes = 0;
        assert_eq!(until_finished(&rx, |event| notes += usize::from(matches!(event, Event::Note { .. }))), Outcome::Done);
        let seen = script.seen.lock().unwrap();
        assert_eq!((seen.len(), notes), (4, 1));
        let reminder = &seen[2].messages.last().unwrap().content;
        assert!(reminder.starts_with(PLAN_LEFT) && reminder.contains("- Test it") && !reminder.contains("- Write it"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_full_window_is_compacted_and_the_turn_carries_on() {
        let root = temp("compact-turn");
        let usage = Stream::Usage(quill_providers::Tokens { prompt: 8000, completion: 10, cache_read: 0 });
        let plan = call("c1", "update_plan", r#"{"plan":[{"step":"Write it","status":"in_progress"}]}"#);
        let script = Script::new(vec![
            vec![plan, usage, Stream::Stop(StopReason::ToolUse)],
            vec![Stream::Content("They started writing it.".into())],
            vec![Stream::Content("finished".into())],
        ]);
        let (handle, rx) = start(&root);
        let input = Input { text: "write the parser".into(), images: Vec::new() };
        let context = TurnContext { context_window: Some(9000), ..context(script.clone(), &root, Access::Full) };
        handle.send(Op::Turn { turn: 0, input, context, cancel: Arc::default() });
        let mut compacted = false;
        let outcome = until_finished(&rx, |event| compacted |= matches!(event, Event::Compacted { .. }));
        assert_eq!(outcome, Outcome::Done);
        assert!(compacted);
        let seen = script.seen.lock().unwrap();
        assert_eq!(seen.len(), 4);
        assert!(seen[3].messages.last().unwrap().content.starts_with(PLAN_LEFT));
        assert!(seen[1].messages[0].content.starts_with("You are compacting a coding session"));
        let resumed = &seen[2].messages;
        assert!(resumed[1].content.contains("They started writing it.") && resumed[1].content.contains("write the parser"));
        assert!(resumed[1].content.contains("- [in progress] Write it"));
        assert_eq!(resumed[2].tool_calls[0].function.name, "update_plan");
        assert_eq!(resumed.last().unwrap().role, quill_providers::Role::Tool);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn steering_adds_a_message_before_the_next_step() {
        let root = temp("steer");
        let script = Script::new(vec![vec![call("c1", "ask_user", r#"{"question":"Which one?"}"#)], vec![Stream::Content("ok".into())]]);
        let (handle, rx) = start(&root);
        let input = Input { text: "go".into(), images: Vec::new() };
        handle.send(Op::Turn { turn: 0, input, context: context(script.clone(), &root, Access::Full), cancel: Arc::default() });
        let answerer = handle.clone();
        let outcome = until_finished(&rx, |event| {
            if let Event::QuestionAsked { id, .. } = event {
                answerer.send(Op::Steer("use the blue one".into()));
                answerer.send(Op::Answer { id: id.clone(), answers: vec!["the second".into()] });
            }
        });
        assert_eq!(outcome, Outcome::Done);
        let second = &script.seen.lock().unwrap()[1];
        let texts: Vec<&str> = second.messages.iter().map(|m| m.content.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("The user answered: the second")));
        assert_eq!(texts.last(), Some(&"use the blue one"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stopping_answers_every_open_call() {
        let root = temp("stop");
        let script = Script::new(vec![vec![
            Stream::ToolCall { index: 0, id: Some("a".into()), name: Some("shell".into()), arguments: r#"{"command":"npm run a"}"#.into() },
            Stream::ToolCall { index: 1, id: Some("b".into()), name: Some("shell".into()), arguments: r#"{"command":"npm run b"}"#.into() },
        ]]);
        let (handle, rx) = start(&root);
        let cancel = Arc::new(AtomicBool::new(false));
        let input = Input { text: "go".into(), images: Vec::new() };
        handle.send(Op::Turn { turn: 0, input, context: context(script, &root, Access::Supervised), cancel: cancel.clone() });
        let mut tool_messages = 0;
        let outcome = until_finished(&rx, |event| match event {
            Event::ApprovalRequested { .. } => cancel.store(true, Ordering::Relaxed),
            Event::Message { message } if message.tool_call_id.is_some() => tool_messages += 1,
            _ => {}
        });
        assert_eq!(outcome, Outcome::Stopped);
        assert_eq!(tool_messages, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compacting_with_nothing_earlier_says_why() {
        let root = temp("compact");
        let (handle, rx) = start(&root);
        handle.send(Op::Compact { context: context(Script::new(Vec::new()), &root, Access::Full), cancel: Arc::default() });
        let event = rx.recv_timeout(Duration::from_secs(20)).expect("an event");
        assert_eq!(event, Event::CompactFailed { reason: "There is nothing to compact yet.".into() });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_reply_only_in_reasoning_becomes_the_answer() {
        let root = temp("reasoning");
        let script = Script::new(vec![vec![Stream::Reasoning("The answer is 4.".into())]]);
        let (handle, rx) = start(&root);
        let input = Input { text: "2+2".into(), images: Vec::new() };
        handle.send(Op::Turn { turn: 0, input, context: context(script, &root, Access::Full), cancel: Arc::default() });
        let mut last = None;
        until_finished(&rx, |event| {
            if let Event::Message { message } = event {
                last = Some(message.content.clone());
            }
        });
        assert_eq!(last.as_deref(), Some("The answer is 4."));
        let _ = std::fs::remove_dir_all(&root);
    }
}