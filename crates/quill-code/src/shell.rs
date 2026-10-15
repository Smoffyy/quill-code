use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, TitleBar};
use gpui_kit::{
    Anchor, AnimationExt, AnyElement, Context, Div, Entity, FontWeight, Hsla, IntoElement, KeystrokeEvent, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, Render, SharedString, SpringAnimation, SpringConfig, Stateful, StyleRefinement, Styled, Subscription,
    Task, Window, anchored, deferred, div, point, prelude::*, px,
};
use quill_config::{Motion, ProviderKind, Theme as ThemeChoice};
use quill_core::Access;
use quill_git::github::{self, PullRequest, PullState};
use quill_git::{self as git, Snapshot};
use quill_weave::State;

use crate::add_project::{AddProject, AddProjectEvent};
use crate::backend::{Backend, provider_icon};
use crate::commit_dialog::{CommitDialog, CommitEvent};
use crate::composer::{ApprovalCard, Composer, ComposerEvent, QuestionCard, Strip, access_icon};
use crate::config::Config;
use crate::icons::Icon;
use crate::mentions::{Command, INIT_PROMPT};
use crate::motion;
use crate::palette::{self, FONT, palette};
use crate::panel::{ClosePanel, Panel, Tab};
use crate::project;
use crate::settings::{Section, Settings, SettingsEvent};
use crate::terminal::{CloseTerminal, Terminal};
use crate::thread::{Item, Thread, ago, now_ms};
use crate::thread_view::{ThreadEvent, ThreadView};
use crate::ui::{MONO, badge, icon, icon_button, kbd, menu_item, menu_label, menu_panel, popup, separator, tip};
use crate::usage::UsageView;
use crate::version::{APP_NAME, VERSION};
use crate::weave::WeaveMark;
use crate::workspace::{Prompt, Workspace};

const GREETINGS: [&str; 4] = ["Where should we start?", "What are we building today?", "Ready when you are.", "What needs fixing?"];
const SIDEBAR: f32 = 280.;

#[derive(Clone, Copy, PartialEq)]
enum Menu {
    Project,
    Open,
    Thread(u64, Point<Pixels>),
}

#[derive(Clone, Default)]
struct RepoInfo {
    branch: Option<String>,
    repo: bool,
    has_commits: bool,
    remote: Option<String>,
    snapshot: Snapshot,
}

#[derive(Clone, Copy, PartialEq)]
enum ToastKind {
    Done,
    Problem,
}

#[derive(Clone)]
struct Toast {
    kind: ToastKind,
    title: SharedString,
    detail: SharedString,
    serial: usize,
}

#[derive(Clone)]
enum Action {
    NewThread,
    QuickChat,
    Project(PathBuf),
    AddProject,
    Settings(Section),
    Sidebar,
    Terminal,
    Review,
    Files,
    Usage,
    Commit,
    Compact,
    Stop,
    Access(Access),
}

struct Entry {
    icon: Icon,
    label: String,
    detail: String,
    keys: Option<&'static str>,
    action: Action,
}

struct Launcher {
    input: Entity<InputState>,
    highlighted: usize,
    _subscription: Subscription,
}

pub struct Shell {
    backend: Entity<Backend>,
    workspace: Entity<Workspace>,
    composer: Entity<Composer>,
    thread_view: Entity<ThreadView>,
    panel: Entity<Panel>,
    terminal: Entity<Terminal>,
    settings: Entity<Settings>,
    add_project: Option<Entity<AddProject>>,
    commit: Option<Entity<CommitDialog>>,
    hero: Entity<WeaveMark>,
    brand: Entity<WeaveMark>,
    search: Entity<InputState>,
    filter: String,
    in_settings: bool,
    panel_open: bool,
    terminal_open: bool,
    menu: Option<Menu>,
    renaming: Option<(u64, Entity<InputState>, Subscription)>,
    confirm_delete: Option<u64>,
    toast: Option<Toast>,
    toasts: usize,
    toast_timer: Option<Task<()>>,
    launcher: Option<Launcher>,
    appearance: (ThemeChoice, Motion),
    greeting: &'static str,
    git: bool,
    editor: bool,
    repos: HashMap<PathBuf, RepoInfo>,
    pulls: HashMap<(String, String), Option<PullRequest>>,
    marks: HashMap<u64, (String, Entity<WeaveMark>)>,
    running: usize,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        window.set_window_title(APP_NAME);
        let backend = cx.new(Backend::new);
        let workspace = cx.new(|_| Workspace::new(backend.clone()));
        let composer = cx.new(|cx| Composer::new(backend.clone(), window, cx));
        let thread_view = cx.new(|cx| ThreadView::new(workspace.clone(), cx));
        let usage = cx.new(|cx| UsageView::new(workspace.clone(), cx));
        let panel = cx.new(|cx| Panel::new(usage, window, cx));
        let terminal = cx.new(|cx| Terminal::new(window, cx));
        let settings = cx.new(|cx| Settings::new(backend.clone(), window, cx));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search threads"));
        let weak = cx.entity().downgrade();
        let subscriptions = vec![
            cx.observe_window_appearance(window, |_, window, cx| {
                let theme = Config::get(cx).appearance.theme;
                palette::apply(palette::resolve(theme, window.appearance()), Some(window), cx);
            }),
            cx.observe_global_in::<Config>(window, |this, window, cx| this.apply_appearance(window, cx)),
            cx.observe(&workspace, |this, _, cx| this.sync(cx)),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.repos.clear();
                    this.pulls.clear();
                    Config::reload(cx);
                    this.sync(cx);
                }
            }),
            cx.subscribe_in(&composer, window, |this, _, event, window, cx| match event {
                ComposerEvent::Submit(prompt) => {
                    let prompt = prompt.clone();
                    this.workspace.update(cx, |w, cx| w.send(prompt, cx));
                }
                ComposerEvent::Stop => this.stop_active(cx),
                ComposerEvent::Decide(decision) => {
                    let decision = *decision;
                    if let Some(id) = this.workspace.read(cx).active {
                        this.workspace.update(cx, |w, cx| w.decide(id, decision, cx));
                    }
                }
                ComposerEvent::Answer(text) => {
                    let text = text.clone();
                    if let Some(id) = this.workspace.read(cx).active {
                        this.workspace.update(cx, |w, cx| w.answer(id, text, cx));
                    }
                }
                ComposerEvent::OpenModelSettings => this.open_settings(Section::Models, window, cx),
                ComposerEvent::Command(command) => this.run_command(*command, window, cx),
                ComposerEvent::SetIsolated(isolated) => {
                    let isolated = *isolated;
                    this.workspace.update(cx, |w, cx| w.set_isolated(isolated, cx));
                }
                ComposerEvent::SwitchProject(path) => this.new_thread(Some(path.clone()), window, cx),
                ComposerEvent::AddProject => this.show_add_project(window, cx),
                ComposerEvent::RepoChanged => {
                    this.repos.clear();
                    this.pulls.clear();
                    this.sync(cx);
                }
            }),
            cx.subscribe(&thread_view, |this, _, event, cx| match event {
                ThreadEvent::OpenDiff { thread, turn } => this.open_diff(*thread, Some(*turn), cx),
                ThreadEvent::Retry { thread, turn } => {
                    let (thread, turn) = (*thread, *turn);
                    this.workspace.update(cx, |w, cx| w.retry(thread, turn, cx));
                }
            }),
            cx.subscribe(&panel, |this, _, _: &ClosePanel, cx| {
                this.panel_open = false;
                cx.notify();
            }),
            cx.subscribe_in(&terminal, window, |this, _, _: &CloseTerminal, window, cx| {
                this.terminal_open = false;
                this.focus_composer(window, cx);
                cx.notify();
            }),
            cx.subscribe_in(&settings, window, |this, _, event, window, cx| match event {
                SettingsEvent::Back => {
                    this.in_settings = false;
                    this.focus_composer(window, cx);
                    cx.notify();
                }
                SettingsEvent::AddProject => this.show_add_project(window, cx),
            }),
            cx.subscribe(&search, |this, search, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.filter = search.read(cx).value().trim().to_lowercase();
                    cx.notify();
                }
            }),
            cx.intercept_keystrokes(move |event, window, cx| {
                if let Some(this) = weak.upgrade() {
                    this.update(cx, |this, cx| this.intercept(event, window, cx));
                }
            }),
        ];
        let seed = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as usize;
        let settings_now = Config::get(cx).clone();
        if settings_now.general.remember_project
            && let Some(path) = settings_now.general.last_project.clone().filter(|p| p.is_dir())
        {
            workspace.update(cx, |w, cx| w.start_new(Some(path), cx));
        }
        composer.update(cx, |composer, cx| composer.focus(window, cx));
        let mut shell = Shell {
            backend: backend.clone(),
            workspace,
            composer,
            thread_view,
            panel,
            terminal,
            settings,
            add_project: None,
            commit: None,
            hero: cx.new(|_| WeaveMark::new(State::Idle)),
            brand: cx.new(|_| WeaveMark::new(State::Idle)),
            search,
            filter: String::new(),
            in_settings: false,
            panel_open: false,
            terminal_open: false,
            menu: None,
            renaming: None,
            confirm_delete: None,
            toast: None,
            toasts: 0,
            toast_timer: None,
            launcher: None,
            appearance: (settings_now.appearance.theme, settings_now.appearance.motion),
            greeting: GREETINGS[seed % GREETINGS.len()],
            git: project::git_available(),
            editor: project::editor_available(),
            repos: HashMap::new(),
            pulls: HashMap::new(),
            marks: HashMap::new(),
            running: 0,
            _subscriptions: subscriptions,
        };
        shell.sync(cx);
        shell
    }

    fn apply_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let appearance = Config::get(cx).appearance.clone();
        let wanted = (appearance.theme, appearance.motion);
        if wanted != self.appearance {
            self.appearance = wanted;
            palette::apply(palette::resolve(appearance.theme, window.appearance()), Some(window), cx);
            motion::apply(appearance.motion, cx);
        }
        cx.notify();
    }

    fn intercept(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let control = keystroke.modifiers.control && !keystroke.modifiers.alt;
        if self.launcher.is_some() {
            match key {
                "escape" => self.close_launcher(window, cx),
                "up" | "down" => {
                    let count = self.entries(cx).len().max(1);
                    if let Some(launcher) = self.launcher.as_mut() {
                        launcher.highlighted =
                            if key == "up" { (launcher.highlighted + count - 1) % count } else { (launcher.highlighted + 1) % count };
                    }
                    cx.notify();
                }
                "enter" => {
                    let index = self.launcher.as_ref().map_or(0, |l| l.highlighted);
                    if let Some(entry) = self.entries(cx).into_iter().nth(index) {
                        self.close_launcher(window, cx);
                        self.perform(entry.action, window, cx);
                    }
                }
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        if control && !keystroke.modifiers.shift {
            let handled = match key {
                "k" => {
                    self.open_launcher(window, cx);
                    true
                }
                "n" => {
                    let project = self.workspace.read(cx).project();
                    self.new_thread(project, window, cx);
                    true
                }
                "b" => {
                    self.toggle_sidebar(cx);
                    true
                }
                "," => {
                    self.open_settings(Section::General, window, cx);
                    true
                }
                "`" => {
                    self.toggle_terminal(window, cx);
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
            }
            return;
        }
        if key != "escape" {
            return;
        }
        if self.commit.take().is_some() {
            self.focus_composer(window, cx);
        } else if let Some(sheet) = self.add_project.clone() {
            sheet.update(cx, |sheet, cx| sheet.back(window, cx));
        } else if self.menu.is_some() || self.renaming.is_some() {
            self.renaming = None;
            self.menu = None;
            self.confirm_delete = None;
        } else {
            return;
        }
        cx.notify();
        cx.stop_propagation();
    }

    fn repo(&mut self, path: &Path) -> RepoInfo {
        if !self.git {
            return RepoInfo::default();
        }
        self.repos
            .entry(path.to_path_buf())
            .or_insert_with(|| match git::snapshot(path) {
                Some(snapshot) => RepoInfo {
                    branch: snapshot.branch.clone().or_else(|| snapshot.head.as_ref().map(|head| format!("detached at {head}"))),
                    repo: true,
                    has_commits: snapshot.head.is_some(),
                    remote: git::remote_url(path),
                    snapshot,
                },
                None => RepoInfo::default(),
            })
            .clone()
    }

    fn pull_request(&mut self, info: &RepoInfo, cx: &mut Context<Self>) -> Option<PullRequest> {
        let slug = info.remote.as_deref().and_then(git::github_slug)?;
        let branch = info.snapshot.branch.clone().filter(|_| info.snapshot.upstream)?;
        let key = (slug, branch);
        if let Some(known) = self.pulls.get(&key) {
            return *known;
        }
        self.pulls.insert(key.clone(), None);
        let (slug, branch) = key.clone();
        let task = cx.background_executor().spawn(async move { github::pull_request(&slug, &branch) });
        cx.spawn(async move |this, cx| {
            if let Some(pull) = task.await {
                _ = this.update(cx, |this, cx| {
                    this.pulls.insert(key, Some(pull));
                    cx.notify();
                });
            }
        })
        .detach();
        None
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.read(cx);
        let running = workspace.running_count();
        let project = workspace.project();
        let workdir = workspace.workdir();
        let isolated = workspace.isolated();
        let locked = workspace.active.is_some();
        let (busy, approval, question) = match workspace.active {
            Some(id) => (
                workspace.running(id),
                workspace.pending(id).map(|pending| {
                    let diffs = &pending.approval.diffs;
                    let stats = (!diffs.is_empty()).then(|| (diffs.iter().map(|d| d.added).sum(), diffs.iter().map(|d| d.removed).sum()));
                    ApprovalCard {
                        title: pending.approval.title.clone(),
                        target: pending.approval.subject.clone(),
                        stats,
                        diffs: pending.approval.diffs.clone(),
                        similar: pending.approval.similar.clone(),
                        tool: quill_core::tools::title(&pending.tool),
                    }
                }),
                workspace.asked(id).map(|asked| QuestionCard { question: asked.question.clone(), options: asked.options.clone() }),
            ),
            None => (false, None, None),
        };
        if running < self.running {
            self.repos.clear();
            self.pulls.clear();
        }
        self.running = running;
        let strip = match (&project, &workdir) {
            (Some(project_path), Some(root)) => {
                let can_isolate = self.repo(project_path).has_commits;
                Some(Strip {
                    root: root.clone(),
                    project: project_path.clone(),
                    name: project::name(project_path),
                    branch: self.repo(root).branch,
                    isolated,
                    can_isolate,
                    locked,
                })
            }
            _ => None,
        };
        let context_used = self.workspace.read(cx).active_thread().and_then(|t| t.context_used);
        self.composer.update(cx, |composer, cx| composer.set_context(busy, approval, question, strip, context_used, cx));
        self.panel.update(cx, |panel, cx| panel.set_root(workdir.clone(), cx));
        self.terminal.update(cx, |terminal, cx| terminal.set_root(workdir, cx));
        cx.notify();
    }

    fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |composer, cx| composer.focus(window, cx));
    }

    fn stop_active(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.workspace.read(cx).active {
            self.workspace.update(cx, |w, cx| w.stop(id, cx));
        }
    }

    fn new_thread(&mut self, project: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.in_settings = false;
        self.menu = None;
        if Config::get(cx).general.remember_project && Config::get(cx).general.last_project != project {
            let remembered = project.clone();
            Config::update(cx, |s| s.general.last_project = remembered);
        }
        self.workspace.update(cx, |w, cx| w.start_new(project, cx));
        self.focus_composer(window, cx);
    }

    fn run_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::New => {
                let project = self.workspace.read(cx).project();
                self.new_thread(project, window, cx);
            }
            Command::Review => self.toggle_panel(Tab::Diff, cx),
            Command::Files => self.toggle_panel(Tab::Files, cx),
            Command::Terminal => self.toggle_terminal(window, cx),
            Command::Commit => self.open_commit(window, cx),
            Command::Compact => self.compact(cx),
            _ => {}
        }
    }

    fn compact(&mut self, cx: &mut Context<Self>) {
        match self.workspace.read(cx).active {
            Some(id) if !self.workspace.read(cx).running(id) => {
                self.workspace.update(cx, |w, cx| w.compact(id, cx));
                self.show_toast(
                    "Compacting this thread".into(),
                    "Earlier turns are being summarised to free up the model's window.".into(),
                    cx,
                );
            }
            Some(_) => self.show_problem("Wait for the reply to finish".into(), "A thread can be compacted between replies.".into(), cx),
            None => self.show_problem("Nothing to compact yet".into(), "Open a thread first.".into(), cx),
        }
    }

    fn perform(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::NewThread => {
                let project = self.workspace.read(cx).project();
                self.new_thread(project, window, cx);
            }
            Action::QuickChat => self.new_thread(None, window, cx),
            Action::Project(path) => self.new_thread(Some(path), window, cx),
            Action::AddProject => self.show_add_project(window, cx),
            Action::Settings(section) => self.open_settings(section, window, cx),
            Action::Sidebar => self.toggle_sidebar(cx),
            Action::Terminal => self.toggle_terminal(window, cx),
            Action::Review => self.toggle_panel(Tab::Diff, cx),
            Action::Files => self.toggle_panel(Tab::Files, cx),
            Action::Usage => self.toggle_panel(Tab::Usage, cx),
            Action::Commit => self.open_commit(window, cx),
            Action::Compact => self.compact(cx),
            Action::Stop => self.stop_active(cx),
            Action::Access(access) => Config::update(cx, |s| s.agent.access = access),
        }
    }

    fn entries(&self, cx: &Context<Self>) -> Vec<Entry> {
        let query = self.launcher.as_ref().map(|l| l.input.read(cx).value().trim().to_lowercase()).unwrap_or_default();
        let entry = |icon, label: &str, detail: &str, keys, action| Entry {
            icon,
            label: label.to_string(),
            detail: detail.to_string(),
            keys,
            action,
        };
        let mut all = vec![
            entry(Icon::Compose, "New thread", "Start a fresh thread in this project", Some("Ctrl N"), Action::NewThread),
            entry(Icon::Chat, "Quick chat", "Talk to the model without a project or tools", None, Action::QuickChat),
            entry(Icon::FolderPlus, "Add a project", "Start fresh, open a folder or clone a repository", None, Action::AddProject),
            entry(Icon::Sliders, "Settings", "Every option, with search", Some("Ctrl ,"), Action::Settings(Section::General)),
            entry(Icon::Cube, "Models", "Connect a server and pick a model", None, Action::Settings(Section::Models)),
            entry(Icon::Bolt, "Toggle the sidebar", "Show or hide the thread list", Some("Ctrl B"), Action::Sidebar),
            entry(Icon::Terminal, "Toggle the terminal", "A PowerShell drawer in the project", Some("Ctrl `"), Action::Terminal),
            entry(Icon::FileDiff, "Review changes", "What the last reply changed", None, Action::Review),
            entry(Icon::Files, "Browse files", "The project's files", None, Action::Files),
            entry(Icon::Gauge, "Usage", "What this chat and everything else has cost", None, Action::Usage),
            entry(Icon::Commit, "Commit", "Commit your work, push or open a pull request", None, Action::Commit),
            entry(Icon::Eraser, "Compact this thread", "Summarise earlier turns to free up context", None, Action::Compact),
            entry(Icon::CircleStop, "Stop the agent", "Stop the reply that is running", None, Action::Stop),
        ];
        for access in Access::ALL {
            all.push(entry(
                access_icon(access),
                &format!("Permissions: {}", access.label()),
                access.description(),
                None,
                Action::Access(access),
            ));
        }
        for path in &Config::get(cx).general.projects {
            all.push(entry(
                Icon::Folder,
                &format!("Open {}", project::name(path)),
                &path.display().to_string(),
                None,
                Action::Project(path.clone()),
            ));
        }
        all.into_iter()
            .filter(|e| query.is_empty() || e.label.to_lowercase().contains(&query) || e.detail.to_lowercase().contains(&query))
            .take(12)
            .collect()
    }

    fn open_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command or a project"));
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event
                && let Some(launcher) = this.launcher.as_mut()
            {
                launcher.highlighted = 0;
                cx.notify();
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.menu = None;
        self.launcher = Some(Launcher { input, highlighted: 0, _subscription: subscription });
        cx.notify();
    }

    fn close_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.launcher = None;
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        Config::update(cx, |s| s.appearance.sidebar = !s.appearance.sidebar);
        cx.notify();
    }

    fn open_settings(&mut self, section: Section, _: &mut Window, cx: &mut Context<Self>) {
        self.in_settings = true;
        self.menu = None;
        self.settings.update(cx, |s, cx| s.open(section, cx));
        cx.notify();
    }

    fn show_add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let sheet = cx.new(|cx| AddProject::new(self.git, window, cx));
        let subscription = cx.subscribe_in(&sheet, window, |this, _, event, window, cx| match event {
            AddProjectEvent::Close => {
                this.add_project = None;
                this.focus_composer(window, cx);
                cx.notify();
            }
            AddProjectEvent::Added(path, note) => {
                let path = path.clone();
                this.add_project = None;
                this.repos.remove(&path);
                Config::add_project(cx, path.clone());
                let detail = note.clone().unwrap_or_else(|| path.display().to_string());
                this.show_toast(format!("{} is ready", project::name(&path)), detail, cx);
                this.new_thread(Some(path), window, cx);
            }
        });
        self._subscriptions.push(subscription);
        self.add_project = Some(sheet);
        cx.notify();
    }

    fn open_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.read(cx).workdir() else {
            self.show_problem("Pick a project first".into(), "Commits need a project that uses Git.".into(), cx);
            return;
        };
        if !self.repo(&root).repo {
            self.show_problem("Not a Git repository".into(), root.display().to_string(), cx);
            return;
        }
        self.menu = None;
        let role = Config::get(cx).models.roles.commit.clone().or_else(|| Backend::selected_ref(cx));
        let writer = match role {
            Some(model) => self.backend.read(cx).writer(&model, cx),
            None => Err("Choose a model first, or type a commit message yourself.".into()),
        };
        let dialog = cx.new(|cx| CommitDialog::new(root, writer, window, cx));
        let subscription = cx.subscribe_in(&dialog, window, |this, _, event, window, cx| match event {
            CommitEvent::Close => {
                this.commit = None;
                this.focus_composer(window, cx);
                cx.notify();
            }
            CommitEvent::Updated(note) => {
                this.repos.clear();
                let note = note.clone();
                this.show_toast(note, String::new(), cx);
                this.sync(cx);
            }
        });
        self._subscriptions.push(subscription);
        self.commit = Some(dialog);
        cx.notify();
    }

    fn push_toast(&mut self, kind: ToastKind, title: String, detail: String, cx: &mut Context<Self>) {
        self.toasts += 1;
        self.toast = Some(Toast { kind, title: title.into(), detail: detail.into(), serial: self.toasts });
        self.toast_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            _ = this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn show_toast(&mut self, title: String, detail: String, cx: &mut Context<Self>) {
        self.push_toast(ToastKind::Done, title, detail, cx);
    }

    fn show_problem(&mut self, title: String, detail: String, cx: &mut Context<Self>) {
        self.push_toast(ToastKind::Problem, title, detail, cx);
    }

    fn open_diff(&mut self, thread: u64, turn: Option<usize>, cx: &mut Context<Self>) {
        let workspace = self.workspace.read(cx);
        let Some(found) = workspace.threads.iter().find(|t| t.id == thread) else { return };
        let index = turn.unwrap_or_else(|| found.turns.iter().rposition(|t| t.diffs().next().is_some()).unwrap_or(0));
        let diffs = found.turns.get(index).map(|t| t.diffs().cloned().collect()).unwrap_or_default();
        let title = if index + 1 == found.turns.len() { "Last reply".to_string() } else { format!("Reply {}", index + 1) };
        self.panel.update(cx, |panel, cx| panel.show_diff(title, diffs, cx));
        self.panel_open = true;
        cx.notify();
    }

    fn toggle_panel(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.panel_open && self.panel.read(cx).tab == tab {
            self.panel_open = false;
            cx.notify();
            return;
        }
        match tab {
            Tab::Diff => match self.workspace.read(cx).active {
                Some(id) => self.open_diff(id, None, cx),
                None => {
                    self.panel.update(cx, |panel, cx| panel.show_diff("Last reply".into(), Vec::new(), cx));
                    self.panel_open = true;
                }
            },
            Tab::Files => {
                self.panel.update(cx, |panel, cx| panel.show_files(cx));
                self.panel_open = true;
            }
            Tab::Usage => {
                self.panel.update(cx, |panel, cx| panel.show_usage(cx));
                self.panel_open = true;
            }
        }
        cx.notify();
    }

    fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_open = !self.terminal_open;
        if self.terminal_open {
            self.terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
        } else {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    fn start_rename(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let title = self.workspace.read(cx).threads.iter().find(|t| t.id == id).map(|t| t.title.clone()).unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let subscription = cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) && this.renaming.is_some() {
                let title = input.read(cx).value().trim().to_string();
                this.renaming = None;
                if !title.is_empty() {
                    this.workspace.update(cx, |w, cx| w.rename(id, title, cx));
                }
                cx.notify();
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.renaming = Some((id, input, subscription));
        cx.notify();
    }

    fn remove_isolated(&mut self, id: u64, cx: &mut Context<Self>) {
        self.close_menu(cx);
        match self.workspace.update(cx, |w, cx| w.remove_isolated(id, cx)) {
            Ok(()) => self.show_toast("Removed the isolated copy".into(), "The thread now works in the project folder.".into(), cx),
            Err(error) => self.show_problem("Could not remove the copy".into(), error, cx),
        }
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.confirm_delete = None;
        cx.notify();
    }

    fn thread_row(&mut self, thread: &Thread, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = palette(cx);
        let info = thread.workdir().map(|path| self.repo(&path)).filter(|info| info.repo);
        let pull = info.as_ref().and_then(|info| self.pull_request(info, cx));
        let workspace = self.workspace.read(cx);
        let id = thread.id;
        let active = !self.in_settings && workspace.active == Some(id);
        let running = workspace.running(id);
        let pending = workspace.waiting(id);
        let project_name = thread.project.as_ref().map(|path| project::name(path));
        let elapsed = thread.turns.last().map(|t| t.elapsed_secs()).unwrap_or(0);
        let motion_state = match thread.turns.last().and_then(|t| t.items.last()) {
            _ if !running || pending => State::Idle,
            Some(Item::Text(_)) => State::Generating,
            _ => State::Thinking,
        };
        let model = thread.turns.iter().rev().find_map(|turn| turn.model.clone()).or_else(|| thread.model.clone());
        let logo = thread
            .provider
            .as_deref()
            .and_then(|id| Config::get(cx).models.provider(id))
            .map(|provider| provider.kind)
            .filter(|kind| *kind != ProviderKind::Local);
        let weave = model.filter(|_| logo.is_none()).map(|model| {
            let fresh = self.marks.get(&id).is_none_or(|(seeded, _)| *seeded != model);
            if fresh {
                let mark = cx.new(|_| WeaveMark::new(State::Idle).seeded(&model).opacity(0.7));
                self.marks.insert(id, (model, mark));
            }
            let mark = self.marks[&id].1.clone();
            mark.update(cx, |mark, cx| mark.set_state(motion_state, cx));
            mark
        });
        let status = if pending {
            div()
                .px(px(6.))
                .h(px(18.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .bg(p.warning.opacity(0.14))
                .text_color(p.warning)
                .child("Needs you")
        } else if running {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_color(p.accent)
                .child(motion::spin(SharedString::from(format!("row-spin-{id}")), icon(Icon::Loader, 12., p.accent)))
                .child(format!("{elapsed}s"))
        } else {
            div().text_color(p.text_faint).child(ago(thread.updated))
        };
        let mark = match &project_name {
            Some(name) => badge(name, 26.),
            None => div().size(px(26.)).flex_shrink_0().flex().items_center().justify_center().rounded(px(8.)).bg(p.fill).child(icon(
                Icon::Chat,
                14.,
                p.text_muted,
            )),
        };
        let renaming = self.renaming.as_ref().filter(|(rid, _, _)| *rid == id).map(|(_, input, _)| input.clone());
        let title = match renaming {
            Some(input) => div().flex_1().min_w_0().text_size(px(13.5)).child(Input::new(&input).small()).into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(13.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if active { p.text } else { p.sidebar_text })
                .child(thread.title.clone())
                .into_any_element(),
        };
        let snapshot = info.as_ref().map(|info| info.snapshot.clone()).unwrap_or_default();
        let branch = info.and_then(|info| info.branch);
        let place = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(12.))
            .text_color(p.text_faint)
            .child(project_name.unwrap_or_else(|| "Quick chat".into()))
            .when(thread.worktree.is_some() || branch.is_some(), |row| row.child("·"))
            .when(thread.worktree.is_some(), |row| row.child(icon(Icon::Isolated, 11., p.text_faint)))
            .when_some(branch, |row, branch| row.child(div().overflow_hidden().text_ellipsis().child(branch)));
        let spent: f64 = thread.turns.iter().filter_map(|turn| turn.cost).sum();
        let footer = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(11.5))
            .text_color(p.text_faint)
            .child(div().flex_1().min_w_0().flex().items_center().gap(px(4.)).when_some(snapshot.head, |row, head| {
                row.child(icon(Icon::Commit, 12., p.text_faint)).child(div().font_family(MONO).child(head))
            }))
            .when(snapshot.changed > 0, |row| row.child(stat(Icon::FilePen, snapshot.changed.to_string(), p.warning)))
            .when(snapshot.ahead > 0, |row| row.child(stat(Icon::ArrowUp, snapshot.ahead.to_string(), p.text_muted)))
            .when_some(pull, |row, pull| {
                let color = match pull.state {
                    PullState::Open => p.success,
                    PullState::Merged => p.link,
                    PullState::Closed => p.text_faint,
                };
                row.child(stat(Icon::PullRequest, format!("#{}", pull.number), color))
            })
            .when(spent > 0.0, |row| row.child(div().flex_shrink_0().text_color(p.text_muted).child(crate::usage::money(spent))))
            .when_some(weave, |row, weave| row.child(div().size(px(16.)).flex_shrink_0().child(weave)))
            .when_some(logo, |row, kind| row.child(div().flex_shrink_0().child(icon(provider_icon(kind), 14., p.text_muted))));
        div()
            .id(SharedString::from(format!("thread-{id}")))
            .group(SharedString::from(format!("thread-row-{id}")))
            .relative()
            .flex()
            .items_start()
            .gap(px(10.))
            .px(px(10.))
            .py(px(9.))
            .rounded(px(10.))
            .cursor_pointer()
            .when(active, |row| row.bg(p.row_active))
            .hover(move |style| style.bg(if active { p.row_active } else { p.row_hover }))
            .when(active, |row| row.child(div().absolute().left(px(-8.)).top(px(14.)).w(px(3.)).h(px(18.)).rounded_full().bg(p.accent)))
            .child(div().mt(px(2.)).child(mark))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div().flex().items_center().gap(px(8.)).child(title).child(div().flex_shrink_0().text_size(px(11.5)).child(status)),
                    )
                    .child(place)
                    .child(footer),
            )
            .child(
                div()
                    .id(SharedString::from(format!("thread-more-{id}")))
                    .absolute()
                    .top(px(7.))
                    .right(px(6.))
                    .size(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .bg(p.surface_hover)
                    .border_1()
                    .border_color(p.border_soft)
                    .invisible()
                    .group_hover(SharedString::from(format!("thread-row-{id}")), |style| style.visible())
                    .child(icon(Icon::Dots, 14., p.text_muted))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.menu = Some(Menu::Thread(id, event.position));
                            this.confirm_delete = None;
                            cx.notify();
                        }),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.in_settings = false;
                this.workspace.update(cx, |w, cx| w.activate(id, cx));
                this.focus_composer(window, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.menu = Some(Menu::Thread(id, event.position));
                    this.confirm_delete = None;
                    cx.notify();
                }),
            )
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let filter = self.filter.clone();
        let threads: Vec<Thread> = self
            .workspace
            .read(cx)
            .threads
            .iter()
            .filter(|t| filter.is_empty() || t.title.to_lowercase().contains(&filter))
            .cloned()
            .collect();
        let empty = threads.is_empty();
        let known: Vec<u64> = self.workspace.read(cx).threads.iter().map(|t| t.id).collect();
        self.marks.retain(|id, _| known.contains(id));
        let mut groups: Vec<Vec<Stateful<Div>>> = SECTIONS.iter().map(|_| Vec::new()).collect();
        for thread in &threads {
            let workspace = self.workspace.read(cx);
            let live = workspace.running(thread.id) || workspace.waiting(thread.id);
            groups[section(live, thread.updated)].push(self.thread_row(thread, cx));
        }
        let sections = SECTIONS.iter().zip(groups).filter(|(_, rows)| !rows.is_empty()).map(|(label, rows)| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .px(px(10.))
                        .pt(px(14.))
                        .pb(px(4.))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text_faint)
                        .child(*label),
                )
                .children(rows)
        });
        div()
            .w(px(SIDEBAR))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .px(px(10.))
            .pb(px(10.))
            .child(
                div()
                    .id("new-thread")
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(38.))
                    .px(px(12.))
                    .rounded(px(10.))
                    .bg(p.raised)
                    .border_1()
                    .border_color(p.border_soft)
                    .text_size(px(13.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text)
                    .cursor_pointer()
                    .hover(move |style| style.bg(p.surface_hover))
                    .child(icon(Icon::Compose, 16., p.text))
                    .child(div().flex_1().child("New thread"))
                    .child(kbd("Ctrl N", p))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let project = this.workspace.read(cx).project();
                        this.new_thread(project, window, cx);
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pt(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .h(px(34.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(9.))
                            .bg(p.fill)
                            .text_size(px(13.))
                            .child(Input::new(&self.search).appearance(false).prefix(icon(Icon::Search, 14., p.text_faint))),
                    )
                    .child(
                        icon_button("add-project", Icon::FolderPlus, "Add a project", p)
                            .on_click(cx.listener(|this, _, window, cx| this.show_add_project(window, cx))),
                    ),
            )
            .child(div().id("threads").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().children(sections).when(empty, |list| {
                list.child(
                    div()
                        .pt(px(28.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(13.))
                        .text_color(p.text_faint)
                        .child(icon(Icon::Chat, 18., p.text_faint))
                        .child(if filter.is_empty() { "Your threads will show up here" } else { "Nothing matches that" }),
                )
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(p.border_soft)
                    .child(
                        icon_button("open-settings", Icon::Gear, "Settings  Ctrl ,", p)
                            .on_click(cx.listener(|this, _, window, cx| this.open_settings(Section::General, window, cx))),
                    )
                    .child(
                        icon_button("open-models", Icon::Cube, "Models", p)
                            .on_click(cx.listener(|this, _, window, cx| this.open_settings(Section::Models, window, cx))),
                    )
                    .child(
                        icon_button("open-launcher", Icon::Search, "Commands  Ctrl K", p)
                            .on_click(cx.listener(|this, _, window, cx| this.open_launcher(window, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("open-version")
                            .px(px(8.))
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .rounded(px(7.))
                            .text_size(px(12.))
                            .text_color(p.text_faint)
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.row_hover).text_color(p.text))
                            .child(format!("v{VERSION}"))
                            .on_click(cx.listener(|this, _, window, cx| this.open_settings(Section::Version, window, cx))),
                    ),
            )
    }

    fn breadcrumb(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let workspace = self.workspace.read(cx);
        let thread = workspace.active_thread();
        let locked = thread.is_some();
        let isolated = thread.is_some_and(|t| t.worktree.is_some());
        let title = thread.map(|t| t.title.clone()).unwrap_or_else(|| "New thread".into());
        let project_path = workspace.project();
        let project_name = project_path.as_ref().map(|path| project::name(path));
        let picker = (self.menu == Some(Menu::Project)).then(|| {
            let current = project_path.clone();
            let projects = Config::get(cx).general.projects.clone();
            let menu = menu_panel(p)
                .w(px(320.))
                .child(menu_label("Projects", p))
                .children(projects.into_iter().enumerate().map(|(i, path)| {
                    let chosen = current.as_ref() == Some(&path);
                    let parent = path.parent().map(project::short_path).unwrap_or_default();
                    menu_item(("pick-project", i), Some(Icon::Folder), project::name(&path), Some(parent.into()), chosen, p)
                        .on_click(cx.listener(move |this, _, window, cx| this.new_thread(Some(path.clone()), window, cx)))
                }))
                .child(separator(p))
                .child(
                    menu_item(
                        "no-project",
                        Some(Icon::Chat),
                        "Quick chat",
                        Some("Talk to the model without a project or tools".into()),
                        current.is_none(),
                        p,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.new_thread(None, window, cx))),
                )
                .child(
                    menu_item("picker-add", Some(Icon::FolderPlus), "Add a project", None, false, p)
                        .on_click(cx.listener(|this, _, window, cx| this.show_add_project(window, cx))),
                );
            popup(window, cx, Anchor::TopLeft, menu, |this, cx| this.close_menu(cx))
        });
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .text_size(px(13.5))
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .id("project-picker")
                            .occlude()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .h(px(28.))
                            .px(px(8.))
                            .rounded(px(8.))
                            .text_color(p.text_muted)
                            .when(!locked, |b| b.cursor_pointer().hover(move |style| style.bg(p.row_hover).text_color(p.text)))
                            .child(match &project_name {
                                Some(name) => badge(name, 18.).into_any_element(),
                                None => icon(Icon::Chat, 15., p.text_faint).into_any_element(),
                            })
                            .child(project_name.clone().unwrap_or_else(|| "Quick chat".into()))
                            .when(isolated, |b| b.child(icon(Icon::Isolated, 13., p.text_faint)))
                            .when(!locked, |b| b.child(icon(Icon::ChevronDown, 13., p.text_faint)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !locked {
                                    this.menu = Some(Menu::Project);
                                    cx.notify();
                                }
                            })),
                    )
                    .children(picker),
            )
            .child(icon(Icon::ChevronRight, 13., p.text_faint))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text)
                    .child(title),
            )
    }

    fn toolbar(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let workdir = self.workspace.read(cx).workdir();
        let info = workdir.as_ref().map(|root| self.repo(root)).unwrap_or_default();
        let changes = self.workspace.read(cx).active_thread().and_then(|t| {
            t.turns.iter().rev().find(|turn| turn.diffs().next().is_some()).map(|turn| {
                let c = turn.changes();
                (c.iter().map(|x| x.added).sum::<usize>(), c.iter().map(|x| x.removed).sum::<usize>())
            })
        });
        let panel_tab = self.panel.read(cx).tab;
        let review_on = self.panel_open && panel_tab == Tab::Diff;
        let files_on = self.panel_open && panel_tab == Tab::Files;
        let usage_on = self.panel_open && panel_tab == Tab::Usage;
        let spent: f64 =
            self.workspace.read(cx).active_thread().map_or(0.0, |thread| thread.turns.iter().filter_map(|turn| turn.cost).sum());
        let open_menu = (self.menu == Some(Menu::Open)).then(|| {
            let editor = self.editor;
            let reveal = workdir.clone().unwrap_or_default();
            let code = reveal.clone();
            let menu = menu_panel(p)
                .w(px(240.))
                .child(menu_label("Open this project in", p))
                .when(editor, |menu| {
                    menu.child(menu_item("open-vscode", Some(Icon::Code), "VS Code", None, false, p).on_click(cx.listener(
                        move |this, _, _, cx| {
                            project::open_in_editor(&code);
                            this.close_menu(cx);
                        },
                    )))
                })
                .child(menu_item("open-explorer", Some(Icon::FolderOpen), "File Explorer", None, false, p).on_click(cx.listener(
                    move |this, _, _, cx| {
                        project::open_folder(&reveal);
                        this.close_menu(cx);
                    },
                )))
                .child(menu_item("open-terminal", Some(Icon::Terminal), "Terminal drawer", None, false, p).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.close_menu(cx);
                        this.toggle_terminal(window, cx);
                    },
                )));
            popup(window, cx, Anchor::TopRight, menu, |this, cx| this.close_menu(cx))
        });
        let pill = |id: &'static str, on: bool| {
            div()
                .id(id)
                .occlude()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(6.))
                .h(px(28.))
                .px(px(9.))
                .rounded(px(8.))
                .text_size(px(12.5))
                .text_color(if on { p.text } else { p.text_muted })
                .cursor_pointer()
                .when(on, |b| b.bg(p.row_active))
                .hover(move |style| style.bg(p.row_hover).text_color(p.text))
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .when(workdir.is_some(), |bar| {
                bar.child(
                    div()
                        .relative()
                        .child(
                            pill("toolbar-open", self.menu == Some(Menu::Open))
                                .tooltip(tip("Open the project in another app"))
                                .child(icon(Icon::FolderOpen, 14., p.text_muted))
                                .child("Open")
                                .child(icon(Icon::ChevronDown, 12., p.text_faint))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.menu = Some(Menu::Open);
                                    cx.notify();
                                })),
                        )
                        .children(open_menu),
                )
            })
            .when(info.repo, |bar| {
                bar.child(
                    pill("toolbar-commit", self.commit.is_some())
                        .tooltip(tip("Commit, push or open a pull request"))
                        .child(icon(Icon::Commit, 14., p.text_muted))
                        .child("Commit")
                        .when(info.snapshot.changed > 0, |b| {
                            b.child(
                                div()
                                    .px(px(5.))
                                    .rounded(px(5.))
                                    .bg(p.warning.opacity(0.16))
                                    .text_size(px(11.))
                                    .text_color(p.warning)
                                    .child(info.snapshot.changed.to_string()),
                            )
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.open_commit(window, cx))),
                )
            })
            .child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(p.border_soft))
            .child(
                pill("toolbar-review", review_on)
                    .tooltip(tip("Review what the last reply changed"))
                    .child(icon(Icon::FileDiff, 14., p.text_muted))
                    .when_some(changes, |b, (added, removed)| {
                        b.child(div().text_color(p.add).child(format!("+{added}")))
                            .child(div().text_color(p.del).child(format!("−{removed}")))
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(Tab::Diff, cx))),
            )
            .child(
                pill("toolbar-files", files_on)
                    .tooltip(tip("Browse the project's files"))
                    .child(icon(Icon::Files, 14., p.text_muted))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(Tab::Files, cx))),
            )
            .child(
                pill("toolbar-usage", usage_on)
                    .tooltip(tip("Usage and cost"))
                    .child(icon(Icon::Gauge, 14., p.text_muted))
                    .when(spent > 0.0, |b| b.child(div().text_color(p.text_muted).child(crate::usage::money(spent))))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(Tab::Usage, cx))),
            )
            .child(
                pill("toolbar-terminal", self.terminal_open)
                    .tooltip(tip("Terminal  Ctrl `"))
                    .child(icon(Icon::Terminal, 14., p.text_muted))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx))),
            )
    }

    fn fresh(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let project = self.workspace.read(cx).draft_project.clone();
        let heading: AnyElement = match &project {
            Some(path) => div()
                .flex()
                .gap(px(9.))
                .child("What's next for")
                .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{}?", project::name(path))))
                .into_any_element(),
            None => div().child(self.greeting).into_any_element(),
        };
        let starters: Vec<(Icon, &str, String)> = match &project {
            Some(_) => vec![
                (
                    Icon::Sparkles,
                    "Explain this project",
                    "Explain how this project is laid out and how its main parts fit together.".into(),
                ),
                (Icon::Search, "Find a bug", "Look for a bug in this project, explain it, and fix it.".into()),
                (Icon::Check, "Add tests", "Find code in this project that has no tests and add focused tests for it.".into()),
                (Icon::FileText, "Write AGENTS.md", INIT_PROMPT.into()),
            ],
            None => vec![
                (Icon::Bulb, "Explain a concept", "Explain how async and await work in Rust, with a small example.".into()),
                (Icon::Code, "Draft a function", "Write a function that parses a duration like 1h30m into seconds.".into()),
                (Icon::Chat, "Review an idea", "I have an idea for a feature. Ask me about it, then tell me what could go wrong.".into()),
            ],
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(32.))
            .pb(px(72.))
            .child(motion::rise("hero", div().size(px(84.)).child(self.hero.clone()), 10., cx))
            .child(motion::rise(
                "hero-heading",
                div().mt(px(18.)).text_size(px(30.)).line_height(px(38.)).text_color(p.text).child(heading),
                8.,
                cx,
            ))
            .child(
                div()
                    .id("fresh-sub")
                    .mt(px(6.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(7.))
                    .text_size(px(14.))
                    .text_color(p.text_muted)
                    .cursor_pointer()
                    .hover(move |style| style.bg(p.row_hover).text_color(p.text))
                    .child(if project.is_some() { "or switch to a quick chat" } else { "Pick a project to work in" })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.workspace.read(cx).draft_project.is_some() {
                            this.new_thread(None, window, cx);
                        } else {
                            this.menu = Some(Menu::Project);
                            cx.notify();
                        }
                    })),
            )
            .child(div().mt(px(26.)).w_full().flex().justify_center().child(self.composer.clone()))
            .child(div().mt(px(16.)).flex().flex_wrap().justify_center().gap(px(8.)).children(starters.into_iter().enumerate().map(
                |(index, (name, label, prompt))| {
                    motion::rise(
                        ("starter-rise", index),
                        div()
                            .id(("starter", index))
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .h(px(32.))
                            .px(px(12.))
                            .rounded_full()
                            .border_1()
                            .border_color(p.border_soft)
                            .bg(p.fill)
                            .text_size(px(13.))
                            .text_color(p.text_muted)
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.row_hover).text_color(p.text).border_color(p.border))
                            .child(icon(name, 14., p.text_faint))
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let prompt = Prompt { text: prompt.clone(), attachments: Vec::new() };
                                this.workspace.update(cx, |w, cx| w.send(prompt, cx));
                            })),
                        6.,
                        cx,
                    )
                },
            )))
            .into_any_element()
    }

    fn thread_menu(&self, id: u64, position: Point<Pixels>, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let thread = self.workspace.read(cx).threads.iter().find(|t| t.id == id);
        let project_path = thread.and_then(|t| t.project.clone());
        let isolated = thread.is_some_and(|t| t.worktree.is_some());
        let confirming = self.confirm_delete == Some(id);
        let menu = menu_panel(p)
            .w(px(240.))
            .child(
                menu_item("ctx-rename", Some(Icon::Pencil), "Rename", None, false, p)
                    .on_click(cx.listener(move |this, _, window, cx| this.start_rename(id, window, cx))),
            )
            .when_some(project_path, |menu, path| {
                let next = path.clone();
                menu.child(
                    menu_item("ctx-new", Some(Icon::Compose), "Start another thread here", None, false, p)
                        .on_click(cx.listener(move |this, _, window, cx| this.new_thread(Some(next.clone()), window, cx))),
                )
                .child(
                    menu_item("ctx-folder", Some(Icon::FolderOpen), "Show project in File Explorer", None, false, p).on_click(cx.listener(
                        move |this, _, _, cx| {
                            project::open_folder(&path);
                            this.close_menu(cx);
                        },
                    )),
                )
            })
            .when(isolated, |menu| {
                menu.child(
                    menu_item("ctx-unisolate", Some(Icon::Isolated), "Remove the isolated copy", None, false, p)
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_isolated(id, cx))),
                )
            })
            .child(separator(p))
            .child(
                menu_item("ctx-delete", Some(Icon::Trash), if confirming { "Click again to delete" } else { "Delete" }, None, false, p)
                    .text_color(p.danger)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.confirm_delete == Some(id) {
                            this.workspace.update(cx, |w, cx| w.delete(id, cx));
                            this.close_menu(cx);
                        } else {
                            this.confirm_delete = Some(id);
                            cx.notify();
                        }
                    })),
            );
        div()
            .child(
                deferred(
                    anchored().position(point(px(0.), px(0.))).child(
                        div()
                            .id("shell-backdrop")
                            .w(px(10000.))
                            .h(px(10000.))
                            .occlude()
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_menu(cx)))
                            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.close_menu(cx))),
                    ),
                )
                .with_priority(1),
            )
            .child(
                deferred(anchored().position(position).snap_to_window().child(motion::pop("thread-menu", menu, false, cx)))
                    .with_priority(2),
            )
    }

    fn launcher_view(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let launcher = self.launcher.as_ref()?;
        let p = palette(cx);
        let entries = self.entries(cx);
        let highlighted = launcher.highlighted.min(entries.len().saturating_sub(1));
        let list = div().id("launcher-list").max_h(px(440.)).overflow_y_scroll().flex().flex_col().gap(px(2.)).p(px(6.)).children(
            entries.into_iter().enumerate().map(|(index, entry)| {
                let on = index == highlighted;
                let action = entry.action.clone();
                div()
                    .id(("launcher-entry", index))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(9.))
                    .cursor_pointer()
                    .when(on, |row| row.bg(p.accent_soft))
                    .hover(move |style| style.bg(if on { p.accent_soft } else { p.row_hover }))
                    .child(icon(entry.icon, 16., if on { p.accent } else { p.text_muted }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.5)).text_color(p.text).child(entry.label))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(12.))
                                    .text_color(p.text_faint)
                                    .child(entry.detail),
                            ),
                    )
                    .when_some(entry.keys, |row, keys| row.child(kbd(keys, p)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_launcher(window, cx);
                        this.perform(action.clone(), window, cx);
                    }))
            }),
        );
        let panel = div()
            .w(px(580.))
            .rounded(px(16.))
            .bg(p.menu_bg)
            .border_1()
            .border_color(p.border)
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .h(px(50.))
                    .border_b_1()
                    .border_color(p.border_soft)
                    .text_size(px(14.5))
                    .child(icon(Icon::Search, 16., p.text_faint))
                    .child(div().flex_1().child(Input::new(&launcher.input).appearance(false))),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(p.border_soft)
                    .text_size(px(11.5))
                    .text_color(p.text_faint)
                    .child("↑↓ to move")
                    .child("Enter to run")
                    .child("Esc to close"),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    div()
                        .id("launcher-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(gpui_kit::black().opacity(if p.dark { 0.45 } else { 0.2 }))
                        .occlude()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_launcher(window, cx))),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(90.))
                        .left_0()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(motion::pop("launcher", panel, false, cx)),
                ),
        )
    }

    fn toast_view(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let toast = self.toast.clone()?;
        let p = palette(cx);
        let (mark, tone) = match toast.kind {
            ToastKind::Done => (Icon::CheckCircle, p.success),
            ToastKind::Problem => (Icon::CircleAlert, p.danger),
        };
        let card = div()
            .w(px(360.))
            .flex()
            .gap(px(10.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(14.))
            .bg(p.menu_bg)
            .border_1()
            .border_color(p.border)
            .shadow_lg()
            .child(div().pt(px(1.)).child(icon(mark, 16., tone)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(toast.title))
                    .when(!toast.detail.is_empty(), |col| {
                        col.child(div().text_size(px(12.5)).text_color(p.text_faint).child(toast.detail))
                    }),
            );
        Some(div().absolute().bottom(px(22.)).right(px(22.)).child(motion::rise(("toast", toast.serial), card, 14., cx)))
    }
}

const SECTIONS: [&str; 4] = ["In motion", "Past day", "Past week", "Older"];

fn section(live: bool, updated: u64) -> usize {
    if live {
        return 0;
    }
    match now_ms().saturating_sub(updated) / 3_600_000 {
        0..24 => 1,
        24..168 => 2,
        _ => 3,
    }
}

fn stat(name: Icon, text: String, color: Hsla) -> Div {
    div().flex().items_center().gap(px(3.)).text_color(color).child(icon(name, 12., color)).child(text)
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let has_thread = self.workspace.read(cx).active.is_some();
        let sidebar_open = Config::get(cx).appearance.sidebar && !self.in_settings;
        let spring = SpringAnimation::new(SpringConfig::new(420., 42., 1.));
        let left = div()
            .w(px(SIDEBAR - 8.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(6.))
            .child(
                icon_button("toggle-sidebar", Icon::Panel, "Sidebar  Ctrl B", p)
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            )
            .child(div().size(px(18.)).child(self.brand.clone()))
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(APP_NAME));
        let mut bar = div().flex().items_center().h_full().flex_1().min_w_0().pr(px(8.)).child(left);
        if self.in_settings {
            bar = bar.child(div().flex_1());
        } else {
            let toolbar = self.toolbar(window, cx);
            bar = bar
                .child(div().flex().flex_1().w(px(0.)).min_w_0().overflow_hidden().child(self.breadcrumb(window, cx)))
                .child(toolbar.flex_shrink_0());
        }
        let body: AnyElement = if self.in_settings {
            self.settings.clone().cached(StyleRefinement::default().size_full()).into_any_element()
        } else {
            let center: AnyElement = if has_thread {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(div().flex_1().min_h_0().child(self.thread_view.clone().cached(StyleRefinement::default().size_full())))
                    .child(div().flex_shrink_0().flex().justify_center().px(px(28.)).pb(px(16.)).child(self.composer.clone()))
                    .into_any_element()
            } else {
                self.fresh(cx)
            };
            let panel_width = px(((window.viewport_size().width.as_f32() - SIDEBAR) * 0.44).clamp(380., 660.));
            let sidebar = self.sidebar(cx);
            let terminal_open = self.terminal_open;
            let panel_open = self.panel_open;
            div()
                .size_full()
                .flex()
                .child(div().h_full().flex_shrink_0().overflow_hidden().child(sidebar).with_spring(
                    "sidebar-width",
                    spring.clone().to(if sidebar_open { px(SIDEBAR) } else { px(0.) }),
                    |el, width| el.w(width),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .bg(p.bg)
                        .when(sidebar_open, |main| main.rounded_tl(px(12.)).border_l_1())
                        .border_t_1()
                        .border_color(p.border_soft)
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .flex_col()
                                .child(div().relative().flex_1().min_h_0().child(center))
                                .when(terminal_open, |col| {
                                    col.child(motion::rise(
                                        "terminal-drawer",
                                        div()
                                            .h(px(280.))
                                            .flex_shrink_0()
                                            .border_t_1()
                                            .border_color(p.border_soft)
                                            .child(self.terminal.clone().cached(StyleRefinement::default().size_full())),
                                        24.,
                                        cx,
                                    ))
                                }),
                        )
                        .when(panel_open, |main| {
                            main.child(motion::rise(
                                "side-panel",
                                div()
                                    .w(panel_width)
                                    .flex_shrink_0()
                                    .h_full()
                                    .border_l_1()
                                    .border_color(p.border_soft)
                                    .child(self.panel.clone().cached(StyleRefinement::default().size_full())),
                                0.,
                                cx,
                            ))
                        }),
                )
                .into_any_element()
        };
        let thread_menu = match self.menu {
            Some(Menu::Thread(id, position)) => Some(self.thread_menu(id, position, cx)),
            _ => None,
        };
        let toast = self.toast_view(cx);
        let launcher = self.launcher_view(cx);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(p.sidebar)
            .font_family(FONT)
            .text_color(p.text)
            .child(TitleBar::new().child(bar))
            .child(div().flex_1().min_h_0().child(body))
            .children(thread_menu)
            .children(self.add_project.clone())
            .children(self.commit.clone())
            .children(toast)
            .children(launcher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_fall_into_sections_by_age() {
        let hours_ago = |h: u64| now_ms() - h * 3_600_000;
        assert_eq!(section(true, hours_ago(500)), 0);
        assert_eq!(section(false, hours_ago(2)), 1);
        assert_eq!(section(false, hours_ago(30)), 2);
        assert_eq!(section(false, hours_ago(200)), 3);
    }
}