use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quill_providers::{Message, Provider, Tuning};

use crate::context;
use crate::options::{Limits, Toolsets};
use crate::policy::{Access, Rule};
use crate::protocol::{Decision, Event, Outcome};
use crate::tools;
use crate::turn;

#[derive(Clone)]
pub struct Summarizer {
    pub provider: Arc<dyn Provider>,
    pub model: String,
    pub tuning: Tuning,
}

#[derive(Clone)]
pub struct TurnContext {
    pub provider: Arc<dyn Provider>,
    pub summarizer: Option<Summarizer>,
    pub model: String,
    pub tuning: Tuning,
    pub context_window: Option<u64>,
    pub root: Option<PathBuf>,
    pub access: Access,
    pub toolsets: Toolsets,
    pub personal: String,
    pub instructions: Vec<String>,
    pub limits: Limits,
    pub rules: Vec<Rule>,
}

pub struct Input {
    pub text: String,
    pub images: Vec<String>,
}

pub enum Op {
    Turn { turn: usize, input: Input, context: TurnContext, cancel: Arc<AtomicBool> },
    Steer(String),
    Decide { id: String, decision: Decision },
    Answer { id: String, answers: Vec<String> },
    Compact { context: TurnContext, cancel: Arc<AtomicBool> },
    Shutdown,
}

#[derive(Clone)]
pub struct SessionHandle {
    ops: Sender<Op>,
}

impl SessionHandle {
    pub fn send(&self, op: Op) -> bool {
        self.ops.send(op).is_ok()
    }
}

pub(crate) type Sink = Arc<dyn Fn(Event) + Send + Sync>;

pub fn spawn(history: Vec<Message>, sink: impl Fn(Event) + Send + Sync + 'static) -> SessionHandle {
    let (ops, rx) = channel();
    let session = Session {
        history,
        inbox: Mutex::new(Inbox {
            rx,
            queue: VecDeque::new(),
            steers: Vec::new(),
            decisions: HashMap::new(),
            answers: HashMap::new(),
            closed: false,
        }),
        sink: Arc::new(sink),
        rules: Mutex::new(Vec::new()),
        raised: AtomicBool::new(false),
        measured: None,
        asked_for_git: false,
    };
    std::thread::Builder::new().name("quill-session".into()).spawn(move || session.run()).expect("start a session thread");
    SessionHandle { ops }
}

pub(crate) struct Inbox {
    rx: Receiver<Op>,
    queue: VecDeque<Op>,
    steers: Vec<String>,
    decisions: HashMap<String, Decision>,
    answers: HashMap<String, Vec<String>>,
    closed: bool,
}

impl Inbox {
    fn take(&mut self, op: Op) -> Option<Op> {
        match op {
            Op::Steer(text) => self.steers.push(text),
            Op::Decide { id, decision } => {
                self.decisions.insert(id, decision);
            }
            Op::Answer { id, answers } => {
                self.answers.insert(id, answers);
            }
            Op::Shutdown => self.closed = true,
            other => return Some(other),
        }
        None
    }

    fn take_running(&mut self, op: Op) {
        if let Some(op) = self.take(op) {
            self.queue.push_back(op);
        }
    }

    pub fn pump(&mut self) {
        while let Ok(op) = self.rx.try_recv() {
            self.take_running(op);
        }
    }

    pub fn steers(&mut self) -> Vec<String> {
        self.pump();
        std::mem::take(&mut self.steers)
    }

    fn wait<T>(&mut self, cancel: &AtomicBool, mut found: impl FnMut(&mut Inbox) -> Option<T>) -> Option<T> {
        loop {
            if let Some(value) = found(self) {
                return Some(value);
            }
            if self.closed || cancel.load(Ordering::Relaxed) {
                return None;
            }
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(op) => self.take_running(op),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.closed = true,
            }
        }
    }

    pub fn decision(&mut self, id: &str, cancel: &AtomicBool) -> Decision {
        self.wait(cancel, |inbox| inbox.decisions.remove(id)).unwrap_or(Decision::Decline)
    }

    pub fn answer(&mut self, id: &str, cancel: &AtomicBool) -> Option<Vec<String>> {
        self.wait(cancel, |inbox| inbox.answers.remove(id))
    }

    fn next(&mut self) -> Option<Op> {
        if let Some(op) = self.queue.pop_front() {
            return Some(op);
        }
        while !self.closed {
            match self.rx.recv() {
                Ok(op) => {
                    if let Some(op) = self.take(op) {
                        return Some(op);
                    }
                }
                Err(_) => return None,
            }
        }
        None
    }
}

pub(crate) struct Session {
    pub history: Vec<Message>,
    pub inbox: Mutex<Inbox>,
    pub sink: Sink,
    pub rules: Mutex<Vec<Rule>>,
    pub raised: AtomicBool,
    pub measured: Option<(u64, usize)>,
    pub asked_for_git: bool,
}

impl Session {
    fn run(mut self) {
        loop {
            let op = self.inbox.lock().expect("inbox lock").next();
            match op {
                Some(Op::Turn { turn, input, context, cancel }) => {
                    self.emit(Event::TurnStarted { turn });
                    let outcome = turn::run(&mut self, turn, input, &context, context.provider.as_ref(), &cancel);
                    let outcome = if cancel.load(Ordering::Relaxed) && outcome == Outcome::Done { Outcome::Stopped } else { outcome };
                    self.emit(Event::TurnFinished { turn, outcome });
                }
                Some(Op::Compact { context, cancel }) => {
                    if let Err(reason) = self.compact(&context, &cancel, false) {
                        self.emit(Event::CompactFailed { reason });
                    }
                }
                Some(_) => {}
                None => break,
            }
        }
    }

    pub fn emit(&self, event: Event) {
        (self.sink)(event);
    }

    pub fn push(&mut self, message: Message) {
        self.history.push(message.clone());
        self.emit(Event::Message { message });
    }

    pub fn take_steers(&mut self) -> bool {
        let steers = self.inbox.lock().expect("inbox lock").steers();
        let any = !steers.is_empty();
        self.asked_for_git |= steers.iter().any(|text| tools::asks_for_git(text));
        for text in steers {
            self.push(Message::user(text));
        }
        any
    }

    pub fn used(&self, overhead: u64) -> u64 {
        match self.measured {
            Some((tokens, at)) if at <= self.history.len() => tokens + context::estimate(&self.history[at..]),
            _ => context::estimate(&self.history) + overhead,
        }
    }

    pub fn compact(&mut self, ctx: &TurnContext, cancel: &AtomicBool, mid_turn: bool) -> Result<(), String> {
        if self.history.len() < 2 {
            return Err("There is nothing to compact yet.".into());
        }
        self.emit(Event::Compacting);
        let (provider, model, tuning) = match &ctx.summarizer {
            Some(other) => (other.provider.as_ref(), &other.model, &other.tuning),
            None => (ctx.provider.as_ref(), &ctx.model, &ctx.tuning),
        };
        let summary = match context::summarize(provider, model, tuning, ctx.context_window, &self.history, cancel) {
            Ok(summary) => summary,
            Err(error) => return Err(format!("The conversation could not be summarised: {error}")),
        };
        let limit = ctx.context_window.map_or(16_384, context::limit);
        let kept = if mid_turn { context::recent(&self.history, limit / 4) } else { self.history.len() };
        let restored = ctx.root.as_deref().map(|root| context::restore(root, &self.history, kept, limit / 6)).unwrap_or_default();
        self.history = context::rebuild(&summary, &self.history, mid_turn, kept, &restored);
        self.measured = None;
        self.emit(Event::Compacted { history: self.history.clone() });
        Ok(())
    }
}