use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use quill_config::{ModelRef, Reasoning};
use quill_core::{Access, Decision, Event};

use crate::Msg;
use crate::backend::{Backend, Connection};
use crate::config::Config;
use crate::history::{self, Look};
use crate::text::{Line, bad, dim, warn};
use crate::thread::{Item, Turn, TurnState};
use crate::workspace::{Prompt, Workspace};

pub struct Options {
    pub prompt: String,
    pub root: PathBuf,
    pub model: Option<String>,
    pub access: Option<Access>,
    pub json: bool,
    pub resume: bool,
    pub isolate: bool,
}

const NO_ONE: &str = "The user is not available to answer. Decide for yourself and carry on.";

pub fn read_prompt(words: &[String]) -> Result<String, String> {
    let joined = words.join(" ");
    if !joined.trim().is_empty() && joined.trim() != "-" {
        return Ok(joined);
    }
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err("Give quill exec a task, for example: quill exec \"add a test for the parser\"".into());
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text).map_err(|e| format!("Could not read the task from standard input: {e}"))?;
    if text.trim().is_empty() { Err("The task on standard input was empty.".into()) } else { Ok(text) }
}

pub fn parse_model(text: &str, providers: &[quill_config::Provider], selected: Option<&ModelRef>) -> ModelRef {
    if let Some((provider, model)) = text.split_once('/')
        && providers.iter().any(|p| p.id == provider)
    {
        return ModelRef::new(provider, model);
    }
    let provider = selected.map(|selected| selected.provider.as_str()).or(providers.first().map(|p| p.id.as_str())).unwrap_or_default();
    ModelRef::new(provider, text)
}

pub fn wait_for(provider: &str, backend: &mut Backend, rx: &Receiver<Msg>, settings: &quill_config::Settings) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while backend.checking(provider) && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Msg::Probed { generation, id, found }) => backend.probed(generation, &id, found, settings),
            Ok(Msg::Props { model, props }) => backend.got_props(model, props),
            _ => {}
        }
    }
}

struct Printer {
    item: usize,
    printed: usize,
    color: bool,
    width: usize,
    open_line: bool,
}

impl Printer {
    fn err(&self, line: &Line) {
        let text = if self.color { line.ansi() } else { line.text() };
        eprintln!("{text}");
    }

    fn text(&mut self, text: &str) {
        let mut out = std::io::stdout();
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
        self.open_line = !text.ends_with('\n');
    }

    fn close_text(&mut self) {
        if self.open_line {
            self.text("\n");
        }
    }

    fn look(&self) -> Look {
        Look { width: self.width, reasoning: Reasoning::Hidden, group: false }
    }

    fn advance(&mut self, turn: &Turn) {
        let working = turn.working();
        while self.item < turn.items.len() {
            let last = self.item + 1 == turn.items.len();
            match &turn.items[self.item] {
                Item::Text(text) => {
                    if text.len() > self.printed {
                        self.text(&text[self.printed..]);
                        self.printed = text.len();
                    }
                    if last && working {
                        return;
                    }
                    self.close_text();
                }
                Item::Reasoning(_) if last && working => return,
                Item::Reasoning(_) => {}
                Item::Tool(tool) => {
                    if tool.working() {
                        return;
                    }
                    if !matches!(tool.category, quill_core::Category::Plan | quill_core::Category::Question) {
                        self.close_text();
                        self.err(&history::tool_row(tool, self.look(), "•"));
                        for line in history::tool_body(tool, self.look()) {
                            self.err(&line);
                        }
                    }
                }
                Item::Plan { steps, explanation } => {
                    self.close_text();
                    for line in history::plan_lines(steps, explanation.as_deref(), self.look()) {
                        self.err(&line);
                    }
                }
                Item::Question { question, answer, .. } => {
                    if answer.is_none() && working {
                        return;
                    }
                    self.close_text();
                    self.err(&Line::styled(format!("? {question}"), warn()));
                    self.err(&Line::styled(format!("  › {}", answer.as_deref().unwrap_or("not answered")), dim()));
                }
                Item::Steer(_) => {}
                Item::Note(text) => self.err(&Line::styled(format!("· {text}"), dim())),
            }
            self.item += 1;
            self.printed = 0;
        }
    }
}

fn width() -> usize {
    crossterm::terminal::size().map(|(columns, _)| columns as usize).unwrap_or(100).clamp(40, 160) - 1
}

pub fn run(options: Options) -> u8 {
    let mut config = Config::load();
    if let Some(error) = &config.error {
        eprintln!("{error}. Using the defaults for this run.");
    }
    let (tx, rx) = channel();
    let mut backend = Backend::new(tx.clone());
    let mut workspace = Workspace::new(tx.clone());
    workspace.access = options.access;
    if let Some(text) = &options.model {
        let model = parse_model(text, &config.settings.models.providers, config.settings.models.selected.as_ref());
        config.settings.models.selected = Some(model);
    }
    let Some(model) = config.settings.models.selected.clone() else {
        eprintln!(
            "No model is selected yet. Run quill and pick one with /model, or pass --model provider/model (quill models lists them)."
        );
        return 2;
    };
    let settings = config.settings.clone();
    if options.resume {
        match workspace.threads_in(&options.root).first().map(|thread| thread.id) {
            Some(id) => workspace.open(id),
            None => eprintln!("No earlier thread in this folder, starting a new one."),
        }
    }
    backend.refresh(&settings);
    wait_for(&model.provider, &mut backend, &rx, &settings);
    if let Some(source) = backend.source(&model.provider).filter(|source| source.connection != Connection::Online) {
        eprintln!("{}: {}", source.config.name, source.status());
        return 1;
    }
    let interrupt = tx.clone();
    let _ = ctrlc::set_handler(move || {
        let _ = interrupt.send(Msg::Interrupt);
    });
    let prompt = Prompt { text: options.prompt, attachments: Vec::new() };
    let Some(index) = workspace.send(prompt, &options.root, options.isolate, &mut backend, &settings) else { return 1 };
    let Some(id) = workspace.active else { return 1 };
    let mut printer = Printer { item: 0, printed: 0, color: std::io::stderr().is_terminal(), width: width(), open_line: false };
    let mut stopping: Option<Instant> = None;
    loop {
        let turn_done = workspace.active_thread().and_then(|thread| thread.turns.get(index)).is_some_and(|turn| !turn.working());
        if turn_done && stopping.is_none() {
            break;
        }
        let message = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(message) => message,
            Err(_) if stopping.is_some_and(|at| at.elapsed() > Duration::from_secs(5)) => break,
            Err(_) => continue,
        };
        match message {
            Msg::Session(session, event) => {
                if options.json
                    && let Ok(line) = serde_json::to_string(&event)
                {
                    println!("{line}");
                }
                if stopping.is_some() && matches!(event, Event::TurnFinished { .. }) {
                    break;
                }
                let approval = matches!(event, Event::ApprovalRequested { .. });
                let question = matches!(event, Event::QuestionAsked { .. });
                workspace.apply(session, event);
                if approval && let Some(pending) = workspace.pending() {
                    let line = Line::styled(format!("! Declined: {} {}", pending.approval.title, pending.approval.subject), bad());
                    if !options.json {
                        printer.err(&line);
                        printer.err(&Line::styled("  quill exec cannot ask you. Pass --access auto to allow it.", dim()));
                    }
                    workspace.decide(Decision::Decline);
                }
                if question {
                    let count = workspace.asked().map_or(0, |asked| asked.questions.len());
                    workspace.answer(vec![NO_ONE.into(); count]);
                }
            }
            Msg::Interrupt => {
                if stopping.is_some() {
                    break;
                }
                workspace.stop();
                stopping = Some(Instant::now());
            }
            Msg::Probed { generation, id, found } => backend.probed(generation, &id, found, &settings),
            Msg::Props { model, props } => backend.got_props(model, props),
            _ => {}
        }
        if !options.json
            && let Some(turn) = workspace.threads.iter().find(|thread| thread.id == id).and_then(|thread| thread.turns.get(index))
        {
            printer.advance(turn);
        }
    }
    let Some(turn) = workspace.threads.iter().find(|thread| thread.id == id).and_then(|thread| thread.turns.get(index)) else { return 1 };
    if !options.json {
        printer.advance(turn);
        printer.close_text();
        let look = Look { width: printer.width, reasoning: Reasoning::Hidden, group: false };
        for line in history::ending_lines(turn, look) {
            printer.err(&line);
        }
    }
    let code = match turn.state {
        TurnState::Done | TurnState::Working => 0,
        TurnState::Stopped => 130,
        TurnState::Failed(_) => 1,
    };
    workspace.shutdown();
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_name_their_provider() {
        let kind = quill_config::ServerKind::Ollama;
        let providers = vec![quill_config::Provider::new("ollama", kind, &kind.default_url())];
        let selected = ModelRef::new("ollama-2", "gemma");
        assert_eq!(parse_model("ollama/qwen3:8b", &providers, None), ModelRef::new("ollama", "qwen3:8b"));
        assert_eq!(parse_model("qwen", &providers, Some(&selected)), ModelRef::new("ollama-2", "qwen"));
        assert_eq!(parse_model("org/model", &providers, None), ModelRef::new("ollama", "org/model"));
    }
}