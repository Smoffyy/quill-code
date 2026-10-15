use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quill_providers::{Effort, Message, Provider};

use crate::claude_code::{self, ClaudeCode};
use crate::options::{Limits, Toolsets};
use crate::policy::{Access, Rule};
use crate::protocol::{ClaudeLink, Decision, Event, Outcome};
use crate::turn;

#[derive(Clone)]
pub enum Engine {
    Model(Arc<dyn Provider>),
    ClaudeCode(ClaudeCode),
}

#[derive(Clone)]
pub struct Summarizer {
    pub provider: Arc<dyn Provider>,
    pub model: String,
}

#[derive(Clone)]
pub struct TurnContext {
    pub engine: Engine,
    pub summarizer: Option<Summarizer>,
    pub model: String,
    pub thinking: Option<bool>,
    pub effort: Option<Effort>,
    pub max_output: Option<u64>,
    pub server_compaction: bool,
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
    Answer { id: String, text: String },
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
        used: None,
        claude: None,
        link: None,
    };
    std::thread::Builder::new().name("quill-session".into()).spawn(move || session.run()).expect("start a session thread");
    SessionHandle { ops }
}

pub(crate) struct Inbox {
    rx: Receiver<Op>,
    queue: VecDeque<Op>,
    steers: Vec<String>,
    decisions: HashMap<String, Decision>,
    answers: HashMap<String, String>,
    closed: bool,
}

impl Inbox {
    fn take(&mut self, op: Op) -> Option<Op> {
        match op {
            Op::Steer(text) => self.steers.push(text),
            Op::Decide { id, decision } => {
                self.decisions.insert(id, decision);
            }
            Op::Answer { id, text } => {
                self.answers.insert(id, text);
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

    pub fn answer(&mut self, id: &str, cancel: &AtomicBool) -> Option<String> {
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
    pub used: Option<u64>,
    pub claude: Option<claude_code::Process>,
    pub link: Option<ClaudeLink>,
}

impl Session {
    fn run(mut self) {
        loop {
            let op = self.inbox.lock().expect("inbox lock").next();
            match op {
                Some(Op::Turn { turn, input, context, cancel }) => {
                    self.emit(Event::TurnStarted { turn });
                    let outcome = match &context.engine {
                        Engine::Model(provider) => turn::run(&mut self, turn, input, &context, provider.as_ref(), &cancel),
                        Engine::ClaudeCode(claude) => claude_code::run(&mut self, turn, input, &context, claude, &cancel),
                    };
                    let outcome = if cancel.load(Ordering::Relaxed) && outcome == Outcome::Done { Outcome::Stopped } else { outcome };
                    self.emit(Event::TurnFinished { turn, outcome });
                }
                Some(Op::Compact { context, cancel }) => {
                    let _ = self.compact(&context, &cancel);
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
        for text in steers {
            self.push(Message::user(text));
        }
        any
    }

    pub fn compact(&mut self, ctx: &TurnContext, cancel: &AtomicBool) -> Result<(), String> {
        let Engine::Model(provider) = &ctx.engine else {
            return Err("Claude Code keeps its own context and compacts it when it needs to.".into());
        };
        let mut changed = false;
        if let Some(cut) = crate::context::cut(&self.history) {
            let summary = match &ctx.summarizer {
                Some(other) => {
                    crate::context::summarize(other.provider.as_ref(), &other.model, None, ctx.context_window, &self.history[..cut], cancel)
                }
                None => {
                    crate::context::summarize(provider.as_ref(), &ctx.model, ctx.effort, ctx.context_window, &self.history[..cut], cancel)
                }
            };
            if let Ok(summary) = summary {
                let tail = self.history.split_off(cut);
                self.history = crate::context::join(&summary, tail);
                changed = true;
            }
        }
        changed |= crate::context::trim_outputs(&mut self.history, 6);
        if !changed {
            return Err("There was nothing to compact.".into());
        }
        for message in &mut self.history {
            message.native = None;
        }
        self.used = None;
        self.emit(Event::Compacted { history: self.history.clone() });
        Ok(())
    }
}