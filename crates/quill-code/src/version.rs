use std::time::Duration;

use gpui_kit::{
    App, ClipboardItem, Context, Entity, FontWeight, IntoElement, ParentElement, Render, Styled, Window, div, linear_color_stop,
    linear_gradient, prelude::*, px,
};
use quill_weave::State;

use crate::icons::Icon;
use crate::palette::palette;
use crate::ui::icon;
use crate::weave::WeaveMark;

pub const APP_NAME: &str = "quill-code";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn version_line() -> String {
    let mut line = VERSION.split(['-', '+']).next().unwrap_or(VERSION).to_string();
    while line.matches('.').count() > 0 && line.ends_with(".0") {
        line.truncate(line.len() - 2);
    }
    line
}

fn platform() -> String {
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    };
    format!("{os} · {}", std::env::consts::ARCH)
}

fn channel() -> Option<&'static str> {
    if cfg!(debug_assertions) { Some("dev build") } else { None }
}

pub struct VersionPage {
    mark: Entity<WeaveMark>,
    copied: bool,
}

impl VersionPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        VersionPage { mark: cx.new(|_| WeaveMark::new(State::Idle).opacity(0.13)), copied: false }
    }

    fn copy_details(&mut self, cx: &mut Context<Self>) {
        let details = format!("{APP_NAME} {VERSION}\n{}", platform());
        cx.write_to_clipboard(ClipboardItem::new_string(details));
        self.copied = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1400)).await;
            _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn badge(&self, cx: &App) -> impl IntoElement {
        let p = palette(cx);
        div()
            .relative()
            .flex_shrink_0()
            .size(px(84.))
            .rounded(px(19.))
            .overflow_hidden()
            .border_1()
            .border_color(p.border_soft)
            .bg(linear_gradient(160., linear_color_stop(p.surface_hover, 0.), linear_color_stop(p.surface_2, 1.)))
            .child(div().absolute().right(px(-34.)).bottom(px(-38.)).size(px(112.)).child(self.mark.clone()))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(40.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text)
                    .child(version_line()),
            )
    }
}

impl Render for VersionPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let copied = self.copied;
        div()
            .id("version-page")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(px(640.))
                    .mx_auto()
                    .px(px(32.))
                    .pt(px(56.))
                    .pb(px(40.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(20.))
                            .pt(px(4.))
                            .pb(px(20.))
                            .child(self.badge(cx))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(30.))
                                            .line_height(px(34.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(p.text)
                                            .child(APP_NAME),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(8.))
                                            .mt(px(6.))
                                            .text_size(px(14.5))
                                            .text_color(p.text_muted)
                                            .child(format!("Version {VERSION}"))
                                            .when_some(channel(), |row, channel| {
                                                row.child(
                                                    div()
                                                        .px(px(9.))
                                                        .py(px(2.))
                                                        .rounded_full()
                                                        .border_1()
                                                        .border_color(p.border_soft)
                                                        .bg(p.surface_2)
                                                        .text_size(px(12.))
                                                        .text_color(p.text_muted)
                                                        .child(channel),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .mt(px(4.))
                                            .text_size(px(13.))
                                            .text_color(p.text_faint)
                                            .child(platform()),
                                    ),
                            ),
                    )
                    .child(
                        div().flex().gap(px(8.)).child(
                            div()
                                .id("copy-details")
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .h(px(32.))
                                .px(px(12.))
                                .rounded(px(8.))
                                .text_size(px(13.5))
                                .text_color(p.text)
                                .cursor_pointer()
                                .hover(move |style| style.bg(p.row_hover))
                                .child(icon(
                                    if copied { Icon::Check } else { Icon::Copy },
                                    15.,
                                    p.text_muted,
                                ))
                                .child(if copied { "Copied" } else { "Copy details" })
                                .on_click(cx.listener(|this, _, _, cx| this.copy_details(cx))),
                        ),
                    )
                    .child(
                        div()
                            .mt(px(28.))
                            .pt(px(20.))
                            .border_t_1()
                            .border_color(p.border_soft)
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text)
                                    .child("Release notes"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .line_height(px(21.))
                                    .text_color(p.text_muted)
                                    .child("The first development build: the window, the theme that follows Windows light and dark mode, the live weave mark and the Code layout. Agents arrive in the next milestones."),
                            ),
                    ),
            )
    }
}