use gpui_kit::{
    Anchor, AnyElement, Context, Div, ElementId, FontWeight, Hsla, IntoElement, MouseButton, ParentElement, SharedString, Stateful, Styled,
    Svg, Window, anchored, deferred, div, point, prelude::*, px, svg,
};

use crate::icons::Icon;
use crate::palette::Palette;

pub const MONO: &str = "Consolas";

pub fn icon(name: Icon, size: f32, color: Hsla) -> Svg {
    svg().path(name.path()).flex_shrink_0().size(px(size)).text_color(color)
}

pub fn page(id: &'static str) -> Stateful<Div> {
    div().id(id).size_full().overflow_y_scroll()
}

pub fn menu_panel(p: Palette) -> Div {
    div().occlude().p(px(5.)).rounded(px(12.)).bg(p.menu_bg).border_1().border_color(p.border).shadow_lg().flex().flex_col().gap(px(1.))
}

pub fn menu_label(text: impl Into<SharedString>, p: Palette) -> Div {
    div().px(px(10.)).pt(px(6.)).pb(px(4.)).text_size(px(12.)).text_color(p.text_faint).child(text.into())
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
        .rounded(px(8.))
        .text_color(p.text)
        .cursor_pointer()
        .hover(move |style| style.bg(p.row_hover))
        .when_some(name, |row, name| row.child(icon(name, 15., p.text_muted)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_size(px(13.5)).child(label.into()))
                .when_some(detail, |col, detail| col.child(div().text_size(px(12.)).text_color(p.text_faint).child(detail))),
        )
        .when(checked, |row| row.child(icon(Icon::Check, 15., p.text)))
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
        .rounded(px(size * 0.28))
        .bg(crate::project::color(name))
        .text_size(px(size * 0.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui_kit::white())
        .child(crate::project::initials(name))
}

pub fn name_hash(name: &str) -> u32 {
    name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}