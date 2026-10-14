use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, TitleBar};
use gpui_kit::{
    Anchor, AnyElement, Context, Div, Entity, FontWeight, Hsla, IntoElement, KeystrokeEvent, MouseButton, MouseDownEvent, ParentElement,
    Pixels, Point, Render, SharedString, Stateful, StyleRefinement, Styled, Subscription, Task, Window, anchored, deferred, div, point,
    prelude::*, px,
};
use quill_agent::Approval;
use quill_weave::State;

use crate::add_project::{AddProject, AddProjectEvent};
use crate::backend::Backend;
use crate::commit_dialog::{CommitDialog, CommitEvent};
use crate::composer::{ApprovalCard, Composer, ComposerEvent, Strip};
use crate::config::Config;
use crate::git::{self, Snapshot};
use crate::github::{self, PullRequest, PullState};
use crate::icons::Icon;
use crate::mentions::Command;
use crate::palette::{self, FONT, Palette, palette};
use crate::panel::{ClosePanel, Panel, Tab};
use crate::project;
use crate::settings::{Section, Settings, SettingsEvent};
use crate::terminal::{CloseTerminal, Terminal};
use crate::thread::{Item, Thread, ago, now_ms};
use crate::thread_view::{OpenDiff, ThreadView};
use crate::ui::{MONO, badge, icon, menu_item, menu_label, menu_panel, popup, separator};
use crate::version::{APP_NAME, VERSION};
use crate::weave::WeaveMark;
use crate::workspace::Workspace;

const GREETINGS: [&str; 4] = ["Where should we start?", "What are we building today?", "Ready when you are.", "What needs fixing?"];

#[derive(Clone, Copy, PartialEq)]
enum Menu {
    Project,
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

pub struct Shell {
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
    toast: Option<(SharedString, SharedString)>,
    toast_timer: Option<Task<()>>,
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
        let panel = cx.new(|cx| Panel::new(window, cx));
        let terminal = cx.new(|cx| Terminal::new(window, cx));
        let settings = cx.new(|cx| Settings::new(backend.clone(), window, cx));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a thread"));
        let weak = cx.entity().downgrade();
        let subscriptions = vec![
            cx.observe_window_appearance(window, |_, window, cx| palette::apply(window.appearance(), Some(window), cx)),
            cx.observe(&workspace, |this, _, cx| this.sync(cx)),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.repos.clear();
                    this.pulls.clear();
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
            cx.subscribe(&thread_view, |this, _, OpenDiff { thread, turn }, cx| this.open_diff(*thread, Some(*turn), cx)),
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
        composer.update(cx, |composer, cx| composer.focus(window, cx));
        let mut shell = Shell {
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
            toast_timer: None,
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

    fn intercept(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != "escape" {
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
        let (busy, approval) = match workspace.active {
            Some(id) => (
                workspace.running(id),
                workspace.pending(id).map(|pending| match &pending.approval {
                    Approval::FileChange(diff) => {
                        ApprovalCard { title: "Wants to change a file", target: diff.path.clone(), stats: Some((diff.added, diff.removed)) }
                    }
                    Approval::Command(command) => ApprovalCard { title: "Wants to run a command", target: command.clone(), stats: None },
                    Approval::Commit(message) => ApprovalCard { title: "Wants to commit", target: message.clone(), stats: None },
                    Approval::PullRequest(title) => {
                        ApprovalCard { title: "Wants to open a pull request", target: title.clone(), stats: None }
                    }
                }),
            ),
            None => (false, None),
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
        self.composer.update(cx, |composer, cx| composer.set_context(busy, approval, strip, context_used, cx));
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
            _ => {}
        }
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
            self.show_toast("Pick a project first".into(), "Commits need a project that uses Git.".into(), cx);
            return;
        };
        if !self.repo(&root).repo {
            self.show_toast("Not a Git repository".into(), root.display().to_string(), cx);
            return;
        }
        self.menu = None;
        let (url, model) = (Backend::url(cx), Backend::selected_id(cx));
        let dialog = cx.new(|cx| CommitDialog::new(root, url, model, window, cx));
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

    fn show_toast(&mut self, title: String, detail: String, cx: &mut Context<Self>) {
        self.toast = Some((title.into(), detail.into()));
        self.toast_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            _ = this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            });
        }));
        cx.notify();
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
            Err(error) => self.show_toast("Could not remove the copy".into(), error, cx),
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
        let pending = workspace.pending(id).is_some();
        let project_name = thread.project.as_ref().map(|path| project::name(path));
        let elapsed = thread.turns.last().map(|t| t.elapsed_secs()).unwrap_or(0);
        let motion = match thread.turns.last().and_then(|t| t.items.last()) {
            _ if !running || pending => State::Idle,
            Some(Item::Text(_)) => State::Generating,
            _ => State::Thinking,
        };
        let model = thread.turns.iter().rev().find_map(|turn| turn.model.clone()).or_else(|| thread.model.clone());
        let weave = model.map(|model| {
            let fresh = self.marks.get(&id).is_none_or(|(seeded, _)| *seeded != model);
            if fresh {
                let mark = cx.new(|_| WeaveMark::new(State::Idle).seeded(&model).opacity(0.7));
                self.marks.insert(id, (model, mark));
            }
            let mark = self.marks[&id].1.clone();
            mark.update(cx, |mark, cx| mark.set_state(motion, cx));
            mark
        });
        let status = if pending {
            div().px(px(6.)).rounded(px(5.)).bg(p.warning.opacity(0.14)).text_color(p.warning).child("Needs you")
        } else if running {
            div().flex().items_center().gap(px(4.)).text_color(p.link).child(icon(Icon::Loader, 12., p.link)).child(format!("{elapsed}s"))
        } else {
            div().text_color(p.text_faint).child(ago(thread.updated))
        };
        let mark = match &project_name {
            Some(name) => badge(name, 26.),
            None => div().size(px(26.)).flex_shrink_0().flex().items_center().justify_center().rounded(px(7.)).bg(p.fill).child(icon(
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
            .when_some(weave, |row, weave| row.child(div().size(px(16.)).flex_shrink_0().child(weave)));
        div()
            .id(SharedString::from(format!("thread-{id}")))
            .flex()
            .items_start()
            .gap(px(10.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(active, |row| row.bg(p.row_active))
            .hover(move |style| style.bg(p.row_hover))
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
            let live = workspace.running(thread.id) || workspace.pending(thread.id).is_some();
            groups[section(live, thread.updated)].push(self.thread_row(thread, cx));
        }
        let sections = SECTIONS.iter().zip(groups).filter(|(_, rows)| !rows.is_empty()).map(|(label, rows)| {
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .px(px(10.))
                        .pt(px(10.))
                        .pb(px(4.))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text_faint)
                        .child(*label),
                )
                .children(rows)
        });
        div()
            .w(px(272.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .px(px(8.))
            .pb(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pb(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .h(px(34.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(9.))
                            .bg(p.fill)
                            .text_size(px(13.5))
                            .child(Input::new(&self.search).appearance(false).prefix(icon(Icon::Search, 14., p.text_faint))),
                    )
                    .child(
                        icon_button("add-project", Icon::FolderPlus, p)
                            .on_click(cx.listener(|this, _, window, cx| this.show_add_project(window, cx))),
                    )
                    .child(icon_button("new-thread", Icon::Compose, p).on_click(cx.listener(|this, _, window, cx| {
                        let project = this.workspace.read(cx).project();
                        this.new_thread(project, window, cx);
                    }))),
            )
            .child(div().id("threads").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().children(sections).when(empty, |list| {
                list.child(
                    div().pt(px(24.)).flex().justify_center().text_size(px(13.5)).text_color(p.text_faint).child(if filter.is_empty() {
                        "Your threads will show up here"
                    } else {
                        "Nothing matches that"
                    }),
                )
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .pt(px(6.))
                    .child(
                        icon_button("open-settings", Icon::Gear, p)
                            .on_click(cx.listener(|this, _, window, cx| this.open_settings(Section::General, window, cx))),
                    )
                    .child(
                        icon_button("open-models", Icon::Cube, p)
                            .on_click(cx.listener(|this, _, window, cx| this.open_settings(Section::Models, window, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("open-version")
                            .px(px(8.))
                            .h(px(28.))
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
            let projects = Config::get(cx).projects.clone();
            let menu = menu_panel(p)
                .w(px(320.))
                .child(menu_label("Your projects", p))
                .children(projects.into_iter().enumerate().map(|(i, path)| {
                    let chosen = current.as_ref() == Some(&path);
                    menu_item(
                        ("pick-project", i),
                        Some(Icon::Folder),
                        project::name(&path),
                        Some(path.display().to_string().into()),
                        chosen,
                        p,
                    )
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
            .gap(px(8.))
            .min_w_0()
            .text_size(px(14.))
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
                            .rounded(px(7.))
                            .text_color(p.text_muted)
                            .when(!locked, |b| b.cursor_pointer().hover(move |style| style.bg(p.row_hover).text_color(p.text)))
                            .child(match &project_name {
                                Some(name) => badge(name, 16.).into_any_element(),
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
            .child(div().text_color(p.text_faint).child("/"))
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

    fn env_card(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let p = palette(cx);
        let path = self.workspace.read(cx).workdir()?;
        let info = self.repo(&path);
        let changes = self.workspace.read(cx).active_thread().and_then(|t| {
            t.turns.iter().rev().find(|turn| turn.diffs().next().is_some()).map(|turn| {
                let c = turn.changes();
                (c.iter().map(|x| x.added).sum::<usize>(), c.iter().map(|x| x.removed).sum::<usize>())
            })
        });
        let editor = self.editor;
        let open_path = path.clone();
        let reveal_path = path.clone();
        Some(
            div()
                .absolute()
                .top(px(14.))
                .right(px(14.))
                .w(px(250.))
                .p(px(5.))
                .rounded(px(14.))
                .bg(p.menu_bg)
                .border_1()
                .border_color(p.border_soft)
                .flex()
                .flex_col()
                .gap(px(1.))
                .when(editor, |card| {
                    card.child(
                        menu_item("open-vscode", Some(Icon::Code), "Open in VS Code", None, false, p)
                            .on_click(move |_, _, _| project::open_in_editor(&open_path)),
                    )
                })
                .child(
                    menu_item("open-explorer", Some(Icon::FolderOpen), "Show in File Explorer", None, false, p)
                        .on_click(move |_, _, _| project::open_folder(&reveal_path)),
                )
                .child(separator(p))
                .child(match info.branch {
                    Some(branch) => menu_item("branch", Some(Icon::Branch), branch, None, false, p),
                    None => {
                        menu_item("no-git", Some(Icon::Branch), if info.repo { "No commits yet" } else { "Not using Git" }, None, false, p)
                    }
                })
                .child(
                    menu_item("changes", Some(Icon::FileDiff), "Edits in this thread", None, false, p)
                        .when_some(changes, |row, (added, removed)| {
                            row.child(div().text_size(px(12.)).text_color(p.add).child(format!("+{added}")))
                                .child(div().text_size(px(12.)).text_color(p.del).child(format!("-{removed}")))
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(Tab::Diff, cx))),
                )
                .when(info.repo, |card| {
                    card.child(
                        menu_item("commit-work", Some(Icon::Commit), "Commit your work", None, false, p)
                            .on_click(cx.listener(|this, _, window, cx| this.open_commit(window, cx))),
                    )
                }),
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
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(32.))
            .pb(px(72.))
            .child(div().size(px(76.)).child(self.hero.clone()))
            .child(div().mt(px(18.)).text_size(px(30.)).line_height(px(38.)).text_color(p.text).child(heading))
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
            .child(deferred(anchored().position(position).snap_to_window().child(menu)).with_priority(2))
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

fn icon_button(id: &'static str, name: Icon, p: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(move |style| style.bg(p.row_hover))
        .child(icon(name, 16., p.text_muted))
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let has_thread = self.workspace.read(cx).active.is_some();
        let has_repo = self.workspace.read(cx).workdir().is_some_and(|root| self.repo(&root).repo);
        let panel_open = self.panel_open;
        let panel_tab = self.panel.read(cx).tab;
        let mut bar = div().flex().items_center().h_full().flex_1().pr(px(8.)).child(
            div()
                .w(px(272.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(6.))
                .child(div().size(px(20.)).child(self.brand.clone()))
                .child(div().text_size(px(14.5)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(APP_NAME)),
        );
        if self.in_settings {
            bar = bar.child(div().text_size(px(14.)).text_color(p.text_muted).child("Settings"));
        } else {
            bar = bar.child(self.breadcrumb(window, cx)).child(div().flex_1());
            if has_repo {
                bar = bar.child(
                    icon_button("title-commit", Icon::Commit, p)
                        .occlude()
                        .when(self.commit.is_some(), |b| b.bg(p.row_active))
                        .on_click(cx.listener(|this, _, window, cx| this.open_commit(window, cx))),
                );
            }
            bar = bar.child(
                icon_button("toggle-terminal", Icon::Terminal, p)
                    .occlude()
                    .when(self.terminal_open, |b| b.bg(p.row_active))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx))),
            );
            for (id, name, tab) in [("toggle-files", Icon::Files, Tab::Files), ("toggle-diff", Icon::FileDiff, Tab::Diff)] {
                let on = panel_open && panel_tab == tab;
                bar = bar.child(
                    icon_button(id, name, p)
                        .occlude()
                        .when(on, |b| b.bg(p.row_active))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_panel(tab, cx))),
                );
            }
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
            let wide = window.viewport_size().width >= px(1700.);
            let env = if panel_open || (has_thread && !wide) { None } else { self.env_card(cx) };
            let panel_width = px(((window.viewport_size().width.as_f32() - 272.) * 0.44).clamp(380., 640.));
            let sidebar = self.sidebar(cx);
            let terminal_open = self.terminal_open;
            div()
                .size_full()
                .flex()
                .child(sidebar)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .bg(p.bg)
                        .rounded_tl(px(10.))
                        .border_t_1()
                        .border_l_1()
                        .border_color(p.border_soft)
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .flex_col()
                                .child(div().relative().flex_1().min_h_0().child(center).children(env))
                                .when(terminal_open, |col| {
                                    col.child(
                                        div()
                                            .h(px(280.))
                                            .flex_shrink_0()
                                            .border_t_1()
                                            .border_color(p.border_soft)
                                            .child(self.terminal.clone().cached(StyleRefinement::default().size_full())),
                                    )
                                }),
                        )
                        .when(panel_open, |main| {
                            main.child(
                                div()
                                    .w(panel_width)
                                    .flex_shrink_0()
                                    .h_full()
                                    .border_l_1()
                                    .border_color(p.border_soft)
                                    .child(self.panel.clone().cached(StyleRefinement::default().size_full())),
                            )
                        }),
                )
                .into_any_element()
        };
        let thread_menu = match self.menu {
            Some(Menu::Thread(id, position)) => Some(self.thread_menu(id, position, cx)),
            _ => None,
        };
        let toast = self.toast.clone().map(|(title, detail)| {
            div()
                .absolute()
                .top(px(48.))
                .right(px(18.))
                .w(px(360.))
                .flex()
                .gap(px(10.))
                .px(px(14.))
                .py(px(12.))
                .rounded(px(12.))
                .bg(p.menu_bg)
                .border_1()
                .border_color(p.border)
                .shadow_lg()
                .child(icon(Icon::CheckCircle, 16., p.success))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(title))
                        .when(!detail.is_empty(), |col| col.child(div().text_size(px(12.5)).text_color(p.text_faint).child(detail))),
                )
        });
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