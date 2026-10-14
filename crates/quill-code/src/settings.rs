use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::switch::Switch;
use gpui_kit::{
    AnyElement, Context, Div, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Subscription,
    Window, div, prelude::*, px, svg,
};
use quill_agent::Access;
use quill_llama::{Status, context_label};

use crate::backend::{Backend, Connection};
use crate::composer::access_icon;
use crate::config::{Config, personal_instructions_path, read_personal_instructions, write_personal_instructions};
use crate::icons::Icon;
use crate::palette::{Palette, palette};
use crate::project;
use crate::ui::{badge, icon, page};
use crate::version::VersionPage;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Models,
    Instructions,
    Projects,
    Version,
}

impl Section {
    const ALL: [Section; 5] = [Section::General, Section::Models, Section::Instructions, Section::Projects, Section::Version];

    fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Models => "Models",
            Section::Instructions => "Instructions",
            Section::Projects => "Projects",
            Section::Version => "Version",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Section::General => Icon::Sliders,
            Section::Models => Icon::Cube,
            Section::Instructions => Icon::FileText,
            Section::Projects => Icon::Folder,
            Section::Version => Icon::Info,
        }
    }
}

pub enum SettingsEvent {
    Back,
    AddProject,
}

pub struct Settings {
    pub section: Section,
    backend: Entity<Backend>,
    url: Entity<InputState>,
    instructions: Entity<TextareaState>,
    version: Entity<VersionPage>,
    saved: Option<Result<(), SharedString>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for Settings {}

impl Settings {
    pub fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url_text = Backend::url(cx);
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("http://127.0.0.1:9931").default_value(url_text));
        let instructions = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("For example: Prefer small, focused changes. Explain risky changes before making them.")
                .auto_grow(8, 28)
                .default_value(read_personal_instructions())
        });
        let subscriptions = vec![
            cx.subscribe_in(&url, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.connect(cx);
                }
            }),
            cx.observe(&backend, |_, _, cx| cx.notify()),
        ];
        Settings {
            section: Section::General,
            backend,
            url,
            instructions,
            version: cx.new(VersionPage::new),
            saved: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn open(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        if section == Section::Models {
            self.backend.update(cx, |b, cx| b.refresh(cx));
        }
        cx.notify();
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let url = self.url.read(cx).value().to_string();
        if !url.trim().is_empty() {
            self.backend.update(cx, |backend, cx| backend.set_url(&url, cx));
        }
    }

    fn save_instructions(&mut self, cx: &mut Context<Self>) {
        let text = self.instructions.read(cx).value().to_string();
        self.saved = Some(write_personal_instructions(&text).map_err(|e| e.to_string().into()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1600)).await;
            _ = this.update(cx, |this, cx| {
                if matches!(this.saved, Some(Ok(()))) {
                    this.saved = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn nav(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .w(px(248.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(1.))
            .px(px(8.))
            .pb(px(8.))
            .child(
                div()
                    .px(px(10.))
                    .pt(px(4.))
                    .pb(px(12.))
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text)
                    .child("Settings"),
            )
            .children(Section::ALL.map(|section| {
                let on = self.section == section;
                div()
                    .id(section.label())
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(34.))
                    .px(px(10.))
                    .rounded(px(8.))
                    .text_size(px(14.))
                    .text_color(if on { p.text } else { p.sidebar_text })
                    .cursor_pointer()
                    .when(on, |row| row.bg(p.row_active))
                    .hover(move |style| style.bg(p.row_hover).text_color(p.text))
                    .child(icon(section.icon(), 16., if on { p.text } else { p.sidebar_text }))
                    .child(section.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.open(section, cx)))
            }))
            .child(div().flex_1())
            .child(
                div()
                    .id("settings-back")
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(34.))
                    .px(px(10.))
                    .rounded(px(8.))
                    .text_size(px(14.))
                    .text_color(p.sidebar_text)
                    .cursor_pointer()
                    .hover(move |style| style.bg(p.row_hover).text_color(p.text))
                    .child(icon(Icon::ArrowLeft, 16., p.sidebar_text))
                    .child("Back")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Back))),
            )
    }

    fn general(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let config = Config::get(cx).clone();
        div()
            .flex()
            .flex_col()
            .child(heading("New threads", p))
            .child(
                card(p)
                    .child(row(
                        "Permissions",
                        "What agents may do without asking. You can also change it from the prompt box.",
                        div().flex().flex_col().gap(px(2.)).w(px(260.)).children(Access::ALL.map(|access| {
                            let on = config.access == access;
                            div()
                                .id(access.label())
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .h(px(30.))
                                .px(px(10.))
                                .rounded(px(8.))
                                .text_size(px(13.5))
                                .text_color(if on { p.text } else { p.text_muted })
                                .when(on, |seg| seg.bg(p.row_active))
                                .cursor_pointer()
                                .hover(move |style| style.bg(p.row_hover))
                                .child(icon(access_icon(access), 14., if on { p.text } else { p.text_faint }))
                                .child(access.label())
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    Config::update(cx, |config| config.access = access);
                                    cx.notify();
                                }))
                        })),
                        p,
                    ))
                    .child(row(
                        "Thinking",
                        "Let models that support it think before they answer. Detected from each model's chat template.",
                        Switch::new("thinking-default").checked(config.thinking).on_click({
                            let backend = self.backend.clone();
                            move |on, _, cx| backend.update(cx, |b, cx| b.set_thinking(*on, cx))
                        }),
                        p,
                    )),
            )
            .into_any_element()
    }

    fn models(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let backend = self.backend.read(cx);
        let (color, status): (_, SharedString) = match &backend.connection {
            Connection::Checking => (p.text_faint, "Connecting".into()),
            Connection::Online => {
                let count = backend.models.len();
                (p.success, format!("Connected · {count} {}", if count == 1 { "model" } else { "models" }).into())
            }
            Connection::Offline(error) => (p.danger, error.clone()),
        };
        let selected = Backend::selected_id(cx);
        let models: Vec<_> = backend.models.clone();
        div()
            .flex()
            .flex_col()
            .child(heading("llama.cpp", p))
            .child(
                card(p)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .px(px(18.))
                            .py(px(16.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(svg().path(Icon::LlamaCpp.path()).size(px(20.)).text_color(p.text))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .text_size(px(14.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(p.text)
                                                    .child("Server address"),
                                            )
                                            .child(div().text_size(px(13.)).text_color(p.text_faint).child(
                                                "In router mode every model the server can load is listed here and in the prompt box.",
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.))
                                    .child(div().flex_1().child(Input::new(&self.url)))
                                    .child(primary("connect", "Connect", p).on_click(cx.listener(|this, _, _, cx| this.connect(cx)))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(12.5))
                                    .text_color(p.text_muted)
                                    .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(color))
                                    .child(status),
                            ),
                    )
                    .children(models.into_iter().map(|model| {
                        let mut detail = vec![model.status.label().to_string()];
                        if let Some(context) = model.context {
                            detail.push(format!("{} context", context_label(context)));
                        }
                        if model.vision {
                            detail.push("Vision".into());
                        }
                        let chosen = selected.as_deref() == Some(model.id.as_str());
                        let id = model.id.clone();
                        div()
                            .id(SharedString::from(format!("settings-model-{}", model.id)))
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .px(px(18.))
                            .py(px(10.))
                            .border_t_1()
                            .border_color(p.border_soft)
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.row_hover))
                            .child(div().size(px(8.)).rounded_full().bg(if model.status == Status::Loaded {
                                p.success
                            } else {
                                p.text_faint.opacity(0.4)
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_size(px(14.)).text_color(p.text).child(model.id.clone()))
                                    .child(div().text_size(px(12.5)).text_color(p.text_faint).child(detail.join(" · "))),
                            )
                            .when(chosen, |row| row.child(div().text_size(px(12.5)).text_color(p.text_muted).child("In use")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = id.clone();
                                this.backend.update(cx, |b, cx| b.select(id, cx));
                            }))
                    })),
            )
            .into_any_element()
    }

    fn instructions(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let note = match &self.saved {
            Some(Ok(())) => Some((p.success, SharedString::from("Saved"))),
            Some(Err(error)) => Some((p.danger, error.clone())),
            None => None,
        };
        div()
            .flex()
            .flex_col()
            .child(heading("Personal instructions", p))
            .child(div().mb(px(12.)).text_size(px(13.5)).text_color(p.text_muted).child(format!(
                "Added to every thread, in every project, after the project's own AGENTS.md or CLAUDE.md. Saved as {}",
                personal_instructions_path().display()
            )))
            .child(Textarea::new(&self.instructions))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .mt(px(12.))
                    .child(primary("save-instructions", "Save", p).on_click(cx.listener(|this, _, _, cx| this.save_instructions(cx))))
                    .when_some(note, |row, (color, text)| row.child(div().text_size(px(12.5)).text_color(color).child(text))),
            )
            .into_any_element()
    }

    fn projects(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let projects = Config::get(cx).projects.clone();
        div()
            .flex()
            .flex_col()
            .child(div().flex().items_center().child(div().flex_1().child(heading("Projects", p))).child(
                primary("settings-add-project", "Add project", p).on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::AddProject))),
            ))
            .child(
                card(p)
                    .when(projects.is_empty(), |c| {
                        c.child(div().px(px(18.)).py(px(16.)).text_size(px(13.5)).text_color(p.text_faint).child("No projects yet."))
                    })
                    .children(projects.into_iter().enumerate().map(|(index, path)| {
                        let name = project::name(&path);
                        let open = path.clone();
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .px(px(18.))
                            .py(px(12.))
                            .when(index > 0, |r| r.border_t_1().border_color(p.border_soft))
                            .child(badge(&name, 22.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_size(px(14.)).text_color(p.text).child(name))
                                    .child(div().text_size(px(12.5)).text_color(p.text_faint).child(path.display().to_string())),
                            )
                            .child(ghost(("open-project", index), Icon::FolderOpen, p).on_click(move |_, _, _| project::open_folder(&open)))
                            .child(ghost(("remove-project", index), Icon::Trash, p).on_click(cx.listener(move |_, _, _, cx| {
                                Config::update(cx, |config| {
                                    config.projects.remove(index);
                                });
                                cx.notify();
                            })))
                    })),
            )
            .into_any_element()
    }
}

fn heading(text: &'static str, p: Palette) -> Div {
    div().pb(px(10.)).text_size(px(15.)).text_color(p.text_muted).child(text)
}

fn card(p: Palette) -> Div {
    div()
        .mb(px(28.))
        .rounded(px(14.))
        .bg(p.surface_2.opacity(0.5))
        .border_1()
        .border_color(p.border_soft)
        .flex()
        .flex_col()
        .overflow_hidden()
}

fn row(title: &'static str, detail: &'static str, control: impl IntoElement, p: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(24.))
        .px(px(18.))
        .py(px(16.))
        .border_b_1()
        .border_color(p.border_soft)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(div().text_size(px(14.5)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(title))
                .child(div().text_size(px(13.)).text_color(p.text_faint).child(detail)),
        )
        .child(control)
}

fn primary(id: &'static str, label: &'static str, p: Palette) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px(px(14.))
        .rounded(px(8.))
        .bg(p.text)
        .text_color(p.bg)
        .text_size(px(13.5))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .child(label)
}

fn ghost(id: impl Into<gpui_kit::ElementId>, name: Icon, p: Palette) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .cursor_pointer()
        .hover(move |style| style.bg(p.row_hover))
        .child(icon(name, 15., p.text_muted))
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let content = match self.section {
            Section::General => self.general(p, cx),
            Section::Models => self.models(p, cx),
            Section::Instructions => self.instructions(p, cx),
            Section::Projects => self.projects(p, cx),
            Section::Version => {
                return div().size_full().flex().child(self.nav(p, cx)).child(self.main(self.version.clone().into_any_element(), p));
            }
        };
        div().size_full().flex().child(self.nav(p, cx)).child(
            self.main(
                page("settings-page")
                    .child(div().max_w(px(820.)).mx_auto().px(px(36.)).pt(px(36.)).pb(px(40.)).child(content))
                    .into_any_element(),
                p,
            ),
        )
    }
}

impl Settings {
    fn main(&self, content: AnyElement, p: Palette) -> impl IntoElement + use<> {
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(p.bg)
            .rounded_tl(px(10.))
            .border_t_1()
            .border_l_1()
            .border_color(p.border_soft)
            .overflow_hidden()
            .child(content)
    }
}