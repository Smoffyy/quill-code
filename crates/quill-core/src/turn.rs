use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use quill_providers::{ChatRequest, Error, Event as Stream, Message, Provider, StopReason, ToolCall, ToolSpec};

use crate::context;
use crate::policy::{self, Access, Effect, Rule, Verdict};
use crate::prompt::{PromptContext, system_prompt};
use crate::protocol::{Approval, Decision, Event, Outcome, PlanStep, Question};
use crate::session::{Inbox, Input, Session, Sink, TurnContext};
use crate::tools::{self, Ctx, Host, Output, Prepared, Tool, Toolset};

pub(crate) const STOPPED: &str = "Stopped by the user before this ran.";
const REFUSED: &str = "The model declined to continue with this request. Try rephrasing it, or pick another model.";

const MAX_COMPACTIONS: usize = 3;

const MALFORMED_RETRIES: usize = 2;

const UNASKED_GIT: &str = "The user has not asked for a commit or a pull request, so leave the changes uncommitted and finish with your reply. The user commits when they choose to.";

const CRAMPED: &str = "The context filled up three times in this turn, so Quill stopped compacting it. The task may need a model with a bigger context window, or smaller steps.";

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
    let mut retried = false;
    let mut malformed = 0;
    let mut floor = 0;
    let mut compactions = 0;
    let mut warned = false;
    let growth = ctx.context_window.map_or(0, |window| context::limit(window) / 4);
    for _ in 0..ctx.limits.max_steps {
        if cancel.load(Ordering::Relaxed) {
            return Outcome::Stopped;
        }
        session.take_steers();
        let used = session.used(overhead);
        if ctx.limits.auto_compact && context::over(used, ctx.context_window) && used.saturating_sub(floor) > growth {
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
            return Outcome::Done;
        }
        run_calls(session, turn, ctx, &tools, &calls, cancel);
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

fn run_calls(session: &mut Session, turn: usize, ctx: &TurnContext, tools: &[Box<dyn Tool>], calls: &[ToolCall], cancel: &AtomicBool) {
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
            finish(session, turn, call, output);
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

fn finish(session: &mut Session, turn: usize, call: &ToolCall, output: Output) {
    session.emit(Event::ToolEnd {
        turn,
        id: call.id.clone(),
        ok: output.ok,
        output: output.text.clone(),
        diffs: output.diffs,
        image: output.image,
    });
    let text = if output.ok { output.text } else { format!("Error: {}", output.text) };
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
        assert_eq!(seen.len(), 3);
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
            Stream::ToolCall { index: 0, id: Some("a".into()), name: Some("shell".into()), arguments: r#"{"command":"echo 1"}"#.into() },
            Stream::ToolCall { index: 1, id: Some("b".into()), name: Some("shell".into()), arguments: r#"{"command":"echo 2"}"#.into() },
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