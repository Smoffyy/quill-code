use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::input::{Input, InputEvent, InputState, Position, Textarea, TextareaState};
use gpui_kit::{
    Anchor, App, Context, Div, Entity, EventEmitter, ExternalPaths, Focusable, FontWeight, Hsla, IntoElement, KeystrokeEvent, ObjectFit,
    ParentElement, PathPromptOptions, Render, SharedString, Stateful, Styled, Subscription, Window, div, img, prelude::*, px,
};
use quill_config::{ModelRef, ProviderKind};
use quill_core::{Access, Decision, FileDiff};
use quill_providers::{Model, Status, context_label};
use quill_weave::State;

use crate::attachments::Attachment;
use crate::backend::{Backend, Connection, Control, Source, provider_icon};
use crate::config::Config;
use crate::context_ring;
use crate::icons::Icon;
use crate::mentions::{self, Command, Trigger};
use crate::palette::{Palette, palette};
use crate::project;
use crate::ui::{MONO, Tone, button, icon, kbd, menu_item, menu_label, menu_panel, popup, separator, tip};
use crate::weave::WeaveMark;
use crate::workspace::Prompt;
use quill_git::{self as git, Branches};

pub enum ComposerEvent {
    Submit(Prompt),
    Stop,
    Decide(Decision),
    Answer(String),
    OpenModelSettings,
    Command(Command),
    SetIsolated(bool),
    SwitchProject(PathBuf),
    AddProject,
    RepoChanged,
}

#[derive(Clone)]
pub struct ApprovalCard {
    pub title: String,
    pub target: String,
    pub stats: Option<(usize, usize)>,
    pub diffs: Vec<FileDiff>,
    pub similar: Option<String>,
    pub tool: String,
}

#[derive(Clone)]
pub struct QuestionCard {
    pub question: String,
    pub options: Vec<String>,
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
enum ModelTab {
    Pinned,
    Source(String),
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
    marks: HashMap<String, Entity<WeaveMark>>,
    menu: Option<Menu>,
    model_tab: Option<ModelTab>,
    busy: bool,
    approval: Option<ApprovalCard>,
    question: Option<QuestionCard>,
    show_diff: bool,
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
            marks: HashMap::new(),
            menu: None,
            model_tab: None,
            busy: false,
            approval: None,
            question: None,
            show_diff: false,
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
        question: Option<QuestionCard>,
        strip: Option<Strip>,
        context_used: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        self.busy = busy;
        self.context_used = context_used;
        if approval.as_ref().map(|a| &a.target) != self.approval.as_ref().map(|a| &a.target) {
            self.show_diff = false;
        }
        self.approval = approval;
        self.question = question;
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
            let pinned = Config::get(cx).models.pinned.clone();
            if let Some(model) = pinned.get(index - 1).cloned() {
                self.backend.update(cx, |backend, cx| backend.select(model, cx));
                cx.stop_propagation();
            }
            return;
        }
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        if key == "escape" {
            if self.menu.is_some() {
                self.close(cx);
                cx.stop_propagation();
            } else if self.suggestions.is_some() {
                self.suggestions = None;
                cx.notify();
                cx.stop_propagation();
            } else if self.approval.is_some() && focused {
                cx.emit(ComposerEvent::Decide(Decision::Decline));
                cx.stop_propagation();
            }
            return;
        }
        if key == "enter"
            && !modifiers.shift
            && focused
            && self.approval.is_some()
            && self.suggestions.is_none()
            && self.input.read(cx).value().trim().is_empty()
        {
            cx.emit(ComposerEvent::Decide(Decision::Approve));
            cx.stop_propagation();
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
            Command::Reasoning => match self.backend.read(cx).control(cx) {
                Control::Toggle => {
                    let on = !Config::get(cx).models.thinking;
                    self.backend.update(cx, |backend, cx| backend.set_thinking(on, cx));
                }
                Control::Effort(_) => self.open(Menu::Thinking, window, cx),
                Control::None => {}
            },
            Command::Init if !self.busy => {
                cx.emit(ComposerEvent::Submit(Prompt { text: mentions::INIT_PROMPT.into(), attachments: Vec::new() }));
            }
            other => cx.emit(ComposerEvent::Command(other)),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.suggestions.is_some() {
            return;
        }
        let text = self.input.read(cx).value().trim().to_string();
        if self.question.is_some() && !text.is_empty() {
            self.input.update(cx, |input, cx| input.set_value("", window, cx));
            cx.emit(ComposerEvent::Answer(text));
            return;
        }
        if self.busy && text.is_empty() {
            return;
        }
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
            self.model_tab = None;
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
        if let Some((_, model)) = backend.selected(cx) {
            return (true, model.label().to_string().into());
        }
        let checking = backend.sources.iter().any(|s| s.connection == Connection::Checking);
        match Backend::selected_ref(cx) {
            Some(selected) if checking => (true, selected.model.into()),
            _ if backend.sources.iter().any(|s| s.ready() && !s.models.is_empty()) => (false, "Choose a model".into()),
            _ if checking => (false, "Connecting".into()),
            _ => (false, "No model connected".into()),
        }
    }

    fn model_mark(&mut self, model: &str, cx: &mut Context<Self>) -> Entity<WeaveMark> {
        self.marks.entry(model.to_string()).or_insert_with(|| cx.new(|_| WeaveMark::new(State::Idle).seeded(model))).clone()
    }

    fn model_row(&mut self, source: &Source, model: &Model, shortcut: Option<usize>, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = palette(cx);
        let backend_logo = source.config.kind != ProviderKind::Local;
        let mark = if backend_logo {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(icon(provider_icon(source.config.kind), 18., p.text))
                .into_any_element()
        } else {
            self.model_mark(&model.id, cx).into_any_element()
        };
        let target = ModelRef::new(source.config.id.clone(), model.id.clone());
        let selected = Backend::selected_ref(cx).as_ref() == Some(&target);
        let pinned = Config::get(cx).models.pinned.contains(&target);
        let local = source.config.kind == ProviderKind::Local;
        let mut detail = Vec::new();
        if local {
            detail.push(model.status.label().to_string());
        }
        if let Some(context) = model.context {
            detail.push(format!("{} context", context_label(context)));
        }
        if model.vision {
            detail.push("Vision".into());
        }
        if !model.efforts.is_empty() {
            detail.push("Reasoning".into());
        }
        if model.name.is_some() && !local {
            detail.push(model.id.clone());
        }
        let ready = !local || model.status == Status::Loaded;
        let key = format!("{}-{}", source.config.id, model.id);
        let pin_target = target.clone();
        div()
            .id(SharedString::from(format!("model-{key}")))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(selected, |row| row.bg(p.row_active))
            .hover(move |style| style.bg(p.row_hover))
            .child(div().relative().flex_shrink_0().size(px(24.)).child(mark).child(
                div().absolute().right_0().bottom_0().size(px(8.)).rounded_full().border_1().border_color(p.menu_bg).bg(if ready {
                    p.success
                } else {
                    p.text_faint.opacity(0.5)
                }),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(13.5)).text_color(p.text).child(model.label().to_string()))
                    .when(!detail.is_empty(), |col| {
                        col.child(div().text_size(px(12.)).text_color(p.text_faint).text_ellipsis().child(detail.join(" · ")))
                    }),
            )
            .when_some(shortcut, |row, n| row.child(kbd(format!("Ctrl+{n}"), p)))
            .child(
                div()
                    .id(SharedString::from(format!("pin-{key}")))
                    .size(px(24.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .hover(move |style| style.bg(p.row_active))
                    .tooltip(tip(if pinned { "Unpin" } else { "Pin to the top" }))
                    .child(icon(Icon::Pin, 13., if pinned { p.accent } else { p.text_faint.opacity(0.6) }))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        let target = pin_target.clone();
                        Config::update(cx, |config| {
                            if config.models.pinned.contains(&target) {
                                config.models.pinned.retain(|m| *m != target);
                            } else {
                                config.models.pinned.push(target);
                            }
                        });
                        cx.notify();
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let target = target.clone();
                this.backend.update(cx, |backend, cx| backend.select(target, cx));
                this.close(cx);
            }))
    }

    fn source_header(&self, source: &Source, cx: &App) -> Div {
        let p = palette(cx);
        let color = match source.connection {
            Connection::Online => p.success,
            Connection::Checking => p.text_faint,
            _ => p.warning,
        };
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .pt(px(8.))
            .pb(px(4.))
            .child(icon(provider_icon(source.config.kind), 14., p.text_muted))
            .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(p.text_muted).child(source.config.name.clone()))
            .child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(color))
            .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(p.text_faint).text_ellipsis().child(source.status()))
    }

    fn current_tab(&self, sources: &[Source], has_pinned: bool, cx: &App) -> Option<ModelTab> {
        let valid = |tab: &ModelTab| match tab {
            ModelTab::Pinned => has_pinned,
            ModelTab::Source(id) => sources.iter().any(|s| s.config.id == *id),
        };
        self.model_tab
            .clone()
            .filter(valid)
            .or_else(|| Backend::selected_ref(cx).map(|r| ModelTab::Source(r.provider)).filter(valid))
            .or_else(|| sources.iter().find(|s| !s.models.is_empty()).or(sources.first()).map(|s| ModelTab::Source(s.config.id.clone())))
    }

    #[allow(clippy::too_many_arguments)]
    fn tab_button(
        &self,
        id: SharedString,
        name: Icon,
        label: String,
        dot: Option<Hsla>,
        on: bool,
        tab: ModelTab,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = palette(cx);
        div()
            .id(id)
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(34.))
            .rounded(px(9.))
            .cursor_pointer()
            .when(on, |tab| tab.bg(p.row_active))
            .hover(move |style| style.bg(if on { p.row_active } else { p.row_hover }))
            .tooltip(tip(label))
            .child(icon(name, 18., if on { p.text } else { p.text_muted }))
            .when_some(dot, |button, color| {
                button.child(
                    div().absolute().right(px(5.)).bottom(px(5.)).size(px(7.)).rounded_full().border_1().border_color(p.menu_bg).bg(color),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.search.update(cx, |search, cx| search.set_value("", window, cx));
                this.model_tab = Some(tab.clone());
                cx.notify();
            }))
    }

    fn model_menu(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let query = self.search.read(cx).value().trim().to_lowercase();
        let pinned_refs = Config::get(cx).models.pinned.clone();
        let sources: Vec<Source> = self.backend.read(cx).sources.iter().filter(|s| s.connection != Connection::Off).cloned().collect();
        let pinned: Vec<(Source, Model)> = pinned_refs
            .iter()
            .filter_map(|r| {
                let source = sources.iter().find(|s| s.config.id == r.provider)?;
                source.models.iter().find(|m| m.id == r.model).map(|m| (source.clone(), m.clone()))
            })
            .collect();
        let current = self.current_tab(&sources, !pinned.is_empty(), cx);
        let searching = !query.is_empty();
        let mut rail = div().flex().flex_col().items_center().gap(px(4.)).w(px(36.)).flex_shrink_0().pt(px(2.));
        if !pinned.is_empty() {
            let on = !searching && current == Some(ModelTab::Pinned);
            rail = rail.child(self.tab_button("tab-pinned".into(), Icon::Pin, "Pinned".into(), None, on, ModelTab::Pinned, cx));
        }
        for source in &sources {
            let color = match source.connection {
                Connection::Online => p.success,
                Connection::Checking => p.text_faint,
                _ => p.warning,
            };
            let tab = ModelTab::Source(source.config.id.clone());
            let on = !searching && current.as_ref() == Some(&tab);
            let id = SharedString::from(format!("tab-{}", source.config.id));
            rail = rail.child(self.tab_button(id, provider_icon(source.config.kind), source.config.name.clone(), Some(color), on, tab, cx));
        }
        rail = rail.child(div().flex_1()).child(
            div()
                .id("server-settings")
                .flex()
                .items_center()
                .justify_center()
                .size(px(34.))
                .rounded(px(9.))
                .cursor_pointer()
                .hover(move |style| style.bg(p.row_hover))
                .tooltip(tip("Providers and models"))
                .child(icon(Icon::Sliders, 17., p.text_faint))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close(cx);
                    cx.emit(ComposerEvent::OpenModelSettings);
                })),
        );
        let matches =
            |model: &Model| !searching || model.id.to_lowercase().contains(&query) || model.label().to_lowercase().contains(&query);
        let mut list = div().id("model-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(1.));
        if searching {
            let mut shown = 0;
            for source in &sources {
                let models: Vec<&Model> = source.models.iter().filter(|m| matches(m)).collect();
                if models.is_empty() {
                    continue;
                }
                list = list.child(self.source_header(source, cx));
                for model in models {
                    shown += 1;
                    list = list.child(self.model_row(source, model, None, cx));
                }
            }
            if shown == 0 {
                list = list.child(note("No model matches", p));
            }
        } else {
            match &current {
                Some(ModelTab::Pinned) => {
                    for (n, (source, model)) in pinned.iter().enumerate() {
                        list = list.child(self.model_row(source, model, (n < 9).then_some(n + 1), cx));
                    }
                }
                Some(ModelTab::Source(id)) => {
                    if let Some(source) = sources.iter().find(|s| s.config.id == *id) {
                        list = list.child(self.source_header(source, cx));
                        for model in &source.models {
                            list = list.child(self.model_row(source, model, None, cx));
                        }
                        if source.models.is_empty() {
                            list = list.child(match source.connection {
                                Connection::Checking => note("Looking for models", p).into_any_element(),
                                Connection::Online => note("This provider lists no models", p).into_any_element(),
                                _ => menu_item("setup-provider", Some(Icon::Sliders), "Set up this provider", None, false, p)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close(cx);
                                        cx.emit(ComposerEvent::OpenModelSettings);
                                    }))
                                    .into_any_element(),
                            });
                        }
                    }
                }
                None => list = list.child(note("No providers are turned on", p)),
            }
        }
        let search_row = div().flex().items_center().gap(px(2.)).child(div().flex_1().child(search_box(&self.search, p))).child(
            div()
                .id("refresh-models")
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .size(px(30.))
                .mb(px(4.))
                .mr(px(2.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(move |style| style.bg(p.row_hover))
                .tooltip(tip("Look for models again"))
                .child(icon(Icon::Refresh, 14., p.text_faint))
                .on_click(cx.listener(|this, _, _, cx| this.backend.update(cx, |b, cx| b.refresh(cx)))),
        );
        menu_panel(p).w(px(390.)).child(
            div()
                .flex()
                .gap(px(6.))
                .h(px(330.))
                .child(rail)
                .child(div().w(px(1.)).flex_shrink_0().my(px(2.)).bg(p.border_soft))
                .child(div().flex_1().min_w_0().flex().flex_col().child(search_row).child(list)),
        )
    }

    fn thinking_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let models = &Config::get(cx).models;
        let (on, chosen) = (models.thinking, models.effort);
        match self.backend.read(cx).control(cx) {
            Control::Effort(levels) => {
                let current = chosen.nearest(&levels);
                menu_panel(p).w(px(270.)).child(menu_label("Effort", p)).children(levels.into_iter().map(|effort| {
                    menu_item(effort.wire(), None, effort.label(), Some(effort.description().into()), current == Some(effort), p).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.backend.update(cx, |b, cx| b.set_effort(effort, cx));
                            this.close(cx);
                        }),
                    )
                }))
            }
            _ => menu_panel(p).w(px(240.)).child(menu_label("Reasoning", p)).children(
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
            ),
        }
    }

    fn access_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let current = Config::get(cx).agent.access;
        menu_panel(p).w(px(360.)).child(menu_label("What the agent may do on its own", p)).children(Access::ALL.map(|access| {
            menu_item(access.label(), Some(access_icon(access)), access.label(), Some(access.description().into()), access == current, p)
                .on_click(cx.listener(move |this, _, _, cx| {
                    Config::update(cx, |config| config.agent.access = access);
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
        let projects = Config::get(cx).general.projects.clone();
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
            .text_size(px(13.))
            .text_color(if open { p.text } else { p.text_muted })
            .cursor_pointer()
            .hover(move |style| style.text_color(p.text))
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

    fn question_card(&self, card: &QuestionCard, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let view = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(16.))
            .border_1()
            .border_color(p.accent.opacity(0.4))
            .bg(p.raised)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .size(px(30.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(p.accent_soft)
                            .child(icon(Icon::Chat, 15., p.accent)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(p.accent).child("Quill is asking"))
                            .child(div().text_size(px(14.)).text_color(p.text).child(card.question.clone())),
                    ),
            )
            .when(!card.options.is_empty(), |col| {
                col.child(div().pl(px(42.)).flex().flex_wrap().gap(px(6.)).children(card.options.iter().enumerate().map(
                    |(index, option)| {
                        let answer = option.clone();
                        div()
                            .id(("question-option", index))
                            .px(px(12.))
                            .h(px(30.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .border_1()
                            .border_color(p.border)
                            .text_size(px(13.))
                            .text_color(p.text)
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.accent_soft).border_color(p.accent.opacity(0.5)))
                            .child(option.clone())
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ComposerEvent::Answer(answer.clone()))))
                    },
                )))
            })
            .child(div().pl(px(42.)).text_size(px(12.)).text_color(p.text_faint).child("Or type an answer below and press Enter"));
        crate::motion::rise("question-card", view, 10., cx)
    }

    fn approval_card(&self, card: &ApprovalCard, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let more_open = self.menu == Some(Menu::ApprovalMore);
        let more = more_open.then(|| {
            let decide = |decision: Decision| {
                cx.listener(move |this: &mut Self, _: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                    this.close(cx);
                    cx.emit(ComposerEvent::Decide(decision));
                })
            };
            let similar = card.similar.clone().map(|similar| {
                menu_item("allow-similar", Some(Icon::Check), format!("Always allow \"{similar}\""), None, false, p)
                    .on_click(decide(Decision::AllowSimilar))
            });
            let tool = (!card.tool.is_empty()).then(|| {
                menu_item("allow-tool", Some(Icon::Shield), format!("Always allow: {}", card.tool), None, false, p)
                    .on_click(decide(Decision::AllowTool))
            });
            let menu = menu_panel(p)
                .w(px(280.))
                .children(similar)
                .children(tool)
                .child(
                    menu_item("allow-thread", Some(Icon::Bolt), "Allow everything in this thread", None, false, p)
                        .on_click(decide(Decision::AllowAll)),
                )
                .child(menu_item("stop-run", Some(Icon::CircleStop), "Stop the agent", None, false, p).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.close(cx);
                        cx.emit(ComposerEvent::Stop);
                    },
                )));
            popup(window, cx, Anchor::BottomRight, menu, |this, cx| this.close(cx))
        });
        let show_diff = self.show_diff && !card.diffs.is_empty();
        let preview = show_diff.then(|| {
            div()
                .id("approval-diff")
                .max_h(px(260.))
                .overflow_y_scroll()
                .rounded(px(10.))
                .bg(p.sidebar)
                .border_1()
                .border_color(p.border_soft)
                .py(px(6.))
                .children(card.diffs.iter().map(|diff| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .when(card.diffs.len() > 1, |col| {
                            col.child(
                                div().px(px(10.)).font_family(MONO).text_size(px(12.)).text_color(p.text_muted).child(diff.path.clone()),
                            )
                        })
                        .child(crate::thread_view::diff_lines(diff, 40, p))
                }))
        });
        let card_view = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(16.))
            .border_1()
            .border_color(p.warning.opacity(0.4))
            .bg(p.raised)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .size(px(30.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(p.warning.opacity(0.14))
                            .child(icon(Icon::Shield, 15., p.warning)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(card.title.clone()))
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
                                            .text_size(px(12.5))
                                            .text_color(p.text_muted)
                                            .child(card.target.clone()),
                                    )
                                    .when_some(card.stats, |row, (added, removed)| {
                                        row.child(div().text_size(px(12.)).text_color(p.add).child(format!("+{added}")))
                                            .child(div().text_size(px(12.)).text_color(p.del).child(format!("−{removed}")))
                                    }),
                            ),
                    )
                    .when(!card.diffs.is_empty(), |row| {
                        row.child(
                            button("approval-show", if show_diff { "Hide changes" } else { "Show changes" }, Tone::Ghost, p).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.show_diff = !this.show_diff;
                                    cx.notify();
                                }),
                            ),
                        )
                    }),
            )
            .children(preview)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().text_size(px(12.)).text_color(p.text_faint).child("Enter to allow · Esc to deny"))
                    .child(
                        div()
                            .relative()
                            .child(
                                button("approval-more", "Always allow", Tone::Ghost, p)
                                    .when(more_open, |b| b.bg(p.row_active))
                                    .child(icon(Icon::ChevronDown, 12., p.text_faint))
                                    .on_click(cx.listener(|this, _, window, cx| this.toggle(Menu::ApprovalMore, window, cx))),
                            )
                            .children(more),
                    )
                    .child(
                        button("deny", "Deny", Tone::Secondary, p)
                            .child(kbd("Esc", p))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Decide(Decision::Decline)))),
                    )
                    .child(
                        button("allow", "Allow", Tone::Primary, p)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Decide(Decision::Approve)))),
                    ),
            );
        crate::motion::rise("approval-card", card_view, 10., cx)
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

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let empty = self.input.read(cx).value().trim().is_empty() && self.attachments.is_empty();
        let busy = self.busy;
        let access = Config::get(cx).agent.access;
        let control = self.backend.read(cx).control(cx);
        let thinking = control != Control::None;
        let thinking_label = match &control {
            Control::Effort(levels) => Config::get(cx).models.effort.nearest(levels).map_or("Effort", |e| e.label()),
            _ if Config::get(cx).models.thinking => "Thinking",
            _ => "Direct",
        };
        let (has_model, label) = self.model_label(cx);
        let provider_logo = Backend::selected_ref(cx)
            .and_then(|model| Config::get(cx).models.provider(&model.provider).map(|provider| provider.kind))
            .filter(|kind| *kind != ProviderKind::Local);
        let window_size = Backend::selected_ref(cx).and_then(|model| self.backend.read(cx).effective(&model, cx)).and_then(|m| m.context);
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
            .when(has_model, |chip| match provider_logo {
                Some(kind) => chip.child(icon(provider_icon(kind), 15., p.text)),
                None => chip.child(div().size(px(16.)).flex_shrink_0().child(self.mark.clone())),
            })
            .when(!has_model, |chip| chip.child(icon(Icon::Cube, 14., p.text_faint)))
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
            .child(icon(Icon::ChevronDown, 13., p.text_faint));
        let thinking_chip = self
            .chip("thinking-chip", Menu::Thinking, cx)
            .child(icon(Icon::Bulb, 15., p.text_muted))
            .child(thinking_label)
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
                if busy && empty {
                    send.bg(p.danger).cursor_pointer().hover(|s| s.opacity(0.9)).tooltip(tip("Stop the agent")).child(icon(
                        Icon::Stop,
                        12.,
                        gpui_kit::white(),
                    ))
                } else if empty {
                    send.bg(p.fill).child(icon(Icon::ArrowUp, 16., p.text_faint))
                } else {
                    send.bg(p.accent)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.9))
                        .tooltip(tip(if busy { "Add this while the agent works" } else { "Send" }))
                        .child(icon(Icon::ArrowUp, 16., p.accent_text))
                }
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if busy && empty {
                    cx.emit(ComposerEvent::Stop);
                } else {
                    this.submit(window, cx);
                }
            }));
        let question = self.question.clone().map(|card| self.question_card(&card, cx));
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
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let width = f32::from(Config::get(cx).appearance.transcript_width.clamp(560, 1600)) - 56.;
        div()
            .w_full()
            .max_w(px(width))
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(approval)
            .children(question)
            .children(suggestions)
            .child(
                div()
                    .id("composer-box")
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pt(px(12.))
                    .pb(px(10.))
                    .px(px(12.))
                    .rounded(px(20.))
                    .bg(p.composer_bg)
                    .border_1()
                    .border_color(if focused { p.accent.opacity(0.55) } else { p.composer_ring })
                    .shadow_lg()
                    .hover(move |style| style.border_color(if focused { p.accent.opacity(0.7) } else { p.composer_ring_hover }))
                    .drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(p.link).bg(p.link.opacity(0.06)))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.add_files(paths.paths().to_vec(), cx)))
                    .children(chips)
                    .child(div().px(px(4.)).text_size(px(14.5)).line_height(px(22.)).child(Textarea::new(&self.input).appearance(false)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .child(div().relative().min_w_0().child(model_chip).children(model_menu))
                            .when(thinking, |row| row.child(div().relative().child(thinking_chip).children(thinking_menu)))
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