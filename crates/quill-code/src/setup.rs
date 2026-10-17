use std::sync::mpsc::Sender;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use quill_config::{Provider, ServerKind};
use quill_providers::{Model, context_label};

use crate::Msg;
use crate::backend;
use crate::composer::Composer;
use crate::picker::{Choice, Picked, Picker};
use crate::text::{Line, accent, bad, bold, dim, truncate, wrap};

pub enum Outcome {
    Stay,
    Close,
    Ready(Provider, String),
}

enum Step {
    Kind(Picker<ServerKind>),
    Address(Composer),
    Key(Composer),
    Connecting,
    Failed(String),
    Models(Picker<String>),
}

pub struct Setup {
    welcome: bool,
    step: Step,
    kind: ServerKind,
    url: String,
    key: Option<String>,
    found: Vec<(ServerKind, String)>,
    error: Option<String>,
    tx: Sender<Msg>,
}

fn filled(text: &str) -> Composer {
    let mut input = Composer::default();
    input.set(text);
    input
}

fn model_detail(model: &Model) -> String {
    let mut parts = vec![model.status.label().to_string()];
    if let Some(context) = model.context {
        parts.push(format!("{} context", context_label(context)));
    }
    if model.vision {
        parts.push("vision".into());
    }
    parts.join(" · ")
}

impl Setup {
    pub fn new(welcome: bool, tx: Sender<Msg>) -> Setup {
        let mut setup = Setup {
            welcome,
            step: Step::Connecting,
            kind: ServerKind::Custom,
            url: String::new(),
            key: None,
            found: Vec::new(),
            error: None,
            tx,
        };
        setup.step = Step::Kind(setup.kind_picker());
        setup
    }

    fn kind_picker(&self) -> Picker<ServerKind> {
        let choices = ServerKind::ALL
            .iter()
            .map(|kind| {
                let detail = match self.found.iter().find(|(found, _)| found == kind) {
                    Some((_, url)) => format!("running now at {}", url.trim_start_matches("http://")),
                    None => kind.description().to_string(),
                };
                Choice::new(kind.label(), detail, *kind)
            })
            .collect();
        let title = if self.welcome { "Welcome to Quill Code. Which app runs your models?" } else { "Connect a model server" };
        let close = if self.welcome { "esc sets it up later" } else { "esc closes" };
        let mut picker = Picker::new(title, choices).hint(format!("↑↓ choose · enter continue · {close}"));
        if let Some(index) = ServerKind::ALL.iter().position(|kind| self.found.iter().any(|(found, _)| found == kind)) {
            picker.select(index);
        }
        picker
    }

    pub fn detected(&mut self, found: Vec<(ServerKind, String)>) {
        self.found = found;
        if matches!(self.step, Step::Kind(_)) {
            self.step = Step::Kind(self.kind_picker());
        }
    }

    pub fn working(&self) -> bool {
        matches!(self.step, Step::Connecting)
    }

    fn provider(&self) -> Provider {
        Provider { key: self.key.clone(), ..Provider::new("", self.kind, &self.url) }
    }

    fn address(&self) -> Step {
        Step::Address(filled(&self.url))
    }

    fn connect(&mut self) {
        self.error = None;
        self.step = Step::Connecting;
        let provider = self.provider();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Connected(backend::list_models(&provider)));
        });
    }

    pub fn connected(&mut self, result: Result<Vec<Model>, String>) {
        if !self.working() {
            return;
        }
        self.step = match result {
            Ok(models) if models.is_empty() => {
                Step::Failed(format!("{} answered but has no models yet. Download or load one, then try again.", self.kind.label()))
            }
            Ok(models) => {
                let choices = models.iter().map(|model| Choice::new(model.label(), model_detail(model), model.id.clone())).collect();
                Step::Models(
                    Picker::new("Pick the model that powers Quill", choices)
                        .filtered(true)
                        .hint("type to filter · enter choose · esc goes back"),
                )
            }
            Err(error) => Step::Failed(error),
        };
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let enter = key.code == KeyCode::Enter && !key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
        match &mut self.step {
            Step::Kind(picker) => match picker.key(key) {
                Picked::Closed => return Outcome::Close,
                Picked::Chosen(index) => {
                    self.kind = picker.choices[index].value;
                    let found = self.found.iter().find(|(kind, _)| *kind == self.kind).map(|(_, url)| url.clone());
                    self.url = found.unwrap_or_else(|| self.kind.default_url());
                    self.error = None;
                    self.step = self.address();
                }
                Picked::Nothing => {}
            },
            Step::Address(input) => {
                if key.code == KeyCode::Esc {
                    self.error = None;
                    self.step = Step::Kind(self.kind_picker());
                } else if enter {
                    let url = input.text.trim().to_string();
                    if url.is_empty() {
                        self.error = Some("Type the server's address, for example http://127.0.0.1:9931.".into());
                        return Outcome::Stay;
                    }
                    self.url = url;
                    if self.kind.asks_key() {
                        self.error = None;
                        self.step = Step::Key(filled(self.key.as_deref().unwrap_or_default()));
                    } else {
                        self.connect();
                    }
                } else {
                    input.key(key);
                }
            }
            Step::Key(input) => {
                if key.code == KeyCode::Esc {
                    self.step = self.address();
                } else if enter {
                    self.key = Some(input.text.trim().to_string()).filter(|key| !key.is_empty());
                    self.connect();
                } else {
                    input.key(key);
                }
            }
            Step::Connecting => {}
            Step::Failed(_) => match key.code {
                KeyCode::Enter => self.connect(),
                KeyCode::Esc => self.step = self.address(),
                _ => {}
            },
            Step::Models(picker) => match picker.key(key) {
                Picked::Closed => self.step = self.address(),
                Picked::Chosen(index) => {
                    let model = picker.choices[index].value.clone();
                    return Outcome::Ready(self.provider(), model);
                }
                Picked::Nothing => {}
            },
        }
        Outcome::Stay
    }

    pub fn paste(&mut self, text: &str) {
        match &mut self.step {
            Step::Address(input) | Step::Key(input) => input.insert(text.trim()),
            Step::Kind(picker) => picker.paste(text),
            Step::Models(picker) => picker.paste(text),
            _ => {}
        }
    }

    pub fn render(&self, width: usize, rows: usize, spinner: &str) -> (Vec<Line>, Option<(usize, usize)>) {
        let label = self.kind.label();
        let mut lines = Vec::new();
        let mut cursor = None;
        match &self.step {
            Step::Kind(picker) => lines.extend(picker.render(width, rows)),
            Step::Address(input) => {
                lines.push(Line::styled(format!("Where is your {label} server?"), bold()));
                let example = self.kind.default_url();
                let (text, at) = input.render(width, &example);
                cursor = Some((lines.len() + at.0, at.1));
                lines.extend(text);
                lines.push(Line::styled("  enter continues · esc goes back", dim()));
            }
            Step::Key(input) => {
                lines.push(Line::styled("API key", bold()).with(" (optional)", dim()));
                let typed = input.text.chars().count();
                let mut line = Line::styled("› ", accent());
                if typed == 0 {
                    line.push("leave it empty if the server has none", dim());
                } else {
                    line.push("•".repeat(typed), accent());
                }
                cursor = Some((lines.len(), 2 + input.text[..input.cursor].chars().count()));
                lines.push(line);
                if self.kind == ServerKind::Jan {
                    lines.push(Line::styled("  Jan asks for the key set in its Local API Server settings.", dim()));
                }
                lines.push(Line::styled("  enter connects · esc goes back", dim()));
            }
            Step::Connecting => lines.push(Line::styled(format!("{spinner} Looking for models at {}", self.url), accent())),
            Step::Failed(error) => {
                lines.push(Line::styled(format!("Quill could not use {label} at {}", self.url), bold()));
                for line in wrap(&Line::styled(error.clone(), bad()), width.saturating_sub(2)) {
                    lines.push(Line::plain("  ").with_line(line));
                }
                lines.push(Line::styled("  enter tries again · esc changes the address", dim()));
            }
            Step::Models(picker) => lines.extend(picker.render(width, rows)),
        }
        if let Some(error) = &self.error {
            lines.push(truncate(&Line::styled(format!("  {error}"), bad()), width));
        }
        (lines, cursor)
    }
}