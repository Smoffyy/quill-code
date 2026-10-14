use std::path::PathBuf;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, IntoElement, KeyDownEvent, MouseButton, ParentElement, PathPromptOptions, Render,
    SharedString, Styled, Subscription, Window, div, prelude::*, px,
};

use crate::config::projects_dir;
use crate::git;
use crate::github::Visibility;
use crate::icons::Icon;
use crate::palette::{Palette, palette};
use crate::project;
use crate::ui::{MONO, icon};

pub enum AddProjectEvent {
    Added(PathBuf, Option<String>),
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Sources,
    NewProject,
    GitUrl,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    New,
    Folder,
    GitUrl,
    GitHub,
}

pub struct AddProject {
    step: Step,
    input: Entity<InputState>,
    branch: Entity<InputState>,
    visibility: Option<Visibility>,
    git: bool,
    busy: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AddProjectEvent> for AddProject {}

impl AddProject {
    pub fn new(git: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Where is the project coming from?"));
        let branch = cx.new(|cx| InputState::new(window, cx).default_value("main"));
        let on_input =
            |this: &mut Self, _: &Entity<InputState>, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>| match event {
                InputEvent::PressEnter { .. } => this.confirm(window, cx),
                InputEvent::Change => {
                    this.error = None;
                    cx.notify();
                }
                _ => {}
            };
        let subscriptions = vec![cx.subscribe_in(&input, window, on_input), cx.subscribe_in(&branch, window, on_input)];
        input.update(cx, |input, cx| input.focus(window, cx));
        AddProject { step: Step::Sources, input, branch, visibility: None, git, busy: false, error: None, _subscriptions: subscriptions }
    }

    fn value(&self, cx: &Context<Self>) -> String {
        self.input.read(cx).value().trim().to_string()
    }

    fn go(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        self.step = step;
        self.error = None;
        let placeholder = match step {
            Step::Sources => "Where is the project coming from?",
            Step::NewProject => "Name the new project",
            Step::GitUrl => "https://github.com/owner/repo.git",
        };
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(placeholder, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn sources(&self, cx: &Context<Self>) -> Vec<Source> {
        let query = self.value(cx).to_lowercase();
        [Source::New, Source::Folder, Source::GitUrl, Source::GitHub]
            .into_iter()
            .filter(|source| query.is_empty() || source_title(*source).to_lowercase().contains(&query))
            .collect()
    }

    fn pick(&mut self, source: Source, window: &mut Window, cx: &mut Context<Self>) {
        match source {
            Source::New => self.go(Step::NewProject, window, cx),
            Source::GitUrl if self.git => self.go(Step::GitUrl, window, cx),
            Source::Folder => {
                let picked = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some("Add project".into()),
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(mut paths))) = picked.await
                        && let Some(path) = paths.pop()
                    {
                        _ = this.update(cx, |_, cx| cx.emit(AddProjectEvent::Added(path, None)));
                    }
                })
                .detach();
            }
            _ => {}
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match self.step {
            Step::Sources => {
                if let Some(source) = self.sources(cx).first().copied() {
                    self.pick(source, window, cx);
                }
            }
            Step::NewProject => {
                let branch = self.branch.read(cx).value().trim().to_string();
                if self.git && !git::valid_branch(&branch) {
                    self.error = Some("Give the first branch a name Git accepts, such as main.".into());
                    cx.notify();
                    return;
                }
                let name = self.value(cx);
                let git_branch = self.git.then_some(branch);
                let visibility = self.visibility.filter(|_| self.git);
                self.run(cx, move || project::create(&name, git_branch.as_deref(), visibility).map(|created| (created.path, created.note)));
            }
            Step::GitUrl => {
                let url = self.value(cx);
                if !url.is_empty() {
                    self.run(cx, move || project::clone(&url).map(|path| (path, None)));
                }
            }
        }
    }

    fn run(&mut self, cx: &mut Context<Self>, work: impl FnOnce() -> Result<(PathBuf, Option<String>), String> + Send + 'static) {
        self.busy = true;
        self.error = None;
        cx.notify();
        let task = cx.background_executor().spawn(async move { work() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((path, note)) => cx.emit(AddProjectEvent::Added(path, note)),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn new_options(&self, p: Palette, cx: &mut Context<Self>) -> gpui_kit::Div {
        let choices = [
            ("vis-local", None, "Only on this PC"),
            ("vis-private", Some(Visibility::Private), "Private on GitHub"),
            ("vis-public", Some(Visibility::Public), "Public on GitHub"),
        ];
        let label = |text: &'static str| div().w(px(110.)).flex_shrink_0().text_size(px(13.)).text_color(p.text_muted).child(text);
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(12.))
            .pt(px(10.))
            .pb(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(label("First branch"))
                    .child(div().w(px(220.)).font_family(MONO).text_size(px(13.)).child(Input::new(&self.branch))),
            )
            .child(div().flex().items_center().child(label("Also on GitHub")).child(div().flex().gap(px(6.)).children(choices.map(
                |(id, choice, text)| {
                    let on = self.visibility == choice;
                    div()
                        .id(id)
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(if on { p.border } else { p.border_soft })
                        .when(on, |chip| chip.bg(p.row_active))
                        .text_size(px(12.5))
                        .text_color(if on { p.text } else { p.text_muted })
                        .cursor_pointer()
                        .hover(move |style| style.bg(p.row_hover))
                        .child(text)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.visibility = choice;
                            this.error = None;
                            cx.notify();
                        }))
                },
            ))))
    }

    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Sources => cx.emit(AddProjectEvent::Close),
            _ => self.go(Step::Sources, window, cx),
        }
    }
}

fn row(
    id: &'static str,
    name: Icon,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    badge: Option<&'static str>,
    first: bool,
    p: Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let enabled = badge.is_none();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .py(px(9.))
        .rounded(px(10.))
        .when(first && enabled, |row| row.bg(p.row_hover))
        .when(enabled, |row| row.cursor_pointer().hover(move |style| style.bg(p.row_active)))
        .child(icon(name, 18., if enabled { p.text_muted } else { p.text_faint }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_size(px(14.)).text_color(if enabled { p.text } else { p.text_faint }).child(title.into()))
                .child(div().text_size(px(12.5)).text_color(p.text_faint).child(detail.into())),
        )
        .when_some(badge, |row, badge| {
            row.child(
                div()
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(p.warning.opacity(0.5))
                    .bg(p.warning.opacity(0.1))
                    .text_size(px(11.5))
                    .text_color(p.warning)
                    .child(badge),
            )
        })
}

fn source_title(source: Source) -> &'static str {
    match source {
        Source::New => "Start fresh",
        Source::Folder => "Open a folder",
        Source::GitUrl => "Clone a repository",
        Source::GitHub => "From GitHub",
    }
}

fn key_hint(key: &'static str, label: &'static str, p: Palette) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .px(px(6.))
                .py(px(1.))
                .rounded(px(5.))
                .bg(p.fill)
                .border_1()
                .border_color(p.border_soft)
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.text_muted)
                .child(key),
        )
        .child(label)
}

impl Render for AddProject {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let home = projects_dir().display().to_string();
        let body = match self.step {
            Step::Sources => {
                let sources = self.sources(cx);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().px(px(12.)).pt(px(4.)).pb(px(6.)).text_size(px(12.)).text_color(p.text_faint).child("Add a project from"))
                    .children(sources.iter().enumerate().map(|(index, source)| {
                        let source = *source;
                        let first = index == 0;
                        let row = match source {
                            Source::New => {
                                row("src-new", Icon::FolderPlus, "Start fresh", format!("An empty folder in {home}"), None, first, p)
                            }
                            Source::Folder => {
                                row("src-folder", Icon::FolderOpen, "Open a folder", "Something already on this PC", None, first, p)
                            }
                            Source::GitUrl => row(
                                "src-git",
                                Icon::Link,
                                "Clone a repository",
                                "Paste any Git URL",
                                (!self.git).then_some("Needs Git"),
                                first,
                                p,
                            ),
                            Source::GitHub => row(
                                "src-github",
                                Icon::Globe,
                                "From GitHub",
                                "Pick from your repositories once you sign in",
                                Some("Soon"),
                                first,
                                p,
                            ),
                        };
                        row.on_click(cx.listener(move |this, _, window, cx| this.pick(source, window, cx)))
                    }))
            }
            Step::NewProject => {
                let name = project::clean_name(&self.value(cx));
                let target = if name.is_empty() { format!("Lives in {home}") } else { format!("Will be {home}\\{name}") };
                let title = match (self.busy, self.visibility.filter(|_| self.git)) {
                    (true, None) => "Creating, hang on",
                    (true, Some(_)) => "Creating and publishing, hang on",
                    (false, None) => "Create it",
                    (false, Some(Visibility::Private)) => "Create it and publish it privately",
                    (false, Some(Visibility::Public)) => "Create it and publish it publicly",
                };
                div()
                    .flex()
                    .flex_col()
                    .child(
                        row("create-project", Icon::FolderPlus, title, target, None, true, p)
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    )
                    .when(self.git, |col| col.child(self.new_options(p, cx)))
            }
            Step::GitUrl => {
                let name = project::repo_name(&self.value(cx));
                let target = if name.is_empty() { format!("Lands in {home}") } else { format!("Lands in {home}\\{name}") };
                let title = if self.busy { "Cloning, hang on" } else { "Clone it" };
                div().flex().flex_col().child(
                    row("clone-project", Icon::Download, title, target, None, true, p)
                        .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                )
            }
        };
        let viewport = window.viewport_size();
        div()
            .absolute()
            .top_0()
            .left_0()
            .w(viewport.width)
            .h(viewport.height)
            .occlude()
            .bg(gpui_kit::black().opacity(0.55))
            .flex()
            .items_start()
            .justify_center()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(AddProjectEvent::Close)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.back(window, cx);
                }
            }))
            .child(
                div()
                    .id("add-project")
                    .mt(px(96.))
                    .w(px(660.))
                    .max_h(px(560.))
                    .flex()
                    .flex_col()
                    .rounded(px(16.))
                    .bg(p.menu_bg)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(14.))
                            .py(px(12.))
                            .border_b_1()
                            .border_color(p.border_soft)
                            .child(
                                div()
                                    .id("sheet-back")
                                    .size(px(26.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(7.))
                                    .cursor_pointer()
                                    .hover(move |style| style.bg(p.row_hover))
                                    .child(icon(Icon::ArrowLeft, 15., p.text_muted))
                                    .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                            )
                            .child(div().flex_1().text_size(px(15.)).child(Input::new(&self.input).appearance(false)))
                            .when(self.step != Step::Sources, |row| {
                                row.child(
                                    div()
                                        .px(px(8.))
                                        .py(px(3.))
                                        .rounded(px(7.))
                                        .bg(p.fill)
                                        .text_size(px(12.))
                                        .text_color(p.text_muted)
                                        .child(if self.step == Step::NewProject { "Enter to create" } else { "Enter to clone" }),
                                )
                            }),
                    )
                    .child(div().id("sheet-body").p(px(6.)).overflow_y_scroll().child(body))
                    .when_some(self.error.clone(), |sheet, error| {
                        sheet.child(
                            div()
                                .mx(px(14.))
                                .mb(px(8.))
                                .px(px(12.))
                                .py(px(8.))
                                .rounded(px(8.))
                                .bg(p.danger.opacity(0.1))
                                .text_size(px(13.))
                                .text_color(p.danger)
                                .font_family(if error.contains("fatal") { MONO } else { crate::palette::FONT })
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .px(px(16.))
                            .py(px(10.))
                            .border_t_1()
                            .border_color(p.border_soft)
                            .text_size(px(12.5))
                            .text_color(p.text_faint)
                            .child(key_hint("Enter", "Choose", p))
                            .child(key_hint("Esc", "Go back", p)),
                    ),
            )
    }
}