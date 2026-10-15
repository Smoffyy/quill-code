use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{
    Anchor, AnyElement, AnyView, App, Context, Div, ElementId, FontWeight, Hsla, IntoElement, MouseButton, ParentElement, SharedString,
    Stateful, Styled, Svg, Window, anchored, deferred, div, point, prelude::*, px, svg,
};

use crate::icons::Icon;
use crate::palette::Palette;

pub const MONO: &str = "Consolas";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

pub fn icon(name: Icon, size: f32, color: Hsla) -> Svg {
    svg().path(name.path()).flex_shrink_0().size(px(size)).text_color(color)
}

pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |window, cx| Tooltip::new(text.clone()).build(window, cx)
}

pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, tone: Tone, p: Palette) -> Stateful<Div> {
    let base = div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(12.))
        .rounded(px(8.))
        .text_size(px(13.))
        .cursor_pointer()
        .child(label.into());
    match tone {
        Tone::Primary => base.bg(p.accent).text_color(p.accent_text).font_weight(FontWeight::SEMIBOLD).hover(|style| style.opacity(0.9)),
        Tone::Secondary => base.border_1().border_color(p.border).text_color(p.text).hover(move |style| style.bg(p.row_hover)),
        Tone::Ghost => base.text_color(p.text_muted).hover(move |style| style.bg(p.row_hover).text_color(p.text)),
        Tone::Danger => base.bg(p.danger.opacity(0.14)).text_color(p.danger).hover(move |style| style.bg(p.danger.opacity(0.22))),
    }
}

pub fn icon_button(id: impl Into<ElementId>, name: Icon, label: &'static str, p: Palette) -> Stateful<Div> {
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
        .tooltip(tip(label))
        .child(icon(name, 16., p.text_muted))
}

pub fn kbd(text: impl Into<SharedString>, p: Palette) -> Div {
    div()
        .flex_shrink_0()
        .px(px(5.))
        .h(px(18.))
        .flex()
        .items_center()
        .rounded(px(5.))
        .border_1()
        .border_color(p.border)
        .text_size(px(11.))
        .text_color(p.text_faint)
        .child(text.into())
}

pub fn menu_panel(p: Palette) -> Div {
    div().occlude().p(px(6.)).rounded(px(14.)).bg(p.menu_bg).border_1().border_color(p.border).shadow_lg().flex().flex_col().gap(px(2.))
}

pub fn menu_label(text: impl Into<SharedString>, p: Palette) -> Div {
    div()
        .px(px(10.))
        .pt(px(6.))
        .pb(px(4.))
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.text_faint)
        .child(text.into())
}

pub fn menu_item(
    id: impl Into<ElementId>,
    name: Option<Icon>,
    label: impl Into<SharedString>,
    detail: Option<SharedString>,
    checked: bool,
    p: Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(10.))
        .py(px(if detail.is_some() { 7. } else { 6. }))
        .rounded(px(9.))
        .text_color(p.text)
        .cursor_pointer()
        .when(checked, |row| row.bg(p.accent_soft))
        .hover(move |style| style.bg(if checked { p.accent_soft } else { p.row_hover }))
        .when_some(name, |row, name| row.child(icon(name, 15., if checked { p.accent } else { p.text_muted })))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_size(px(13.5)).child(label.into()))
                .when_some(detail, |col, detail| col.child(div().text_size(px(12.)).text_color(p.text_faint).child(detail))),
        )
        .when(checked, |row| row.child(icon(Icon::Check, 15., p.accent)))
}

pub fn separator(p: Palette) -> Div {
    div().h(px(1.)).mx(px(6.)).my(px(3.)).bg(p.border_soft)
}

pub fn popup<V: 'static>(
    window: &Window,
    cx: &mut Context<V>,
    anchor: Anchor,
    menu: impl IntoElement,
    close: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let viewport = window.viewport_size();
    let offset = match anchor {
        Anchor::BottomLeft | Anchor::BottomRight => point(px(0.), px(-6.)),
        _ => point(px(0.), px(6.)),
    };
    let top = matches!(anchor, Anchor::TopLeft | Anchor::TopRight);
    let right = matches!(anchor, Anchor::TopRight | Anchor::BottomRight);
    let menu = crate::motion::pop("popup-menu", div().child(menu), !top, cx);
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .child(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("popup-backdrop")
                        .w(viewport.width)
                        .h(viewport.height)
                        .occlude()
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| close(this, cx)))
                        .on_mouse_down(MouseButton::Right, cx.listener(|_, _, _, cx| cx.stop_propagation())),
                ),
            )
            .with_priority(1),
        )
        .child(
            div()
                .absolute()
                .when(top, |d| d.bottom_0())
                .when(!top, |d| d.top_0())
                .when(right, |d| d.right_0())
                .when(!right, |d| d.left_0())
                .child(deferred(anchored().anchor(anchor).offset(offset).snap_to_window().child(menu)).with_priority(2)),
        )
        .into_any_element()
}

pub fn badge(name: &str, size: f32) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(size * 0.3))
        .bg(crate::project::color(name))
        .text_size(px(size * 0.46))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui_kit::white())
        .child(crate::project::initials(name))
}

pub fn name_hash(name: &str) -> u32 {
    name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}