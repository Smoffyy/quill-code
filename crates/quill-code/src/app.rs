use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::{ContentStyle, Stylize};
use quill_config::{ModelRef, Provider, Reasoning};
use quill_core::{Access, Decision, Event};
use quill_git::github::{self, PullRequest, PullState};
use quill_git::{self as git, Snapshot};
use quill_providers::{Control, context_label, normalize_url};
use quill_weave::State;

use crate::Msg;
use crate::attachments::Attachment;
use crate::backend::{self, Backend, Connection};
use crate::clipboard;
use crate::commit::{Flow, Update};
use crate::composer::Composer;
use crate::config::{Config, personal_instructions_path};
use crate::history::{self, Look, Progress};
use crate::mentions::{self, Command, INIT_PROMPT, Trigger};
use crate::picker::{Choice, Picked, Picker};
use crate::project;
use crate::setup::{self, Setup};
use crate::term::{self, Live, Screen, Scroll};
use crate::text::{Line, accent, bad, blue, bold, caret, dim, good, indent, pad, plain, truncate, warn, wrap};
use crate::thread::{Item, Turn, ago};
use crate::update;
use crate::usage;
use crate::weave;
use crate::workspace::{Prompt, Workspace};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const PLACEHOLDER: &str = "Ask Quill to do something";
const HINTS: [&str; 2] = ["shift+tab mode · / commands · @ files", "shift+tab mode"];
const SUGGESTIONS: usize = 8;
const THOUGHT_EVERY: Duration = Duration::from_millis(2500);
const INTRO_NOTES: usize = 6;

type Frame = Live;

#[derive(Clone, Default)]
pub struct Repo {
    pub root: PathBuf,
    pub snapshot: Option<Snapshot>,
    pub remote: Option<String>,
    pub pull: Option<PullRequest>,
}

#[derive(Clone)]
enum Think {
    Thinking(bool),
    Effort(String),
}

enum Overlay {
    Model(Picker<Option<ModelRef>>),
    Think(Picker<Think>),
    Mode(Picker<Access>),
    Resume(Picker<u64>, Option<u64>),
    Branch(Picker<String>, PathBuf),
    Commit(Box<Flow>),
    Setup(Box<Setup>),
    Trust(Picker<bool>),
    Version(Picker<String>),
}

enum Suggestions {
    Commands(Vec<Command>),
    Files { start: usize, paths: Vec<String> },
}

impl Suggestions {
    fn len(&self) -> usize {
        match self {
            Suggestions::Commands(commands) => commands.len(),
            Suggestions::Files { paths, .. } => paths.len(),
        }
    }
}

struct View {
    thread: u64,
    turn: usize,
    progress: Progress,
}

pub struct App {
    config: Config,
    backend: Backend,
    workspace: Workspace,
    root: PathBuf,
    isolate: bool,
    composer: Composer,
    overlay: Option<Overlay>,
    suggestions: Option<Suggestions>,
    highlighted: usize,
    files: Option<(PathBuf, Vec<String>)>,
    choice: usize,
    instead: String,
    view: Option<View>,
    out: Vec<Line>,
    queued: Option<Prompt>,
    repo: Repo,
    git: bool,
    started: Instant,
    width: usize,
    height: usize,
    quit_at: Option<Instant>,
    quit: bool,
    guided: bool,
    thought: Option<(Instant, String)>,
    intro: bool,
    intro_notes: Vec<Line>,
    clear: bool,
    editing: Option<PathBuf>,
    waiting: Option<(bool, String)>,
    scroll: Option<Scroll>,
    switching: Option<(String, bool)>,
    tx: Sender<Msg>,
}

fn rule(width: usize) -> Line {
    Line::styled("─".repeat(width), dim())
}

fn typing_row(row: &mut Line, typed: &str, placeholder: &str, focused: bool, resting: ContentStyle) {
    if !typed.is_empty() {
        row.push(typed, bold());
        if focused {
            row.push(" ", plain().reverse());
        }
        return;
    }
    match placeholder.chars().next().filter(|_| focused) {
        Some(first) => {
            row.push(first.to_string(), dim().reverse());
            row.push(&placeholder[first.len_utf8()..], dim());
        }
        None => row.push(placeholder, resting),
    }
}

fn mode_style(access: Access) -> ContentStyle {
    let style = match access {
        Access::Supervised => good(),
        Access::AutoEdits => warn(),
        Access::Full => blue(),
    };
    style.bold()
}

fn help() -> Vec<Line> {
    let mut lines = vec![Line::styled("Commands", bold())];
    for command in Command::ALL {
        lines.push(Line::plain(format!("  /{}", pad(command.name(), Command::widest() + 2))).with(command.description(), dim()));
    }
    lines.push(Line::default());
    lines.push(Line::styled("Keys", bold()));
    let keys = [
        ("enter", "send, or steer the reply while Quill works"),
        ("shift+enter", "new line (alt+enter and ctrl+j work too)"),
        ("shift+tab", "switch between Ask first, Trust edits and Autopilot"),
        ("ctrl+o", "show or hide the model's thoughts"),
        ("page up/down", "scroll the conversation; the mouse wheel works too"),
        ("esc", "stop the reply, close a menu"),
        ("ctrl+c", "clear the prompt or stop; press twice to quit"),
        ("↑ ↓", "move through earlier prompts"),
        ("tab", "take the highlighted suggestion"),
        ("alt+v", "attach an image from the clipboard"),
        ("@path", "bring in a file; images go to models that can see them"),
    ];
    for (key, action) in keys {
        lines.push(Line::plain(format!("  {}", pad(key, Command::widest() + 3))).with(action, dim()));
    }
    lines.push(Line::default());
    lines.push(Line::styled("Paste or drag a file path into the prompt to attach the file.", dim()));
    lines
}

impl App {
    fn new(root: PathBuf, isolate: bool, tx: Sender<Msg>, width: usize, height: usize) -> App {
        let config = Config::load();
        let mut backend = Backend::new(tx.clone());
        backend.refresh(&config.settings);
        let workspace = Workspace::new(tx.clone());
        let mut prompts: Vec<(u64, String)> = workspace
            .threads_in(&root)
            .iter()
            .flat_map(|thread| thread.turns.iter().map(|turn| (turn.started, turn.prompt.clone())))
            .collect();
        prompts.sort_by_key(|(started, _)| *started);
        let skip = prompts.len().saturating_sub(200);
        let history = prompts.into_iter().skip(skip).map(|(_, prompt)| prompt).collect();
        App {
            config,
            backend,
            workspace,
            isolate,
            composer: Composer::new(history),
            overlay: None,
            suggestions: None,
            highlighted: 0,
            files: None,
            choice: 0,
            instead: String::new(),
            view: None,
            out: Vec::new(),
            queued: None,
            repo: Repo { root: root.clone(), ..Repo::default() },
            git: project::git_available(),
            started: Instant::now(),
            width,
            height,
            quit_at: None,
            quit: false,
            guided: false,
            thought: None,
            intro: true,
            intro_notes: Vec::new(),
            clear: false,
            editing: None,
            waiting: None,
            scroll: None,
            switching: None,
            tx,
            root,
        }
    }

    fn look(&self) -> Look {
        let appearance = &self.config.settings.appearance;
        Look { width: self.width, reasoning: appearance.reasoning, group: appearance.group_exploration }
    }

    fn spinner(&self) -> &'static str {
        SPINNER[(self.started.elapsed().as_millis() / 80) as usize % SPINNER.len()]
    }

    fn notice(&mut self, text: impl Into<String>) {
        let lines = wrap(&Line::styled(text.into(), dim()), self.width.saturating_sub(2));
        self.out.push(Line::default());
        self.out.extend(indent(lines, Line::styled("· ", dim()), Line::plain("  ")));
    }

    fn problem(&mut self, text: impl Into<String>) {
        let lines = wrap(&Line::styled(text.into(), bad()), self.width.saturating_sub(2));
        self.out.push(Line::default());
        self.out.extend(indent(lines, Line::styled("! ", bad()), Line::plain("  ")));
    }

    fn workdir(&self) -> PathBuf {
        self.workspace.active_thread().and_then(|thread| thread.workdir()).filter(|path| path.is_dir()).unwrap_or_else(|| self.root.clone())
    }

    fn banner(&self) -> Vec<Line> {
        let settings = &self.config.settings;
        let model = settings.models.selected.as_ref().map_or("no model yet, pick one with /model".to_string(), |m| m.model.clone());
        let place = project::short_path(&self.root);
        let access = settings.agent.access;
        let text = [
            Line::styled("Quill Code", bold()).with(format!(" v{}", env!("CARGO_PKG_VERSION")), dim()),
            Line::plain(model).with(" · ", dim()).with(access.label(), mode_style(access)),
            Line::styled(place, dim()),
            Line::styled("/help for commands · esc stops a reply", dim()),
        ];
        let mark = weave::mark(8, 4, State::Idle, self.started.elapsed().as_secs_f64());
        let mut lines = vec![Line::default()];
        lines.extend(mark.into_iter().zip(text).map(|(mark, text)| Line::plain(format!("  {mark}   ")).with_line(text)));
        lines
    }

    fn viewed_turn(&self) -> Option<&Turn> {
        let view = self.view.as_ref()?;
        self.workspace.threads.iter().find(|thread| thread.id == view.thread)?.turns.get(view.turn)
    }

    fn refresh_thought(&mut self) {
        let due = self.thought.as_ref().is_none_or(|(at, _)| at.elapsed() >= THOUGHT_EVERY);
        let thinking = match self.viewed_turn().filter(|turn| turn.working()).and_then(|turn| turn.items.last()) {
            Some(Item::Reasoning(text)) => Some(if due { history::summary(text) } else { None }),
            _ => None,
        };
        match thinking {
            None => self.thought = None,
            Some(Some(sentence)) if self.thought.as_ref().is_none_or(|(_, shown)| *shown != sentence) => {
                self.thought = Some((Instant::now(), sentence));
            }
            Some(_) => {}
        }
    }

    fn take_out(&mut self) -> Vec<Line> {
        if self.intro {
            self.intro_notes.append(&mut self.out);
            if self.intro_notes.len() <= INTRO_NOTES {
                return Vec::new();
            }
            self.end_intro();
        }
        std::mem::take(&mut self.out)
    }

    fn end_intro(&mut self) {
        if !self.intro {
            return;
        }
        self.intro = false;
        let mut lines = self.banner();
        lines.push(Line::default());
        lines.append(&mut self.intro_notes);
        lines.append(&mut self.out);
        self.out = lines;
    }

    fn toggle_thoughts(&mut self) {
        let expand = self.config.settings.appearance.reasoning != Reasoning::Expanded;
        let next = if expand { Reasoning::Expanded } else { Reasoning::Hidden };
        self.config.update(|settings| settings.appearance.reasoning = next);
        self.notice(if expand {
            "Thoughts now show in full. ctrl+o hides them again."
        } else {
            "Thoughts are hidden. ctrl+o shows them in full."
        });
    }

    fn open_instructions(&mut self) {
        let path = personal_instructions_path();
        if !path.is_file() {
            let made = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, ""));
            if let Err(error) = made {
                return self.problem(format!("{} could not be created: {error}", path.display()));
            }
        }
        self.editing = Some(path);
    }

    fn edited(&mut self, path: &std::path::Path, before: &str, result: Result<(), String>) {
        if let Err(error) = result {
            return self.problem(error);
        }
        let place = project::short_path(path);
        if std::fs::read_to_string(path).unwrap_or_default() != before {
            self.notice(format!("Saved your personal instructions in {place}. Every turn reads them."));
        } else {
            self.notice(format!("Your personal instructions are in {place}. Every turn reads them once the file is saved."));
        }
    }

    fn cycle_mode(&mut self) {
        let current = self.config.settings.agent.access;
        let index = Access::ALL.iter().position(|access| *access == current).unwrap_or(0);
        let next = Access::ALL[(index + 1) % Access::ALL.len()];
        self.config.update(|settings| settings.agent.access = next);
    }

    fn start(&mut self, resume: bool, prompt: String) {
        if let Some(error) = self.config.error.clone() {
            self.problem(format!("{error}. Using the defaults until it is fixed: quill config edit"));
        }
        if self.config.settings.general.trusted.iter().any(|folder| project::within(&self.root, folder)) {
            return self.begin(resume, prompt);
        }
        let choices = vec![
            Choice::new("Yes, trust this folder", "Quill may read and edit files and run commands here", true),
            Choice::new("No, leave", "Quill Code closes without touching anything", false),
        ];
        let title = format!("Do you trust the files in {}?", project::short_path(&self.root));
        self.overlay = Some(Overlay::Trust(Picker::new(title, choices).hint("enter choose · esc leaves")));
        self.waiting = Some((resume, prompt));
    }

    fn trust(&mut self, trusted: bool) {
        self.overlay = None;
        if !trusted {
            self.quit = true;
            return;
        }
        let root = self.root.clone();
        self.config.update(|settings| settings.general.trusted.push(root));
        if let Some((resume, prompt)) = self.waiting.take() {
            self.begin(resume, prompt);
        }
    }

    fn begin(&mut self, resume: bool, prompt: String) {
        if resume {
            match self.workspace.threads_in(&self.root).first().map(|thread| thread.id) {
                Some(id) => self.open_thread(id),
                None => self.notice("No earlier thread in this folder yet."),
            }
        }
        self.refresh_repo();
        if self.config.settings.models.providers.is_empty() {
            self.open_setup(true);
        }
        if !prompt.trim().is_empty() {
            self.send(Prompt { text: prompt, attachments: Vec::new() });
        }
    }

    fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
    }

    fn animating(&self) -> bool {
        self.intro
            || self.workspace.running()
            || self.queued.is_some()
            || matches!(&self.overlay, Some(Overlay::Commit(flow)) if flow.working())
            || matches!(&self.overlay, Some(Overlay::Setup(setup)) if setup.working())
            || self.quit_at.is_some_and(|at| at.elapsed() < Duration::from_secs(3))
    }

    fn refresh_repo(&mut self) {
        if !self.git {
            return;
        }
        let root = self.workdir();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let snapshot = git::snapshot(&root);
            let remote = snapshot.as_ref().and_then(|_| git::remote_url(&root));
            let mut repo = Repo { root, snapshot, remote, pull: None };
            let _ = tx.send(Msg::Repo(repo.clone()));
            let slug = repo.remote.as_deref().and_then(git::github_slug);
            let branch = repo.snapshot.as_ref().filter(|snapshot| snapshot.upstream).and_then(|snapshot| snapshot.branch.clone());
            if let (Some(slug), Some(branch)) = (slug, branch) {
                repo.pull = github::pull_request(&slug, &branch);
                if repo.pull.is_some() {
                    let _ = tx.send(Msg::Repo(repo));
                }
            }
        });
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.key(key),
            Msg::Paste(text) => self.paste(&text),
            Msg::Resize | Msg::Interrupt => {}
            Msg::Scroll(scroll) => self.scroll = Some(scroll),
            Msg::Session(id, event) => self.session(id, event),
            Msg::Probed { generation, id, found } => {
                self.backend.probed(generation, &id, found, &self.config.settings);
                if matches!(self.overlay, Some(Overlay::Model(_))) {
                    self.rebuild_models();
                }
                self.try_queued();
                self.guide();
            }
            Msg::Detected(found) => {
                if let Some(Overlay::Setup(setup)) = &mut self.overlay {
                    setup.detected(found);
                }
            }
            Msg::Connected(result) => {
                if let Some(Overlay::Setup(setup)) = &mut self.overlay {
                    setup.connected(result);
                }
            }
            Msg::Releases(result) => self.show_versions(result),
            Msg::Props { model, props } => self.backend.got_props(model, props),
            Msg::Repo(repo) => {
                if project::same(&repo.root, &self.workdir()) {
                    self.repo = repo;
                }
            }
            Msg::Job(job) => {
                let Some(Overlay::Commit(flow)) = &mut self.overlay else { return };
                let mut lines = Vec::new();
                let update = flow.job(job, &mut lines);
                if !lines.is_empty() {
                    self.out.push(Line::default());
                    self.out.append(&mut lines);
                }
                if update == Update::Close {
                    self.overlay = None;
                    self.refresh_repo();
                }
            }
        }
    }

    fn guide(&mut self) {
        if self.guided || self.backend.sources.iter().any(|source| source.connection == Connection::Checking) {
            return;
        }
        self.guided = true;
        let selected = self.config.settings.models.selected.clone();
        if selected.as_ref().is_some_and(|model| self.backend.find(model).is_some()) {
            return;
        }
        if self.backend.sources.iter().any(|source| !source.models.is_empty()) {
            match &selected {
                Some(model) => {
                    self.notice(format!("{} is not available right now. Pick another model, or start its server again.", model.model))
                }
                None => self.notice("Pick the model that powers Quill to get started."),
            }
            if self.overlay.is_none() {
                self.overlay = Some(Overlay::Model(self.model_picker()));
            }
            return;
        }
        let lines: Vec<String> = self
            .backend
            .sources
            .iter()
            .map(|source| format!("{} at {} is not answering: {}", source.config.name, source.config.url, source.status()))
            .collect();
        for line in lines {
            self.notice(line);
        }
        self.notice("Start it, or connect another server with /connect.");
    }

    fn open_setup(&mut self, welcome: bool) {
        self.overlay = Some(Overlay::Setup(Box::new(Setup::new(welcome, self.tx.clone()))));
        let settings = self.config.settings.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Detected(backend::scan(&settings)));
        });
    }

    fn connected(&mut self, provider: Provider, model: String) {
        let models = &self.config.settings.models;
        let same = |p: &Provider| p.kind == provider.kind && normalize_url(&p.url) == normalize_url(&provider.url);
        let id = models.providers.iter().find(|p| same(p)).map_or_else(|| models.fresh_id(provider.kind), |p| p.id.clone());
        let chosen = ModelRef::new(id.clone(), model.clone());
        let (label, url) = (provider.kind.label(), provider.url.clone());
        self.config.update(|settings| {
            settings.models.providers.retain(|p| p.id != id);
            settings.models.providers.push(Provider { id, ..provider });
            settings.models.selected = Some(chosen.clone());
        });
        self.overlay = None;
        self.backend.refresh(&self.config.settings);
        self.backend.select(chosen);
        self.guided = true;
        self.notice(format!("Connected to {label} at {url}. Quill now runs on {model}."));
    }

    fn session(&mut self, id: u64, event: Event) {
        let failed = match &event {
            Event::CompactFailed { reason } => Some(reason.clone()),
            _ => None,
        };
        let compacted = matches!(event, Event::Compacted { .. }) && !self.workspace.running();
        let finished = self.workspace.apply(id, event);
        if let Some(reason) = failed {
            self.problem(reason);
        }
        if compacted {
            self.notice("The conversation was summarised to free up the model's context.");
        }
        self.advance();
        if finished {
            self.backend.refresh(&self.config.settings);
            self.refresh_repo();
        }
    }

    fn advance(&mut self) {
        let look = self.look();
        let Some(view) = self.view.as_mut() else { return };
        let Some(turn) = self.workspace.threads.iter().find(|t| t.id == view.thread).and_then(|t| t.turns.get(view.turn)) else {
            self.view = None;
            return;
        };
        self.out.extend(view.progress.advance(turn, look));
        if view.progress.done() {
            self.view = None;
        }
    }

    fn try_queued(&mut self) {
        let Some(selected) = self.config.settings.models.selected.clone() else { return };
        if !self.backend.checking(&selected.provider)
            && let Some(prompt) = self.queued.take()
        {
            self.send(prompt);
        }
    }

    fn send(&mut self, prompt: Prompt) {
        self.end_intro();
        self.scroll = Some(Scroll::Bottom);
        self.config.reload();
        if let Some(selected) = &self.config.settings.models.selected
            && self.backend.checking(&selected.provider)
        {
            self.queued = Some(prompt);
            return;
        }
        let worktree = self.workspace.active_thread().and_then(|thread| thread.worktree.clone());
        let Some(index) = self.workspace.send(prompt, &self.root, self.isolate, &mut self.backend, &self.config.settings) else { return };
        self.isolate = false;
        let look = self.look();
        let Some(thread) = self.workspace.active_thread() else { return };
        let Some(turn) = thread.turns.get(index) else { return };
        self.out.push(Line::default());
        self.out.extend(history::prompt_lines(turn, look));
        let isolated = thread.worktree.clone().filter(|path| Some(path) != worktree.as_ref());
        let id = thread.id;
        if let Some(path) = isolated {
            self.notice(format!("Working in an isolated copy at {}", path.display()));
            self.refresh_repo();
        }
        self.view = Some(View { thread: id, turn: index, progress: Progress::new() });
        self.choice = 0;
        self.advance();
    }

    fn open_thread(&mut self, id: u64) {
        self.end_intro();
        self.workspace.open(id);
        self.view = None;
        let look = self.look();
        let Some(thread) = self.workspace.active_thread() else { return };
        let mut lines = vec![Line::default(), Line::styled(format!("── {} ──", thread.title), dim())];
        let skip = thread.turns.len().saturating_sub(10);
        if skip > 0 {
            lines.push(Line::styled(format!("· {skip} earlier turns are not shown"), dim()));
        }
        for turn in thread.turns.iter().skip(skip) {
            lines.extend(history::turn_lines(turn, look));
        }
        self.out.extend(lines);
        self.refresh_repo();
    }

    fn interrupt(&mut self) {
        self.suggestions = None;
        if matches!(self.overlay, Some(Overlay::Trust(_))) {
            return self.trust(false);
        }
        if matches!(&self.overlay, Some(Overlay::Commit(flow)) if flow.working()) {
            return;
        }
        if self.overlay.take().is_some() {
            return;
        }
        if self.workspace.running() {
            return self.stop();
        }
        if self.queued.take().is_some() {
            return;
        }
        if !self.composer.is_empty() {
            self.composer.clear();
            return;
        }
        if self.quit_at.is_some_and(|at| at.elapsed() < Duration::from_secs(3)) {
            self.quit = true;
            return;
        }
        self.quit_at = Some(Instant::now());
    }

    fn stop(&mut self) {
        self.workspace.stop();
        self.advance();
    }

    fn key(&mut self, key: KeyEvent) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if control && key.code == KeyCode::Char('c') {
            return self.interrupt();
        }
        self.quit_at = None;
        let scroll = match key.code {
            KeyCode::PageUp => Some(Scroll::Pages(1)),
            KeyCode::PageDown => Some(Scroll::Pages(-1)),
            KeyCode::Home if control => Some(Scroll::Top),
            KeyCode::End if control => Some(Scroll::Bottom),
            _ => None,
        };
        if scroll.is_some() {
            self.scroll = scroll;
            return;
        }
        if control && key.code == KeyCode::Char('d') && self.composer.is_empty() && self.overlay.is_none() && !self.workspace.running() {
            self.quit = true;
            return;
        }
        if control && key.code == KeyCode::Char('o') {
            return self.toggle_thoughts();
        }
        if self.overlay.is_some() {
            return self.overlay_key(key);
        }
        if self.workspace.pending().is_some() {
            return self.approval_key(key);
        }
        if key.code == KeyCode::BackTab {
            return self.cycle_mode();
        }
        if self.suggestions.is_some() && self.suggestion_key(key) {
            return;
        }
        if self.workspace.asked().is_some() && self.question_key(key) {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                if self.workspace.running() {
                    self.stop();
                } else if self.queued.take().is_some() {
                    self.notice("Not sent.");
                }
            }
            KeyCode::Enter if !key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => self.submit(),
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::ALT) => self.paste_image(),
            _ => {
                if self.composer.key(key) {
                    self.refresh_suggestions();
                }
            }
        }
    }

    fn paste(&mut self, text: &str) {
        match &mut self.overlay {
            Some(Overlay::Commit(flow)) => flow.paste(text),
            Some(Overlay::Setup(setup)) => setup.paste(text),
            Some(Overlay::Model(picker)) => picker.paste(text),
            Some(Overlay::Resume(picker, _)) => picker.paste(text),
            Some(Overlay::Branch(picker, _)) => picker.paste(text),
            Some(Overlay::Version(picker)) => picker.paste(text),
            Some(Overlay::Think(_) | Overlay::Mode(_) | Overlay::Trust(_)) => {}
            None if self.workspace.pending().is_some() => {
                self.instead.push_str(&text.replace(['\r', '\n'], " "));
                self.choice = self.approval_choices().len();
            }
            None if self.workspace.asked().is_some() => {
                if let Some(asked) = self.workspace.asked_mut() {
                    let current = asked.current;
                    asked.highlight[current] = asked.other_row();
                    asked.typed[current].push_str(&text.replace(['\r', '\n'], " "));
                    asked.picked[current] = None;
                }
            }
            None => {
                self.composer.paste(text);
                self.refresh_suggestions();
            }
        }
    }

    fn paste_image(&mut self) {
        match clipboard::save_image() {
            Ok(Some(path)) => {
                let name = format!("clipboard {}.png", self.composer.attachments.len() + 1);
                self.composer.attachments.push(Attachment { path, name, image: true });
            }
            Ok(None) => self.notice("No image on the clipboard."),
            Err(error) => self.problem(format!("Could not read the clipboard: {error}")),
        }
    }

    fn approval_choices(&self) -> Vec<(String, Decision)> {
        let Some(pending) = self.workspace.pending() else { return Vec::new() };
        let mut choices = vec![("Allow".to_string(), Decision::Approve)];
        if let Some(similar) = &pending.approval.similar {
            choices.push((format!("Always allow commands starting with {similar}"), Decision::AllowSimilar));
        }
        if !pending.tool.is_empty() {
            choices.push((format!("Always allow {}", quill_core::tools::title(&pending.tool)), Decision::AllowTool));
        }
        choices.push(("Allow everything in this thread".into(), Decision::AllowAll));
        choices.push(("Deny".into(), Decision::Decline));
        choices
    }

    fn approval_key(&mut self, key: KeyEvent) {
        let choices = self.approval_choices();
        let rows = choices.len() + 1;
        let typing = self.choice == choices.len();
        let plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let decision = match key.code {
            KeyCode::Up => {
                self.choice = (self.choice + rows - 1) % rows;
                None
            }
            KeyCode::Down | KeyCode::Tab => {
                self.choice = (self.choice + 1) % rows;
                None
            }
            KeyCode::Enter if typing => {
                let text = self.instead.trim().to_string();
                if !text.is_empty() {
                    self.workspace.redirect(text);
                    self.settle_approval();
                }
                None
            }
            KeyCode::Enter => choices.get(self.choice).map(|(_, decision)| *decision),
            KeyCode::Esc => Some(Decision::Decline),
            KeyCode::Backspace => {
                self.instead.pop();
                self.choice = choices.len();
                None
            }
            KeyCode::Char(c) if self.instead.is_empty() && c.is_ascii_digit() => {
                c.to_digit(10).and_then(|n| choices.get((n as usize).checked_sub(1)?)).map(|(_, decision)| *decision)
            }
            KeyCode::Char(c) if plain => {
                self.instead.push(c);
                self.choice = choices.len();
                None
            }
            _ => None,
        };
        if let Some(decision) = decision {
            self.workspace.decide(decision);
            self.settle_approval();
        }
    }

    fn settle_approval(&mut self) {
        self.instead.clear();
        self.choice = 0;
        self.advance();
    }

    fn question_key(&mut self, key: KeyEvent) -> bool {
        let Some(asked) = self.workspace.asked_mut() else { return false };
        let questions = asked.questions.len();
        let current = asked.current;
        let other = asked.other_row();
        let rows = other + 1;
        let highlight = asked.highlight[current];
        let plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Left if questions > 1 => asked.current = (current + questions - 1) % questions,
            KeyCode::Right if questions > 1 => asked.current = (current + 1) % questions,
            KeyCode::Up => asked.highlight[current] = (highlight + rows - 1) % rows,
            KeyCode::Down | KeyCode::Tab => asked.highlight[current] = (highlight + 1) % rows,
            KeyCode::Enter => {
                let typed = asked.typed[current].trim().to_string();
                if highlight == other && typed.is_empty() {
                    return true;
                }
                if asked.picked[current] != Some(highlight) {
                    asked.picked[current] = Some(highlight);
                    return true;
                }
                let answer = if highlight == other { typed } else { asked.question().options[highlight].clone() };
                self.record_answer(answer);
            }
            KeyCode::Backspace => {
                asked.highlight[current] = other;
                asked.typed[current].pop();
                asked.picked[current] = asked.picked[current].filter(|picked| *picked != other);
            }
            KeyCode::Char(c) if plain && asked.typed[current].is_empty() && c.is_ascii_digit() => {
                if let Some(row) = c.to_digit(10).and_then(|n| (n as usize).checked_sub(1)).filter(|row| *row < other) {
                    asked.highlight[current] = row;
                    asked.picked[current] = Some(row);
                }
            }
            KeyCode::Char(c) if plain => {
                asked.highlight[current] = other;
                asked.typed[current].push(c);
                asked.picked[current] = asked.picked[current].filter(|picked| *picked != other);
            }
            _ => return false,
        }
        true
    }

    fn record_answer(&mut self, text: String) {
        let Some(asked) = self.workspace.asked_mut() else { return };
        asked.answers[asked.current] = Some(text);
        if asked.finished() {
            let answers = asked.answers.iter().flatten().cloned().collect();
            self.workspace.answer(answers);
            return self.advance();
        }
        let count = asked.questions.len();
        let next = (1..count).map(|step| (asked.current + step) % count).find(|index| asked.answers[*index].is_none());
        asked.current = next.unwrap_or(asked.current);
    }

    fn refresh_suggestions(&mut self) {
        let at_end = self.composer.cursor == self.composer.text.len();
        let next = match mentions::trigger(&self.composer.text).filter(|_| at_end) {
            Some(Trigger::Slash(query)) => Some(Suggestions::Commands(Command::matching(&query))).filter(|s| s.len() > 0),
            Some(Trigger::Mention { start, query }) => {
                let root = self.workdir();
                if self.files.as_ref().is_none_or(|(indexed, _)| *indexed != root) {
                    self.files = Some((root.clone(), mentions::file_index(&root)));
                }
                let paths = self.files.as_ref().map(|(_, files)| mentions::rank(files, &query, SUGGESTIONS)).unwrap_or_default();
                Some(Suggestions::Files { start, paths }).filter(|s| s.len() > 0)
            }
            None => None,
        };
        let count = next.as_ref().map_or(0, Suggestions::len);
        self.highlighted = self.highlighted.min(count.saturating_sub(1));
        self.suggestions = next;
    }

    fn suggestion_key(&mut self, key: KeyEvent) -> bool {
        let count = self.suggestions.as_ref().map_or(0, Suggestions::len);
        let plain_enter = key.code == KeyCode::Enter && key.modifiers.is_empty();
        match key.code {
            KeyCode::Up => self.highlighted = (self.highlighted + count - 1) % count,
            KeyCode::Down => self.highlighted = (self.highlighted + 1) % count,
            KeyCode::Esc => self.suggestions = None,
            KeyCode::Tab => self.accept(false),
            KeyCode::Enter if plain_enter => self.accept(true),
            _ => return false,
        }
        true
    }

    fn accept(&mut self, run: bool) {
        let Some(suggestions) = self.suggestions.take() else { return };
        match suggestions {
            Suggestions::Commands(commands) => {
                let Some(command) = commands.get(self.highlighted).copied() else { return };
                if run && command != Command::Rename {
                    self.composer.set(&format!("/{}", command.name()));
                    self.composer.take();
                    self.run_command(command, String::new());
                } else {
                    self.composer.set(&format!("/{} ", command.name()));
                }
            }
            Suggestions::Files { start, paths } => {
                let Some(path) = paths.get(self.highlighted) else { return };
                let text = format!("{}@{path} ", &self.composer.text[..start.min(self.composer.text.len())]);
                self.composer.set(&text);
            }
        }
        self.highlighted = 0;
    }

    fn submit(&mut self) {
        if self.composer.is_empty() {
            return;
        }
        self.scroll = Some(Scroll::Bottom);
        let text = self.composer.text.trim().to_string();
        if self.composer.attachments.is_empty() && text.starts_with('/') {
            match Command::parse(&text) {
                Some((command, argument)) => {
                    self.composer.take();
                    self.suggestions = None;
                    return self.run_command(command, argument);
                }
                None if !text.contains(char::is_whitespace) => {
                    self.composer.take();
                    self.suggestions = None;
                    return self.problem(format!("There is no {text} command. /help lists them."));
                }
                None => {}
            }
        }
        if self.workspace.running() && !self.composer.attachments.is_empty() {
            return self.problem("Wait for the reply to finish, or stop it with esc, before sending attachments.");
        }
        let prompt = self.composer.take();
        self.suggestions = None;
        if self.workspace.asked().is_some() {
            return self.record_answer(prompt.text);
        }
        if self.workspace.running() {
            self.workspace.send(prompt, &self.root, false, &mut self.backend, &self.config.settings);
            return self.advance();
        }
        self.send(prompt);
    }

    fn busy(&mut self) -> bool {
        if self.workspace.running() {
            self.problem("Wait for the reply to finish, or stop it with esc.");
        }
        self.workspace.running()
    }

    fn run_command(&mut self, command: Command, argument: String) {
        match command {
            Command::New => {
                if !self.busy() {
                    self.workspace.start_new();
                    self.view = None;
                    self.out.push(Line::default());
                    self.out.push(Line::styled("── New thread ──", dim()));
                    self.refresh_repo();
                }
            }
            Command::Connect => self.open_setup(false),
            Command::Clear => {
                if !self.busy() {
                    self.workspace.start_new();
                    self.view = None;
                    self.out.clear();
                    self.clear = true;
                    self.intro = true;
                    self.intro_notes.clear();
                    self.refresh_repo();
                }
            }
            Command::Resume => {
                if !self.busy() {
                    self.open_resume();
                }
            }
            Command::Model => {
                self.config.reload();
                self.overlay = Some(Overlay::Model(self.model_picker()));
            }
            Command::Effort => self.open_think(),
            Command::Mode => {
                let current = self.config.settings.agent.access;
                let choices =
                    Access::ALL.map(|access| Choice::new(access.label(), access.description(), access).current(access == current));
                self.overlay = Some(Overlay::Mode(Picker::new("How much may Quill do without asking?", choices.into())));
            }
            Command::Instructions => self.open_instructions(),
            Command::Init => {
                if !self.busy() {
                    self.send(Prompt { text: INIT_PROMPT.into(), attachments: Vec::new() });
                }
            }
            Command::Compact => match self.workspace.compact(&self.backend, &self.config.settings) {
                Ok(()) => self.notice("Compacting this thread…"),
                Err(error) => self.problem(error),
            },
            Command::Retry => {
                if !self.busy() {
                    match self.workspace.last_prompt() {
                        Some(text) => self.send(Prompt { text, attachments: Vec::new() }),
                        None => self.problem("Nothing to retry yet."),
                    }
                }
            }
            Command::Diff => self.print_diff(),
            Command::Usage => {
                let lines = usage::report(self.workspace.active_thread(), &self.workspace.usage, self.width);
                self.out.push(Line::default());
                self.out.extend(lines);
            }
            Command::Version => self.open_versions(),
            Command::Commit => self.open_commit(),
            Command::Branch => {
                if !self.busy() {
                    self.open_branch();
                }
            }
            Command::Rename => match self.workspace.active {
                Some(_) if argument.is_empty() => self.problem("Give the new title: /rename Fix the login form"),
                Some(id) => {
                    self.workspace.rename(id, argument.clone());
                    self.notice(format!("Renamed to {argument}"));
                }
                None => self.problem("There is no thread to rename yet."),
            },
            Command::Help => {
                self.out.push(Line::default());
                self.out.extend(help());
            }
            Command::Quit => self.quit = true,
        }
    }

    fn open_versions(&mut self) {
        if self.busy() {
            return;
        }
        if let Some(note) = update::source_note() {
            return self.problem(note);
        }
        self.notice("Looking up the releases…");
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Releases(update::releases()));
        });
    }

    fn show_versions(&mut self, result: Result<Vec<github::Release>, String>) {
        let releases = match result {
            Ok(releases) if releases.is_empty() => return self.notice("There is no published release yet."),
            Ok(releases) => releases,
            Err(error) => return self.problem(format!("Could not list the releases. {error}")),
        };
        let choices = update::choices(&releases);
        self.overlay = Some(Overlay::Version(Picker::new("Switch Quill Code to another release", choices)));
    }

    fn switch_to(&mut self, version: String) {
        if version == update::VERSION {
            return self.notice(format!("Quill Code {version} is the one running."));
        }
        if self.busy() {
            return;
        }
        let resume = self.workspace.active_thread().is_some_and(|thread| !thread.turns.is_empty());
        if let Some(id) = self.workspace.active.filter(|_| resume) {
            self.workspace.touch(id);
        }
        self.switching = Some((version, resume));
        self.quit = true;
    }

    fn print_diff(&mut self) {
        let look = self.look();
        let Some(thread) = self.workspace.active_thread() else { return self.problem("Nothing has changed in this thread yet.") };
        let Some((index, turn)) = thread.turns.iter().enumerate().rev().find(|(_, turn)| turn.diffs().next().is_some()) else {
            return self.problem("Nothing has changed in this thread yet.");
        };
        let mut lines = vec![Line::default(), Line::styled(format!("Changes from reply {}", index + 1), bold())];
        for diff in turn.diffs() {
            lines.push(Line::default());
            let mut header =
                Line::styled(&diff.path, accent()).with(format!(" +{}", diff.added), good()).with(format!(" -{}", diff.removed), bad());
            if diff.created {
                header.push(" new", dim());
            }
            lines.push(header);
            lines.extend(history::diff_lines(diff, usize::MAX, look));
        }
        self.out.extend(lines);
    }

    fn model_picker(&self) -> Picker<Option<ModelRef>> {
        let settings = &self.config.settings;
        let selected = settings.models.selected.as_ref();
        let detail = |source: &crate::backend::Source, model: &quill_providers::Model| {
            let mut parts = vec![source.config.name.clone(), model.status.label().to_string()];
            if let Some(context) = model.context {
                parts.push(format!("{} context", context_label(context)));
            }
            if model.vision {
                parts.push("vision".into());
            }
            parts.join(" · ")
        };
        let mut choices = Vec::new();
        for pinned in &settings.models.pinned {
            if let Some((source, model)) = self.backend.find(pinned) {
                let label = format!("{} *", model.label());
                choices.push(Choice::new(label, detail(source, model), Some(pinned.clone())).current(Some(pinned) == selected));
            }
        }
        let mut unavailable = Vec::new();
        for source in self.backend.sources.iter().filter(|source| source.connection != Connection::Off) {
            for model in &source.models {
                let target = ModelRef::new(source.config.id.clone(), model.id.clone());
                if settings.models.pinned.contains(&target) {
                    continue;
                }
                let current = Some(&target) == selected;
                choices.push(Choice::new(model.label(), detail(source, model), Some(target)).current(current));
            }
            if source.models.is_empty() {
                unavailable.push(Choice::new(source.config.name.clone(), source.status(), None));
            }
        }
        choices.extend(unavailable);
        Picker::new("Pick the model that powers Quill", choices)
            .filtered(true)
            .hint("type to filter · enter choose · ctrl+p pins it to the top · esc close")
    }

    fn rebuild_models(&mut self) {
        let Some(Overlay::Model(old)) = &self.overlay else { return };
        let query = old.query.clone();
        let wanted = old.highlighted().and_then(|index| old.choices[index].value.clone());
        let mut fresh = self.model_picker();
        fresh.query = query;
        if let Some(index) = fresh.choices.iter().position(|choice| choice.value == wanted && wanted.is_some()) {
            fresh.select(index);
        }
        self.overlay = Some(Overlay::Model(fresh));
    }

    fn open_think(&mut self) {
        let settings = &self.config.settings;
        let choices = match self.backend.selected_control(settings) {
            Control::None => return self.problem("This model has no reasoning setting to change."),
            Control::Toggle { .. } => {
                let on = settings.models.thinking;
                vec![
                    Choice::new("Think it through", "Reason before replying, slower but sharper", Think::Thinking(true)).current(on),
                    Choice::new("Reply directly", "Skip the reasoning step", Think::Thinking(false)).current(!on),
                ]
            }
            Control::Effort { levels } => {
                let chosen = self.chosen_effort();
                levels
                    .iter()
                    .map(|level| {
                        let detail = match level.as_str() {
                            "low" => "Quick answers, least reasoning",
                            "medium" => "Balanced speed and depth",
                            "high" => "Reasons the longest, slowest",
                            _ => "",
                        };
                        Choice::new(level.clone(), detail, Think::Effort(level.clone())).current(chosen.as_ref() == Some(level))
                    })
                    .collect()
            }
        };
        self.overlay = Some(Overlay::Think(Picker::new("Reasoning", choices)));
    }

    fn chosen_effort(&self) -> Option<String> {
        let settings = &self.config.settings;
        let model = settings.models.selected.as_ref()?;
        match self.backend.tuning(model, settings).reasoning {
            Some(quill_providers::Reasoning::Effort(level)) => Some(level),
            _ => None,
        }
    }

    fn open_resume(&mut self) {
        let active = self.workspace.active;
        let choices: Vec<Choice<u64>> = self
            .workspace
            .threads_in(&self.root)
            .iter()
            .map(|thread| {
                let model =
                    thread.turns.iter().rev().find_map(|turn| turn.model.clone()).or_else(|| thread.model.clone()).unwrap_or_default();
                let turns = if thread.turns.len() == 1 { "1 turn".to_string() } else { format!("{} turns", thread.turns.len()) };
                let mut detail = format!("{} · {turns}", ago(thread.updated));
                if !model.is_empty() {
                    detail.push_str(&format!(" · {model}"));
                }
                if thread.worktree.is_some() {
                    detail.push_str(" · isolated");
                }
                Choice::new(thread.title.clone(), detail, thread.id).current(Some(thread.id) == active)
            })
            .collect();
        if choices.is_empty() {
            return self.problem("There are no threads in this folder yet.");
        }
        let picker =
            Picker::new("Go back to a thread", choices).filtered(true).hint("type to filter · enter open · ctrl+d delete · esc close");
        self.overlay = Some(Overlay::Resume(picker, None));
    }

    fn open_branch(&mut self) {
        let root = self.workdir();
        if self.repo.snapshot.is_none() {
            return self.problem(format!("{} is not a Git repository.", root.display()));
        }
        let branches = git::branches(&root);
        let current = branches.current.clone();
        let mut choices: Vec<Choice<String>> =
            branches.local.iter().map(|name| Choice::new(name.clone(), "", name.clone()).current(current.as_ref() == Some(name))).collect();
        choices.extend(branches.remote.iter().map(|name| Choice::new(name.clone(), "only on origin", name.clone())));
        let picker =
            Picker::new("Switch branch", choices).filtered(true).hint("type to filter or name a new branch · enter switch · esc close");
        self.overlay = Some(Overlay::Branch(picker, root));
    }

    fn open_commit(&mut self) {
        let root = self.workdir();
        if !self.git {
            return self.problem("Commits need Git. Install Git and try again.");
        }
        if git::snapshot(&root).is_none() {
            return self.problem(format!("{} is not a Git repository.", root.display()));
        }
        let settings = &self.config.settings;
        let role = settings.models.roles.commit.clone().or_else(|| settings.models.selected.clone());
        let writer = match role {
            Some(model) => self.backend.writer(&model, settings),
            None => Err("Choose a model first, or type a commit message yourself.".into()),
        };
        match Flow::new(root, writer, self.tx.clone()) {
            Ok(flow) => self.overlay = Some(Overlay::Commit(Box::new(flow))),
            Err(error) => self.problem(error),
        }
    }

    fn overlay_key(&mut self, key: KeyEvent) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(overlay) = self.overlay.as_mut() else { return };
        match overlay {
            Overlay::Model(picker) => {
                if control && key.code == KeyCode::Char('p') {
                    if let Some(Some(model)) = picker.highlighted().map(|index| picker.choices[index].value.clone()) {
                        self.config.update(|settings| {
                            if settings.models.pinned.contains(&model) {
                                settings.models.pinned.retain(|m| *m != model);
                            } else {
                                settings.models.pinned.push(model.clone());
                            }
                        });
                        self.rebuild_models();
                    }
                    return;
                }
                match picker.key(key) {
                    Picked::Closed => self.overlay = None,
                    Picked::Chosen(index) => match picker.choices[index].value.clone() {
                        Some(model) => {
                            let label = picker.choices[index].label.trim_end_matches(" *").to_string();
                            self.overlay = None;
                            self.config.update(|settings| settings.models.selected = Some(model.clone()));
                            self.backend.select(model);
                            self.notice(format!("Quill now runs on {label}"));
                        }
                        None => {
                            let status = picker.choices[index].detail.clone();
                            picker.note = Some(Line::styled(format!("  {status}"), warn()));
                        }
                    },
                    Picked::Nothing => {}
                }
            }
            Overlay::Think(picker) => match picker.key(key) {
                Picked::Closed => self.overlay = None,
                Picked::Chosen(index) => {
                    let choice = picker.choices[index].value.clone();
                    let label = picker.choices[index].label.clone();
                    self.overlay = None;
                    self.config.update(|settings| match choice {
                        Think::Thinking(on) => settings.models.thinking = on,
                        Think::Effort(level) => settings.models.effort = level,
                    });
                    self.notice(format!("Reasoning: {label}"));
                }
                Picked::Nothing => {}
            },
            Overlay::Mode(picker) => match picker.key(key) {
                Picked::Closed => self.overlay = None,
                Picked::Chosen(index) => {
                    let access = picker.choices[index].value;
                    self.overlay = None;
                    self.config.update(|settings| settings.agent.access = access);
                    self.notice(format!("{}: {}", access.label(), access.description()));
                }
                Picked::Nothing => {}
            },
            Overlay::Resume(picker, confirm) => {
                if control && key.code == KeyCode::Char('d') {
                    let Some(id) = picker.highlighted().map(|index| picker.choices[index].value) else { return };
                    if *confirm != Some(id) {
                        *confirm = Some(id);
                        picker.note = Some(Line::styled("  Press ctrl+d again to delete this thread", warn()));
                        return;
                    }
                    let worktree = self.workspace.threads.iter().find(|t| t.id == id).and_then(|t| t.worktree.clone());
                    let viewing = self.view.as_ref().is_some_and(|view| view.thread == id);
                    self.workspace.delete(id);
                    if viewing {
                        self.view = None;
                    }
                    self.notice("Deleted the thread.");
                    if let Some(path) = worktree.filter(|path| path.exists()) {
                        self.notice(format!("Its isolated copy stays at {}. Remove it with: git worktree remove", path.display()));
                    }
                    self.overlay = None;
                    return self.open_resume_if_any();
                }
                *confirm = None;
                match picker.key(key) {
                    Picked::Closed => self.overlay = None,
                    Picked::Chosen(index) => {
                        let id = picker.choices[index].value;
                        self.overlay = None;
                        self.open_thread(id);
                    }
                    Picked::Nothing => picker.note = None,
                }
            }
            Overlay::Branch(picker, root) => {
                let root = root.clone();
                let typed = picker.query.trim().to_string();
                let exact = picker.choices.iter().any(|choice| choice.value == typed);
                let create = key.code == KeyCode::Enter && !typed.is_empty() && !exact && picker.visible().is_empty();
                let picked = if create { None } else { Some(picker.key(key)) };
                let result = match picked {
                    None if !git::valid_branch(&typed) => Err(format!("\"{typed}\" is not a branch name Git accepts.")),
                    None => git::create_branch(&root, &typed).map(|()| format!("Created and switched to {typed}")),
                    Some(Picked::Closed) => {
                        self.overlay = None;
                        return;
                    }
                    Some(Picked::Chosen(index)) => {
                        let name = picker.choices[index].value.clone();
                        if picker.choices[index].current {
                            self.overlay = None;
                            return;
                        }
                        git::switch_branch(&root, &name).map(|()| format!("Switched to {name}"))
                    }
                    Some(Picked::Nothing) => {
                        let query = picker.query.trim().to_string();
                        picker.note = (!query.is_empty() && picker.visible().is_empty())
                            .then(|| Line::styled(format!("  enter creates {query}"), dim()));
                        return;
                    }
                };
                match result {
                    Ok(done) => {
                        self.overlay = None;
                        self.notice(done);
                        self.files = None;
                        self.refresh_repo();
                    }
                    Err(error) => picker.note = Some(Line::styled(format!("  {error}"), bad())),
                }
            }
            Overlay::Commit(flow) => {
                if flow.key(key, &mut self.out) == Update::Close {
                    self.overlay = None;
                    self.refresh_repo();
                }
            }
            Overlay::Trust(picker) => match picker.key(key) {
                Picked::Closed => self.trust(false),
                Picked::Chosen(index) => {
                    let trusted = picker.choices[index].value;
                    self.trust(trusted);
                }
                Picked::Nothing => {}
            },
            Overlay::Version(picker) => match picker.key(key) {
                Picked::Closed => self.overlay = None,
                Picked::Chosen(index) => {
                    let version = picker.choices[index].value.clone();
                    self.overlay = None;
                    self.switch_to(version);
                }
                Picked::Nothing => {}
            },
            Overlay::Setup(setup) => match setup.key(key) {
                setup::Outcome::Stay => {}
                setup::Outcome::Close => {
                    self.overlay = None;
                    if self.config.settings.models.providers.is_empty() {
                        self.notice("No model server yet. Start one, then connect it with /connect.");
                    }
                }
                setup::Outcome::Ready(provider, model) => self.connected(provider, model),
            },
        }
    }

    fn open_resume_if_any(&mut self) {
        if !self.workspace.threads_in(&self.root).is_empty() {
            self.open_resume();
        }
    }

    fn status_line(&self) -> Line {
        let settings = &self.config.settings;
        let access = settings.agent.access;
        let mut line = Line::plain(" ").with(access.label(), mode_style(access)).with(" · ", dim());
        match self.backend.selected(settings) {
            Some((_, model)) => line.push(model.label(), plain()),
            None => match &settings.models.selected {
                Some(selected) if self.backend.checking(&selected.provider) => line.push(format!("{} connecting", selected.model), dim()),
                Some(selected) => line.push(format!("{} unavailable, /model", selected.model), warn()),
                None => line.push("no model, pick one with /model", warn()),
            },
        }
        match self.backend.selected_control(settings) {
            Control::Toggle { .. } => line.push(if settings.models.thinking { " · thinking" } else { " · direct" }, dim()),
            Control::Effort { .. } => line.push(format!(" · {} effort", self.chosen_effort().unwrap_or_default()), dim()),
            Control::None => {}
        }
        let thread = self.workspace.active_thread();
        let mut place = project::name(&self.root);
        if thread.is_some_and(|thread| thread.worktree.is_some()) {
            place.push_str(" (isolated)");
        }
        line.push(format!(" · {place}"), dim());
        if let Some(snapshot) = &self.repo.snapshot {
            let branch = snapshot.branch.clone().or_else(|| snapshot.head.clone().map(|head| format!("at {head}")));
            if let Some(branch) = branch {
                line.push(format!(" {branch}"), accent());
            }
            if snapshot.changed > 0 {
                line.push(format!(" ●{}", snapshot.changed), warn());
            }
            if snapshot.ahead > 0 {
                line.push(format!(" ↑{}", snapshot.ahead), dim());
            }
        }
        if let Some(pull) = &self.repo.pull {
            let (state, style) = match pull.state {
                PullState::Open => ("", good()),
                PullState::Merged => (" merged", accent()),
                PullState::Closed => (" closed", dim()),
            };
            line.push(format!(" PR #{}{state}", pull.number), style);
        }
        if let Some(thread) = thread {
            let window =
                settings.models.selected.as_ref().and_then(|model| self.backend.effective(model, settings)).and_then(|m| m.context);
            if let (Some(used), Some(window)) = (thread.context_used.filter(|n| *n > 0), window.filter(|n| *n > 0)) {
                let percent = ((used * 100 + window / 2) / window).min(100);
                let near = used * 100 >= quill_core::compact_limit(window) * 85;
                line.push(format!(" · {percent}% of {}", context_label(window)), if near { warn() } else { dim() });
                if near && settings.agent.auto_compact {
                    line.push(" · compacts soon", warn());
                }
            }
        }
        if let Some(hint) = HINTS.iter().find(|hint| line.width() + hint.chars().count() + 3 <= self.width) {
            line.push(" ".repeat(self.width - 1 - line.width() - hint.chars().count()), plain());
            line.push(*hint, dim());
        }
        truncate(&line, self.width)
    }

    fn approval_card(&self, look: Look) -> Vec<Line> {
        let Some(pending) = self.workspace.pending() else { return Vec::new() };
        let mut lines =
            vec![Line::styled("─".repeat(self.width), accent()), Line::styled(format!(" {}", pending.approval.title), accent().bold())];
        for line in wrap(&Line::styled(pending.approval.subject.trim(), dim()), self.width.saturating_sub(2)).into_iter().take(4) {
            lines.push(Line::plain(" ").with_line(line));
        }
        let dashes = Line::styled("╌".repeat(self.width), dim());
        if !pending.approval.diffs.is_empty() {
            lines.push(dashes.clone());
        }
        let mut shown = 0;
        for diff in &pending.approval.diffs {
            if shown >= 14 {
                break;
            }
            if pending.approval.diffs.len() > 1 {
                lines.push(Line::styled(format!("  {}", diff.path), dim()));
            }
            let rows = history::diff_lines(diff, 14 - shown, look);
            shown += rows.len();
            lines.extend(rows);
        }
        if !pending.approval.diffs.is_empty() {
            lines.push(dashes);
        }
        let choices = self.approval_choices();
        for (index, (label, _)) in choices.iter().enumerate() {
            let on = index == self.choice;
            let mark = if on { Line::styled("▸ ", accent()) } else { Line::plain("  ") };
            lines.push(mark.with(format!("{}. ", index + 1), dim()).with(label, if on { bold() } else { plain() }));
        }
        let typing = self.choice == choices.len();
        let mut row = if typing { Line::styled("▸ ", accent()) } else { Line::plain("  ") };
        row.push(format!("{}. ", choices.len() + 1), dim());
        typing_row(&mut row, &self.instead, "No, and tell Quill what to do instead", typing, plain());
        lines.push(truncate(&row, self.width));
        lines.push(Line::styled("  enter choose · type to say what to do instead · esc deny", dim()));
        lines
    }

    fn question_card(&self) -> Vec<Line> {
        let Some(asked) = self.workspace.asked() else { return Vec::new() };
        let mut lines = Vec::new();
        let count = asked.questions.len();
        if count > 1 {
            let mut tabs = Line::styled("← ", dim());
            for index in 0..count {
                let style = if index == asked.current { accent().bold() } else { dim() };
                tabs.push(format!(" Question {} ", index + 1), style);
                if asked.answers[index].is_some() {
                    tabs.push("✓ ", good());
                }
            }
            tabs.push(" →", dim());
            lines.push(truncate(&tabs, self.width));
        }
        let question = asked.question();
        let text = wrap(&Line::styled(question.question.trim(), bold()), self.width.saturating_sub(2));
        lines.extend(indent(text, Line::styled("? ", accent()), Line::plain("  ")));
        let current = asked.current;
        let answer = asked.answers[current].as_deref();
        let (highlight, picked, typed) = (asked.highlight[current], asked.picked[current], &asked.typed[current]);
        let other = asked.other_row();
        for index in 0..=other {
            let on = index == highlight;
            let mut row = if on { Line::styled("▸ ", accent()) } else { Line::plain("  ") };
            row.push(format!("{}. ", index + 1), dim());
            let label = if index == other { typed.as_str() } else { question.options[index].as_str() };
            if index == other {
                typing_row(&mut row, typed, "Something else", on, dim());
            } else {
                row.push(label, if on { bold() } else { plain() });
            }
            if picked == Some(index) {
                row.push("  selected, enter again to confirm", good());
            } else if answer == Some(label) && !label.is_empty() {
                row.push(" ✓", good());
            }
            lines.push(truncate(&row, self.width));
        }
        let mut hint = "  ↑↓ or a number · enter selects · enter again confirms".to_string();
        if count > 1 {
            hint.push_str(" · ←→ switch question");
        }
        hint.push_str(" · esc cancels");
        lines.push(truncate(&Line::styled(hint, dim()), self.width));
        lines
    }

    fn suggestion_lines(&self) -> Vec<Line> {
        let Some(suggestions) = &self.suggestions else { return Vec::new() };
        let row = |index: usize, title: String, detail: &str| {
            let on = index == self.highlighted;
            let mark = if on { Line::styled("▸ ", accent()) } else { Line::plain("  ") };
            truncate(&mark.with(title, if on { bold() } else { plain() }).with(detail.to_string(), dim()), self.width)
        };
        let start = self.highlighted.saturating_sub(SUGGESTIONS - 1);
        match suggestions {
            Suggestions::Commands(commands) => commands
                .iter()
                .enumerate()
                .skip(start)
                .take(SUGGESTIONS)
                .map(|(index, command)| row(index, pad(&format!("/{}", command.name()), Command::widest() + 3), command.description()))
                .collect(),
            Suggestions::Files { paths, .. } => paths.iter().enumerate().map(|(index, path)| row(index, path.clone(), "")).collect(),
        }
    }

    fn live(&self) -> Frame {
        let look = self.look();
        let spinner = self.spinner();
        let mut middle = Vec::new();
        let running = self.workspace.running_turn();
        let turn = self.view.as_ref().and_then(|view| {
            self.workspace.threads.iter().find(|t| t.id == view.thread).and_then(|t| t.turns.get(view.turn)).map(|turn| (view, turn))
        });
        if let Some((_, turn)) = turn.filter(|_| running.is_some()) {
            let waiting = self.workspace.pending().is_some();
            let asked = self.workspace.asked().is_some();
            let state = match (waiting || asked, history::writing(turn)) {
                (true, _) => State::Idle,
                (false, true) => State::Generating,
                (false, false) => State::Thinking,
            };
            let seconds = self.started.elapsed().as_secs_f64();
            let mark = weave::mark(2, 1, state, seconds).concat();
            let mut activity = Line::styled(format!("{mark} "), accent());
            let doing = if self.workspace.compacting() {
                "Summarising the conversation to make room".to_string()
            } else {
                history::activity(turn, waiting, asked)
            };
            activity.push(doing, if waiting || asked { warn() } else { plain() });
            activity.push(format!(" · {}", history::duration(turn.elapsed_secs())), dim());
            if let Some(tokens) = history::generated(turn) {
                activity.push(format!(" · {tokens}"), dim());
            }
            activity.push(" · esc to stop", dim());
            middle.push(Line::default());
            middle.push(activity);
            middle.extend(history::plan_progress(turn, look));
        } else if self.queued.is_some() {
            let provider = self.config.settings.models.selected.as_ref().and_then(|selected| self.backend.source(&selected.provider));
            let name = provider.map_or_else(|| "the model".to_string(), |source| source.config.name.clone());
            middle.push(Line::default());
            middle.push(Line::styled(format!("{spinner} Connecting to {name}"), accent()).with(" · esc to cancel", dim()));
        }
        let approval = self.approval_card(look);
        let question = if approval.is_empty() { self.question_card() } else { Vec::new() };
        let asking = !question.is_empty();
        if !approval.is_empty() || !question.is_empty() {
            middle.push(Line::default());
        }
        middle.extend(approval.iter().cloned());
        middle.extend(question);
        if !middle.is_empty() || !self.intro {
            middle.push(Line::default());
        }
        let mut panel = Vec::new();
        let mut cursor = None;
        match &self.overlay {
            Some(overlay) => {
                panel.push(rule(self.width));
                let rows = self.height.saturating_sub(middle.len() + 8).clamp(3, 12);
                let (lines, at) = match overlay {
                    Overlay::Model(picker) => (picker.render(self.width, rows), None),
                    Overlay::Think(picker) => (picker.render(self.width, rows), None),
                    Overlay::Trust(picker) => (picker.render(self.width, rows), None),
                    Overlay::Version(picker) => (picker.render(self.width, rows), None),
                    Overlay::Mode(picker) => (picker.render(self.width, rows), None),
                    Overlay::Resume(picker, _) => (picker.render(self.width, rows), None),
                    Overlay::Branch(picker, _) => (picker.render(self.width, rows), None),
                    Overlay::Commit(flow) => flow.render(self.width, rows, spinner),
                    Overlay::Setup(setup) => setup.render(self.width, rows, spinner),
                };
                cursor = at.map(|(row, column)| (row + panel.len(), column));
                panel.extend(lines);
            }
            None if approval.is_empty() && !asking => {
                panel.push(rule(self.width));
                let (lines, at) = self.composer.render(self.width, PLACEHOLDER);
                cursor = Some((at.0 + panel.len(), at.1));
                panel.extend(lines);
                panel.push(rule(self.width));
                panel.extend(self.suggestion_lines());
            }
            None => panel.push(rule(self.width)),
        }
        if self.quit_at.is_some_and(|at| at.elapsed() < Duration::from_secs(3)) {
            panel.push(Line::styled(" Press ctrl+c again to quit", warn()));
        } else {
            panel.push(self.status_line());
        }
        let room = self.height.saturating_sub(middle.len() + panel.len());
        let mut preview = match turn {
            Some((view, turn)) => view.progress.live(turn, look, spinner, self.thought.as_ref().map(|(_, thought)| thought.as_str())),
            None => Vec::new(),
        };
        if preview.len() > room {
            preview.drain(..preview.len() - room);
        }
        let mut lines = Vec::new();
        if self.intro {
            lines.extend(self.banner());
            lines.push(Line::default());
            lines.extend(self.intro_notes.iter().cloned());
        }
        lines.extend(preview);
        let top = lines.len();
        let offset = lines.len() + middle.len();
        lines.extend(middle);
        lines.extend(panel);
        let cursor = cursor.map(|(row, column)| (row + offset, column));
        if let Some((row, column)) = cursor
            && let Some(line) = lines.get_mut(row)
        {
            *line = caret(line, column);
        }
        Live { lines, cursor, top }
    }

    fn finish(&mut self) -> Vec<Line> {
        if self.workspace.running() {
            self.stop();
        }
        self.workspace.shutdown();
        self.end_intro();
        let mut lines = std::mem::take(&mut self.out);
        if self.workspace.active.is_some() {
            lines.push(Line::default());
            lines.push(Line::styled("Continue this thread with: quill -c", dim()));
        }
        lines
    }
}

pub fn run(root: PathBuf, resume: bool, isolate: bool, prompt: String) -> u8 {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("Quill Code needs an interactive terminal. For scripts, use: quill exec \"your task\"");
        return 2;
    }
    let (tx, rx) = channel();
    let mut screen = match Screen::start() {
        Ok(screen) => screen,
        Err(error) => {
            eprintln!("The terminal could not be set up: {error}");
            return 1;
        }
    };
    let mut app = App::new(root, isolate, tx.clone(), screen.layout.room(), screen.layout.rows());
    term::listen(tx);
    app.start(resume, prompt);
    let mut drawn: Option<Frame> = None;
    loop {
        if let Some(path) = app.editing.take() {
            let out = app.take_out();
            let before = std::fs::read_to_string(&path).unwrap_or_default();
            match screen.suspend(&out, || project::edit(&path)) {
                Ok(result) => app.edited(&path, &before, result),
                Err(_) => break,
            }
            drawn = None;
        }
        if app.clear {
            app.clear = false;
            screen.clear();
            drawn = None;
        }
        if let Some(scroll) = app.scroll.take() {
            screen.layout.scroll(scroll);
            drawn = None;
        }
        app.refresh_thought();
        app.advance();
        let out = app.take_out();
        let frame = app.live();
        if !out.is_empty() || drawn.as_ref() != Some(&frame) {
            if screen.draw(&out, &frame).is_err() {
                break;
            }
            drawn = Some(frame);
        }
        if app.quit {
            break;
        }
        let wait = if app.animating() { Duration::from_millis(80) } else { Duration::from_millis(1000) };
        let first = match rx.recv_timeout(wait) {
            Ok(msg) => msg,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let batch = Instant::now();
        let mut next = Some(first);
        while let Some(msg) = next.take() {
            match msg {
                Msg::Resize => {
                    screen.resized();
                    app.resize(screen.layout.room(), screen.layout.rows());
                    drawn = None;
                }
                Msg::Scroll(scroll) => {
                    screen.layout.scroll(scroll);
                    drawn = None;
                }
                msg => app.handle(msg),
            }
            if batch.elapsed() < Duration::from_millis(25) {
                next = rx.try_recv().ok();
            }
        }
    }
    let goodbye = app.finish();
    let _ = screen.finish(&goodbye);
    match app.switching.take() {
        Some((version, resume)) => update::switch(&version, &app.root, resume),
        None => 0,
    }
}