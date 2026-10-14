use std::path::PathBuf;

use gpui_kit::component::input::{Input, InputEvent, InputState, Position, Textarea, TextareaState};
use gpui_kit::{
    Anchor, App, Context, Div, Entity, EventEmitter, ExternalPaths, Focusable, FontWeight, IntoElement, KeystrokeEvent, ObjectFit,
    ParentElement, PathPromptOptions, Render, SharedString, Stateful, Styled, Subscription, Window, div, img, prelude::*, px, svg,
};
use quill_agent::{Access, Decision};
use quill_llama::{Model, Status, Thinking, context_label};
use quill_weave::State;

use crate::attachments::Attachment;
use crate::backend::{Backend, Connection};
use crate::config::Config;
use crate::context_ring;
use crate::git::{self, Branches};
use crate::icons::Icon;
use crate::mentions::{self, Command, Trigger};
use crate::palette::{Palette, palette};
use crate::project;
use crate::ui::{MONO, icon, menu_item, menu_label, menu_panel, popup, separator};
use crate::weave::{WeaveMark, model_mark};
use crate::workspace::Prompt;

pub enum ComposerEvent {
    Submit(Prompt),
    Stop,
    Decide(Decision),
    OpenModelSettings,
    Command(Command),
    SetIsolated(bool),
    SwitchProject(PathBuf),
    AddProject,
    RepoChanged,
}

#[derive(Clone)]
pub struct ApprovalCard {
    pub title: &'static str,
    pub target: String,
    pub stats: Option<(usize, usize)>,
}

#[derive(Clone)]
pub struct Strip {
    pub root: PathBuf,
    pub project: PathBuf,
    pub name: String,
    pub branch: Option<String>,
    pub isolated: bool,
    pub can_isolate: bool,
    pub locked: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Model,
    Thinking,
    Access,
    Workspace,
    Branch,
    ApprovalMore,
}

#[derive(Clone, PartialEq)]
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

pub struct Composer {
    input: Entity<TextareaState>,
    search: Entity<InputState>,
    branch_input: Entity<InputState>,
    branches: Branches,
    deleting: Option<String>,
    branch_error: Option<String>,
    backend: Entity<Backend>,
    mark: Entity<WeaveMark>,
    menu: Option<Menu>,
    busy: bool,
    approval: Option<ApprovalCard>,
    strip: Option<Strip>,
    attachments: Vec<Attachment>,
    context_used: Option<u64>,
    suggestions: Option<Suggestions>,
    highlighted: usize,
    files: Option<(PathBuf, Vec<String>)>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Describe a task, type / for commands or @ to bring in a file")
                .auto_grow(2, 14)
                .submit_on_enter(true)
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a model"));
        let branch_input = cx.new(|cx| InputState::new(window, cx).placeholder("Find or create a branch"));
        let weak = cx.entity().downgrade();
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => this.submit(window, cx),
                InputEvent::Change => this.refresh_suggestions(cx),
                _ => {}
            }),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            cx.subscribe(&branch_input, |this, _, event: &InputEvent, cx| match event {
                InputEvent::PressEnter { .. } => this.branch_enter(cx),
                InputEvent::Change => {
                    this.deleting = None;
                    this.branch_error = None;
                    cx.notify();
                }
                _ => {}
            }),
            cx.observe(&backend, |_, _, cx| cx.notify()),
            cx.intercept_keystrokes(move |event, window, cx| {
                if let Some(this) = weak.upgrade() {
                    this.update(cx, |this, cx| this.intercept(event, window, cx));
                }
            }),
        ];
        Composer {
            input,
            search,
            branch_input,
            branches: Branches::default(),
            deleting: None,
            branch_error: None,
            backend,
            mark: cx.new(|_| WeaveMark::new(State::Idle)),
            menu: None,
            busy: false,
            approval: None,
            strip: None,
            attachments: Vec::new(),
            context_used: None,
            suggestions: None,
            highlighted: 0,
            files: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    pub fn set_context(
        &mut self,
        busy: bool,
        approval: Option<ApprovalCard>,
        strip: Option<Strip>,
        context_used: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        self.busy = busy;
        self.context_used = context_used;
        self.approval = approval;
        if self.strip.as_ref().map(|s| &s.root) != strip.as_ref().map(|s| &s.root) {
            self.files = None;
        }
        self.strip = strip;
        if self.approval.is_none() && self.menu == Some(Menu::ApprovalMore) {
            self.menu = None;
        }
        cx.notify();
    }

    fn intercept(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;
        if modifiers.control
            && !modifiers.alt
            && !modifiers.shift
            && let Some(index) = key.parse::<usize>().ok().filter(|n| (1..=9).contains(n))
        {
            let pinned = Config::get(cx).pinned_models.clone();
            if let Some(id) = pinned.get(index - 1).cloned() {
                self.backend.update(cx, |backend, cx| backend.select(id, cx));
                cx.stop_propagation();
            }
            return;
        }
        if key == "escape" {
            if self.menu.is_some() {
                self.close(cx);
                cx.stop_propagation();
            } else if self.suggestions.is_some() {
                self.suggestions = None;
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }
        let Some(count) = self.suggestions.as_ref().map(Suggestions::len) else { return };
        if !self.input.read(cx).focus_handle(cx).is_focused(window) || count == 0 {
            return;
        }
        match key {
            "up" => self.highlighted = self.highlighted.checked_sub(1).unwrap_or(count - 1),
            "down" => self.highlighted = (self.highlighted + 1) % count,
            "tab" => self.accept(self.highlighted, window, cx),
            "enter" if !modifiers.shift => self.accept(self.highlighted, window, cx),
            _ => return,
        }
        cx.notify();
        cx.stop_propagation();
    }

    fn refresh_suggestions(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        let next = match mentions::trigger(&text) {
            Some(Trigger::Slash(query)) => {
                let commands = Command::matching(&query);
                (!commands.is_empty()).then_some(Suggestions::Commands(commands))
            }
            Some(Trigger::Mention { start, query }) => self.strip.clone().and_then(|strip| {
                if self.files.as_ref().is_none_or(|(root, _)| *root != strip.root) {
                    self.files = Some((strip.root.clone(), mentions::file_index(&strip.root)));
                }
                let paths = self.files.as_ref().map(|(_, files)| mentions::rank(files, &query, 8)).unwrap_or_default();
                (!paths.is_empty()).then_some(Suggestions::Files { start, paths })
            }),
            None => None,
        };
        if next != self.suggestions {
            self.highlighted = 0;
        }
        self.suggestions = next;
        cx.notify();
    }

    fn accept(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(suggestions) = self.suggestions.take() else { return };
        match suggestions {
            Suggestions::Commands(commands) => {
                let Some(command) = commands.get(index).copied() else { return };
                self.input.update(cx, |input, cx| input.set_value("", window, cx));
                self.run_command(command, window, cx);
            }
            Suggestions::Files { start, paths } => {
                let Some(path) = paths.get(index) else { return };
                let text = self.input.read(cx).value().to_string();
                let next = format!("{}@{path} ", &text[..start.min(text.len())]);
                let line = next.matches('\n').count() as u32;
                let column = next.rsplit('\n').next().unwrap_or_default().encode_utf16().count() as u32;
                self.input.update(cx, |input, cx| {
                    input.set_value(next, window, cx);
                    input.set_cursor_position(Position::new(line, column), window, cx);
                });
            }
        }
        cx.notify();
    }

    fn run_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::Model => self.open(Menu::Model, window, cx),
            Command::Mode => self.open(Menu::Access, window, cx),
            Command::Reasoning => {
                let on = !Config::get(cx).thinking;
                self.backend.update(cx, |backend, cx| backend.set_thinking(on, cx));
            }
            Command::Init if !self.busy => {
                cx.emit(ComposerEvent::Submit(Prompt { text: mentions::INIT_PROMPT.into(), attachments: Vec::new() }));
            }
            other => cx.emit(ComposerEvent::Command(other)),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.suggestions.is_some() {
            return;
        }
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        let text = if text.is_empty() { "Take a look at the attached files.".to_string() } else { text };
        self.input.update(cx, |input, cx| input.set_value("", window, cx));
        let attachments = std::mem::take(&mut self.attachments);
        cx.emit(ComposerEvent::Submit(Prompt { text, attachments }));
    }

    fn add_files(&mut self, paths: impl IntoIterator<Item = PathBuf>, cx: &mut Context<Self>) {
        for attachment in paths.into_iter().filter_map(Attachment::from_path) {
            if !self.attachments.iter().any(|a| a.path == attachment.path) {
                self.attachments.push(attachment);
            }
        }
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let picked =
            cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Attach".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                _ = this.update(cx, |this, cx| this.add_files(paths, cx));
            }
        })
        .detach();
    }

    fn open(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = Some(menu);
        if menu == Menu::Branch
            && let Some(strip) = &self.strip
        {
            self.branches = git::branches(&strip.root);
            self.deleting = None;
            self.branch_error = None;
            self.branch_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.focus(window, cx);
            });
        }
        if menu == Menu::Model {
            self.backend.update(cx, |backend, cx| backend.refresh(cx));
            self.search.update(cx, |search, cx| {
                search.set_value("", window, cx);
                search.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn toggle(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu == Some(menu) {
            self.close(cx);
        } else {
            self.open(menu, window, cx);
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    fn model_label(&self, cx: &App) -> (bool, SharedString) {
        let backend = self.backend.read(cx);
        if let Some(model) = backend.selected(cx) {
            return (true, model.id.clone().into());
        }
        match (&backend.connection, Backend::selected_id(cx)) {
            (Connection::Checking, Some(id)) => (true, id.into()),
            (Connection::Online, _) if !backend.models.is_empty() => (false, "Choose a model".into()),
            (Connection::Checking, None) => (false, "Connecting".into()),
            _ => (false, "No model connected".into()),
        }
    }

    fn model_row(&self, model: &Model, shortcut: Option<usize>, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = palette(cx);
        let id = model.id.clone();
        let pin_id = model.id.clone();
        let selected = Backend::selected_id(cx).as_deref() == Some(model.id.as_str());
        let pinned = Config::get(cx).pinned_models.contains(&model.id);
        let mut detail = vec![model.status.label().to_string()];
        if let Some(context) = model.context {
            detail.push(format!("{} context", context_label(context)));
        }
        if model.vision {
            detail.push("Vision".into());
        }
        div()
            .id(SharedString::from(format!("model-{id}")))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(selected, |row| row.bg(p.row_active))
            .hover(move |style| style.bg(p.row_hover))
            .child(
                div().relative().flex_shrink_0().child(model_mark(&model.id, 24., p.text)).child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .size(px(8.))
                        .rounded_full()
                        .border_1()
                        .border_color(p.menu_bg)
                        .bg(if model.status == Status::Loaded { p.success } else { p.text_faint.opacity(0.5) }),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(13.5)).text_color(p.text).child(model.id.clone()))
                    .child(div().text_size(px(12.)).text_color(p.text_faint).child(detail.join(" · "))),
            )
            .when_some(shortcut, |row, n| {
                row.child(
                    div()
                        .px(px(5.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(p.border_soft)
                        .text_size(px(11.))
                        .text_color(p.text_faint)
                        .child(format!("Ctrl+{n}")),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("pin-{pin_id}")))
                    .size(px(24.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .hover(move |style| style.bg(p.row_active))
                    .child(icon(Icon::Pin, 13., if pinned { p.accent } else { p.text_faint.opacity(0.6) }))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        let id = pin_id.clone();
                        Config::update(cx, |config| {
                            if config.pinned_models.contains(&id) {
                                config.pinned_models.retain(|m| *m != id);
                            } else {
                                config.pinned_models.push(id);
                            }
                        });
                        cx.notify();
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                this.backend.update(cx, |backend, cx| backend.select(id, cx));
                this.close(cx);
            }))
    }

    fn model_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let backend = self.backend.read(cx);
        let url = Backend::url(cx);
        let query = self.search.read(cx).value().trim().to_lowercase();
        let pinned_ids = Config::get(cx).pinned_models.clone();
        let models = backend.models.clone();
        let connection = backend.connection.clone();
        let mut menu = menu_panel(p).w(px(360.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .pt(px(6.))
                .pb(px(6.))
                .child(svg().path(Icon::LlamaCpp.path()).size(px(16.)).text_color(p.text))
                .child(div().flex_1().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child("llama.cpp"))
                .child(
                    div()
                        .id("refresh-models")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(24.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(move |style| style.bg(p.row_hover))
                        .child(icon(Icon::Refresh, 13., p.text_faint))
                        .on_click(cx.listener(|this, _, _, cx| this.backend.update(cx, |b, cx| b.refresh(cx)))),
                ),
        );
        menu = menu.child(search_box(&self.search, p));
        match &connection {
            Connection::Checking if models.is_empty() => menu = menu.child(note("Looking for models", p)),
            Connection::Offline(error) => {
                menu = menu
                    .child(note(format!("Can't reach llama.cpp at {url}"), p))
                    .child(div().px(px(10.)).pb(px(6.)).text_size(px(12.)).text_color(p.text_faint).child(error.clone()));
            }
            _ if models.is_empty() => menu = menu.child(note("This server has no models", p)),
            _ => {}
        }
        let mut body = div().id("model-list").max_h(px(380.)).overflow_y_scroll().flex().flex_col().gap(px(1.));
        if query.is_empty() {
            let pinned: Vec<&Model> = pinned_ids.iter().filter_map(|id| models.iter().find(|m| m.id == *id)).collect();
            if !pinned.is_empty() {
                body = body.child(menu_label("Pinned", p));
                for (n, model) in pinned.iter().enumerate() {
                    body = body.child(self.model_row(model, (n < 9).then_some(n + 1), cx));
                }
                body = body.child(menu_label("Everything on this server", p));
            }
            for model in models.iter().filter(|m| !pinned_ids.contains(&m.id)) {
                body = body.child(self.model_row(model, None, cx));
            }
        } else {
            let found: Vec<&Model> = models.iter().filter(|m| m.id.to_lowercase().contains(&query)).collect();
            if found.is_empty() {
                body = body.child(note("No model matches", p));
            }
            for model in found {
                body = body.child(self.model_row(model, None, cx));
            }
        }
        menu.child(body).child(separator(p)).child(
            menu_item("server-settings", Some(Icon::Server), url, None, false, p)
                .child(div().text_size(px(12.5)).text_color(p.text_muted).child("Change"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close(cx);
                    cx.emit(ComposerEvent::OpenModelSettings);
                })),
        )
    }

    fn thinking_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let on = Config::get(cx).thinking;
        menu_panel(p).w(px(240.)).child(menu_label("Reasoning", p)).children(
            [
                (true, "Think it through", "Reason before replying, slower but sharper"),
                (false, "Reply directly", "Skip the reasoning step"),
            ]
            .map(|(value, label, detail)| {
                menu_item(label, None, label, Some(detail.into()), on == value, p).on_click(cx.listener(move |this, _, _, cx| {
                    this.backend.update(cx, |b, cx| b.set_thinking(value, cx));
                    this.close(cx);
                }))
            }),
        )
    }

    fn access_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let current = Config::get(cx).access;
        menu_panel(p).w(px(360.)).child(menu_label("What the agent may do on its own", p)).children(Access::ALL.map(|access| {
            menu_item(access.label(), Some(access_icon(access)), access.label(), Some(access.description().into()), access == current, p)
                .on_click(cx.listener(move |this, _, _, cx| {
                    Config::update(cx, |config| config.access = access);
                    this.close(cx);
                }))
        }))
    }

    fn branch_query(&self, cx: &App) -> String {
        self.branch_input.read(cx).value().trim().to_string()
    }

    fn branch_matches(&self, query: &str) -> (Vec<String>, Vec<String>) {
        let query = query.to_lowercase();
        let keep = |names: &[String]| names.iter().filter(|name| name.to_lowercase().contains(&query)).cloned().collect();
        (keep(&self.branches.local), keep(&self.branches.remote))
    }

    fn branch_enter(&mut self, cx: &mut Context<Self>) {
        let query = self.branch_query(cx);
        if query.is_empty() {
            return;
        }
        let (local, remote) = self.branch_matches(&query);
        match local.into_iter().chain(remote).next() {
            Some(name) => self.change_branch(name, false, cx),
            None => self.change_branch(query, true, cx),
        }
    }

    fn change_branch(&mut self, name: String, create: bool, cx: &mut Context<Self>) {
        let Some(root) = self.strip.as_ref().map(|s| s.root.clone()) else { return };
        let result = if self.branches.current.as_deref() == Some(name.as_str()) {
            Ok(())
        } else if !create {
            git::switch_branch(&root, &name)
        } else if git::valid_branch(&name) {
            git::create_branch(&root, &name)
        } else {
            Err(format!("\"{name}\" is not a branch name Git accepts."))
        };
        match result {
            Ok(()) => {
                self.close(cx);
                cx.emit(ComposerEvent::RepoChanged);
            }
            Err(error) => {
                self.branch_error = Some(error);
                cx.notify();
            }
        }
    }

    fn remove_branch(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(root) = self.strip.as_ref().map(|s| s.root.clone()) else { return };
        match git::delete_branch(&root, &name) {
            Ok(()) => {
                self.branches = git::branches(&root);
                self.deleting = None;
                self.branch_error = None;
                cx.emit(ComposerEvent::RepoChanged);
            }
            Err(error) => self.branch_error = Some(error),
        }
        cx.notify();
    }

    fn branch_row(&self, name: &str, remote: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = palette(cx);
        let current = self.branches.current.as_deref() == Some(name);
        let confirming = self.deleting.as_deref() == Some(name);
        let unmerged = self.branches.unmerged.iter().any(|b| b == name);
        let base = self.branches.current.clone().unwrap_or_else(|| "this branch".into());
        let detail = if confirming && unmerged {
            Some(format!("Has commits that are not in {base}. Deleting loses them."))
        } else if confirming {
            Some(format!("Everything on it is already in {base}."))
        } else if remote {
            Some("Only on origin. Switching sets up a local copy.".to_string())
        } else {
            None
        };
        let (target, doomed) = (name.to_string(), name.to_string());
        div()
            .id(SharedString::from(format!("branch-{name}")))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(confirming, |row| row.bg(p.danger.opacity(0.08)))
            .hover(move |style| style.bg(p.row_hover))
            .child(icon(if remote { Icon::Globe } else { Icon::Branch }, 14., p.text_muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(MONO)
                            .text_size(px(13.))
                            .text_color(p.text)
                            .child(name.to_string()),
                    )
                    .when_some(detail, |col, detail| {
                        col.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(if confirming && unmerged { p.danger } else { p.text_faint })
                                .child(detail),
                        )
                    }),
            )
            .when(current, |row| row.child(icon(Icon::Check, 14., p.text)))
            .when(!remote && !current && !confirming, |row| {
                row.child(
                    div()
                        .id(SharedString::from(format!("delete-branch-{name}")))
                        .size(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .hover(move |style| style.bg(p.danger.opacity(0.12)))
                        .child(icon(Icon::Trash, 13., p.text_faint.opacity(0.7)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.deleting = Some(doomed.clone());
                            cx.notify();
                        })),
                )
            })
            .when(confirming, |row| {
                let doomed = name.to_string();
                row.child(
                    div()
                        .id(SharedString::from(format!("confirm-delete-{name}")))
                        .px(px(10.))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .rounded(px(7.))
                        .bg(p.danger)
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(gpui_kit::white())
                        .hover(|style| style.opacity(0.9))
                        .child("Delete")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.remove_branch(doomed.clone(), cx);
                        })),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.deleting.is_some() {
                    this.deleting = None;
                    cx.notify();
                } else {
                    this.change_branch(target.clone(), false, cx);
                }
            }))
    }

    fn branch_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let query = self.branch_query(cx);
        let (local, remote) = self.branch_matches(&query);
        let exact = local.iter().chain(&remote).any(|name| *name == query);
        let local_rows: Vec<Stateful<Div>> = local.iter().map(|name| self.branch_row(name, false, cx)).collect();
        let remote_rows: Vec<Stateful<Div>> = remote.iter().map(|name| self.branch_row(name, true, cx)).collect();
        let nothing = local_rows.is_empty() && remote_rows.is_empty() && query.is_empty();
        menu_panel(p)
            .w(px(340.))
            .child(div().pt(px(4.)).child(search_box(&self.branch_input, p)))
            .child(
                div()
                    .id("branch-list")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .when(!local_rows.is_empty(), |list| list.child(menu_label("Branches", p)).children(local_rows))
                    .when(!remote_rows.is_empty(), |list| list.child(menu_label("Only on origin", p)).children(remote_rows))
                    .when(nothing, |list| list.child(note("No branches yet. Type a name to create one.", p))),
            )
            .when(!query.is_empty() && !exact, |panel| {
                let name = query.clone();
                panel.child(separator(p)).child(
                    menu_item(
                        "create-branch",
                        Some(Icon::Plus),
                        format!("Create {query}"),
                        Some("A new branch from where you are now".into()),
                        false,
                        p,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.change_branch(name.clone(), true, cx))),
                )
            })
            .when_some(self.branch_error.clone(), |panel, error| {
                panel.child(
                    div()
                        .mx(px(6.))
                        .my(px(4.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .bg(p.danger.opacity(0.1))
                        .text_size(px(12.5))
                        .text_color(p.danger)
                        .child(error),
                )
            })
    }

    fn workspace_menu(&self, strip: &Strip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let isolated = strip.isolated;
        let choosing = strip.can_isolate && !strip.locked;
        let projects = Config::get(cx).projects.clone();
        let current = strip.project.clone();
        let locked = strip.locked;
        let mut menu = menu_panel(p).w(px(360.));
        if choosing {
            menu = menu
                .child(menu_label("Where the agent works", p))
                .child(
                    menu_item(
                        "work-in-project",
                        Some(Icon::Folder),
                        "Project folder",
                        Some(format!("Edit the files in {} directly", strip.name).into()),
                        !isolated,
                        p,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.emit(ComposerEvent::SetIsolated(false));
                        this.close(cx);
                    })),
                )
                .child(
                    menu_item(
                        "work-isolated",
                        Some(Icon::Isolated),
                        "Isolated copy",
                        Some("A separate Git worktree, so parallel threads never collide".into()),
                        isolated,
                        p,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.emit(ComposerEvent::SetIsolated(true));
                        this.close(cx);
                    })),
                )
                .child(separator(p));
        }
        menu.child(menu_label(if locked { "Start a new thread in" } else { "Project" }, p))
            .children(projects.into_iter().enumerate().map(|(index, path)| {
                let chosen = !locked && path == current;
                menu_item(
                    ("strip-project", index),
                    Some(Icon::Folder),
                    project::name(&path),
                    Some(path.display().to_string().into()),
                    chosen,
                    p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close(cx);
                    cx.emit(ComposerEvent::SwitchProject(path.clone()));
                }))
            }))
            .child(menu_item("strip-add-project", Some(Icon::FolderPlus), "Add a project", None, false, p).on_click(cx.listener(
                |this, _, _, cx| {
                    this.close(cx);
                    cx.emit(ComposerEvent::AddProject);
                },
            )))
    }

    fn chip(&self, id: &'static str, menu: Menu, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = palette(cx);
        let open = self.menu == Some(menu);
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(30.))
            .px(px(8.))
            .min_w_0()
            .rounded(px(8.))
            .text_size(px(13.5))
            .text_color(if open { p.text } else { p.text_muted })
            .cursor_pointer()
            .when(open, |chip| chip.bg(p.row_active))
            .hover(move |style| style.bg(p.row_hover).text_color(p.text))
            .on_click(cx.listener(move |this, _, window, cx| this.toggle(menu, window, cx)))
    }

    fn suggestion_list(&self, suggestions: &Suggestions, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let highlighted = self.highlighted;
        let row = |index: usize, name: Icon, title: String, detail: Option<String>, mono: bool| {
            div()
                .id(("suggestion", index))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(index == highlighted, |r| r.bg(p.row_active))
                .hover(move |style| style.bg(p.row_hover))
                .child(icon(name, 15., p.text_muted))
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(p.text)
                        .flex_shrink_0()
                        .when(mono, |t| t.font_family(MONO).text_size(px(13.)))
                        .child(title),
                )
                .when_some(detail, |r, detail| {
                    r.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.5))
                            .text_color(p.text_faint)
                            .child(detail),
                    )
                })
        };
        let (label, rows): (&str, Vec<Stateful<Div>>) = match suggestions {
            Suggestions::Commands(commands) => (
                "Commands",
                commands
                    .iter()
                    .enumerate()
                    .map(|(i, c)| row(i, Icon::Slash, format!("/{}", c.name()), Some(c.description().into()), false))
                    .collect(),
            ),
            Suggestions::Files { paths, .. } => (
                "Bring a file into the conversation",
                paths.iter().enumerate().map(|(i, path)| row(i, crate::thread_view::file_icon(path), path.clone(), None, true)).collect(),
            ),
        };
        menu_panel(p)
            .w_full()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(menu_label(label, p))
                    .child(menu_label("↑↓ to move · Enter to pick · Esc to dismiss", p)),
            )
            .children(
                rows.into_iter().enumerate().map(|(i, r)| r.on_click(cx.listener(move |this, _, window, cx| this.accept(i, window, cx)))),
            )
    }

    fn approval_card(&self, card: &ApprovalCard, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let more_open = self.menu == Some(Menu::ApprovalMore);
        let more = more_open.then(|| {
            let menu = menu_panel(p)
                .w(px(250.))
                .child(menu_item("allow-thread", Some(Icon::Bolt), "Allow everything in this thread", None, false, p).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.close(cx);
                        cx.emit(ComposerEvent::Decide(Decision::AlwaysAllow));
                    }),
                ))
                .child(menu_item("stop-run", Some(Icon::CircleStop), "Stop the agent", None, false, p).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.close(cx);
                        cx.emit(ComposerEvent::Stop);
                    },
                )));
            popup(window, cx, Anchor::BottomRight, menu, |this, cx| this.close(cx))
        });
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(16.))
            .py(px(10.))
            .rounded(px(14.))
            .border_1()
            .border_color(p.warning.opacity(0.45))
            .bg(p.warning.opacity(0.08))
            .child(icon(Icon::Shield, 17., p.warning))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(p.warning).child(card.title))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_family(MONO)
                                    .text_size(px(13.))
                                    .text_color(p.text)
                                    .child(card.target.clone()),
                            )
                            .when_some(card.stats, |row, (added, removed)| {
                                row.child(div().text_size(px(12.)).text_color(p.add).child(format!("+{added}")))
                                    .child(div().text_size(px(12.)).text_color(p.del).child(format!("-{removed}")))
                            }),
                    ),
            )
            .child(
                div()
                    .id("deny")
                    .h(px(30.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(p.border)
                    .text_size(px(13.))
                    .text_color(p.text)
                    .cursor_pointer()
                    .hover(move |style| style.bg(p.row_hover))
                    .child("Deny")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Decide(Decision::Decline)))),
            )
            .child(
                div()
                    .id("allow")
                    .h(px(30.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .bg(p.text)
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.bg)
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.9))
                    .child("Allow")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Decide(Decision::Approve)))),
            )
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .id("approval-more")
                            .size(px(30.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(p.border)
                            .cursor_pointer()
                            .when(more_open, |b| b.bg(p.row_active))
                            .hover(move |style| style.bg(p.row_hover))
                            .child(icon(Icon::Dots, 15., p.text_muted))
                            .on_click(cx.listener(|this, _, window, cx| this.toggle(Menu::ApprovalMore, window, cx))),
                    )
                    .children(more),
            )
    }

    fn attachment_chips(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        div().flex().flex_wrap().items_end().gap(px(6.)).px(px(4.)).children(self.attachments.iter().enumerate().map(
            |(index, attachment)| {
                let remove = div()
                    .id(("remove-attachment", index))
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(move |style| style.bg(p.row_hover))
                    .child(icon(Icon::X, 12., p.text_faint))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if index < this.attachments.len() {
                            this.attachments.remove(index);
                        }
                        cx.notify();
                    }));
                if attachment.image {
                    return div()
                        .relative()
                        .size(px(64.))
                        .rounded(px(10.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(p.border_soft)
                        .bg(p.fill)
                        .child(img(attachment.path.clone()).size_full().rounded(px(9.)).object_fit(ObjectFit::Cover))
                        .child(div().absolute().top(px(3.)).right(px(3.)).rounded(px(5.)).bg(p.menu_bg.opacity(0.85)).child(remove));
                }
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(28.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .rounded(px(8.))
                    .bg(p.fill)
                    .border_1()
                    .border_color(p.border_soft)
                    .text_size(px(12.5))
                    .text_color(p.text_muted)
                    .child(icon(Icon::FileText, 13., p.text_muted))
                    .child(div().max_w(px(220.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(attachment.name.clone()))
                    .child(remove)
            },
        ))
    }
}

fn search_box(input: &Entity<InputState>, p: Palette) -> Div {
    div()
        .mx(px(4.))
        .mb(px(4.))
        .px(px(8.))
        .h(px(32.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .bg(p.fill)
        .text_size(px(13.))
        .child(Input::new(input).appearance(false).prefix(icon(Icon::Search, 13., p.text_faint)))
}

fn note(text: impl Into<SharedString>, p: Palette) -> impl IntoElement {
    div().px(px(10.)).py(px(8.)).text_size(px(13.)).text_color(p.text_muted).child(text.into())
}

pub fn access_icon(access: Access) -> Icon {
    match access {
        Access::Supervised => Icon::Shield,
        Access::AutoEdits => Icon::FilePen,
        Access::Full => Icon::Bolt,
    }
}

fn divider(p: Palette) -> impl IntoElement {
    div().w(px(1.)).h(px(16.)).mx(px(2.)).bg(p.border)
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let empty = self.input.read(cx).value().trim().is_empty() && self.attachments.is_empty();
        let busy = self.busy;
        let access = Config::get(cx).access;
        let thinking = self.backend.read(cx).thinking(cx) == Thinking::Toggle;
        let thinking_on = Config::get(cx).thinking;
        let (has_model, label) = self.model_label(cx);
        let window_size = self.backend.read(cx).selected(cx).and_then(|m| m.context);
        let menu = self.menu.filter(|m| *m != Menu::ApprovalMore);
        let model_menu = (menu == Some(Menu::Model)).then(|| {
            let content = self.model_menu(cx);
            popup(window, cx, Anchor::BottomLeft, content, |this, cx| this.close(cx))
        });
        let thinking_menu = (menu == Some(Menu::Thinking)).then(|| {
            let content = self.thinking_menu(cx);
            popup(window, cx, Anchor::BottomLeft, content, |this, cx| this.close(cx))
        });
        let access_menu = (menu == Some(Menu::Access)).then(|| {
            let content = self.access_menu(cx);
            popup(window, cx, Anchor::BottomLeft, content, |this, cx| this.close(cx))
        });
        let workspace_menu = match (&self.strip, menu) {
            (Some(strip), Some(Menu::Workspace)) => {
                let content = self.workspace_menu(&strip.clone(), cx);
                Some(popup(window, cx, Anchor::BottomLeft, content, |this, cx| this.close(cx)))
            }
            _ => None,
        };
        let branch_menu = (self.strip.is_some() && menu == Some(Menu::Branch)).then(|| {
            let content = self.branch_menu(cx);
            popup(window, cx, Anchor::BottomRight, content, |this, cx| this.close(cx))
        });
        let suggestions = self.suggestions.clone().map(|s| self.suggestion_list(&s, cx));
        let approval = self.approval.clone().map(|card| self.approval_card(&card, window, cx));
        let chips = (!self.attachments.is_empty()).then(|| self.attachment_chips(cx));
        let model_chip = self
            .chip("model-chip", Menu::Model, cx)
            .when(has_model, |chip| chip.child(div().size(px(16.)).flex_shrink_0().child(self.mark.clone())))
            .when(!has_model, |chip| chip.child(svg().path(Icon::LlamaCpp.path()).size(px(14.)).text_color(p.text_faint)))
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
            .child(icon(Icon::ChevronDown, 13., p.text_faint));
        let thinking_chip = self
            .chip("thinking-chip", Menu::Thinking, cx)
            .child(icon(Icon::Bulb, 15., p.text_muted))
            .child(if thinking_on { "Thinking" } else { "Direct" })
            .child(icon(Icon::ChevronDown, 13., p.text_faint));
        let access_chip = self
            .chip("access-chip", Menu::Access, cx)
            .child(icon(access_icon(access), 15., p.text_muted))
            .child(access.label())
            .child(icon(Icon::ChevronDown, 13., p.text_faint));
        let attach = div()
            .id("attach")
            .size(px(30.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .cursor_pointer()
            .hover(move |style| style.bg(p.row_hover))
            .child(icon(Icon::Paperclip, 15., p.text_muted))
            .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx)));
        let send = div()
            .id("send")
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(34.))
            .rounded_full()
            .map(|send| {
                if busy {
                    send.bg(p.danger).cursor_pointer().child(icon(Icon::Stop, 12., gpui_kit::white()))
                } else if empty {
                    send.bg(p.fill).child(icon(Icon::ArrowUp, 16., p.text_faint))
                } else {
                    send.bg(p.text).cursor_pointer().hover(|s| s.opacity(0.9)).child(icon(Icon::ArrowUp, 16., p.bg))
                }
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if busy {
                    cx.emit(ComposerEvent::Stop);
                } else {
                    this.submit(window, cx);
                }
            }));
        let strip = self.strip.clone().map(|strip| {
            let open = self.menu == Some(Menu::Workspace);
            let branch_open = self.menu == Some(Menu::Branch);
            let branch_clickable = !busy;
            div()
                .mx(px(18.))
                .mt(px(-8.))
                .px(px(10.))
                .pt(px(8.))
                .pb(px(4.))
                .rounded_b(px(12.))
                .bg(p.fill)
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(12.5))
                .text_color(p.text_faint)
                .child(
                    div()
                        .relative()
                        .child(
                            div()
                                .id("workspace-mode")
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .h(px(24.))
                                .px(px(6.))
                                .rounded(px(6.))
                                .when(open, |b| b.bg(p.row_active))
                                .cursor_pointer()
                                .hover(move |s| s.bg(p.row_hover).text_color(p.text))
                                .child(icon(if strip.isolated { Icon::Isolated } else { Icon::Folder }, 13., p.text_faint))
                                .child(format!("{} · {}", if strip.isolated { "Isolated copy" } else { "Project folder" }, strip.name))
                                .child(icon(Icon::ChevronDown, 12., p.text_faint))
                                .on_click(cx.listener(|this, _, window, cx| this.toggle(Menu::Workspace, window, cx))),
                        )
                        .children(workspace_menu),
                )
                .when_some(strip.branch, |row, branch| {
                    row.child(
                        div()
                            .relative()
                            .child(
                                div()
                                    .id("branch-chip")
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .h(px(24.))
                                    .px(px(6.))
                                    .rounded(px(6.))
                                    .when(branch_open, |b| b.bg(p.row_active))
                                    .when(branch_clickable, |b| b.cursor_pointer().hover(move |s| s.bg(p.row_hover).text_color(p.text)))
                                    .child(icon(Icon::Branch, 13., p.text_faint))
                                    .child(branch)
                                    .when(branch_clickable, |b| b.child(icon(Icon::ChevronDown, 12., p.text_faint)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if branch_clickable {
                                            this.toggle(Menu::Branch, window, cx);
                                        }
                                    })),
                            )
                            .children(branch_menu),
                    )
                })
        });
        div()
            .w_full()
            .max_w(px(760.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(approval)
            .children(suggestions)
            .child(
                div()
                    .id("composer-box")
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .pt(px(12.))
                    .pb(px(8.))
                    .px(px(10.))
                    .rounded(px(18.))
                    .bg(p.composer_bg)
                    .border_1()
                    .border_color(p.composer_ring)
                    .hover(move |style| style.border_color(p.composer_ring_hover))
                    .drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(p.link).bg(p.link.opacity(0.06)))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.add_files(paths.paths().to_vec(), cx)))
                    .children(chips)
                    .child(div().px(px(4.)).text_size(px(14.5)).line_height(px(22.)).child(Textarea::new(&self.input).appearance(false)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .min_w_0()
                            .child(div().relative().min_w_0().child(model_chip).children(model_menu))
                            .when(thinking, |row| {
                                row.child(divider(p)).child(div().relative().child(thinking_chip).children(thinking_menu))
                            })
                            .child(divider(p))
                            .child(div().relative().child(access_chip).children(access_menu))
                            .child(div().flex_1())
                            .when(has_model, |row| row.child(context_ring::ring(self.context_used, window_size, p)))
                            .child(attach)
                            .child(send),
                    ),
            )
            .children(strip)
    }
}