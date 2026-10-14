use std::f32::consts::{FRAC_PI_2, TAU};

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{Bounds, Div, Hsla, ParentElement, PathBuilder, Pixels, Stateful, Styled, Window, canvas, div, point, prelude::*, px};
use quill_llama::context_label;

use crate::palette::Palette;

pub fn label(used: Option<u64>, limit: Option<u64>) -> String {
    match (used.filter(|n| *n > 0), limit.filter(|n| *n > 0)) {
        (Some(used), Some(limit)) => {
            let percent = ((used * 100 + limit / 2) / limit).min(100);
            format!("Context: {} of {} tokens used ({percent}%)", context_label(used), context_label(limit))
        }
        (Some(used), None) => format!("Context: {} tokens used", context_label(used)),
        (None, Some(limit)) => format!("Context: {} tokens, nothing used yet", context_label(limit)),
        (None, None) => "Context: nothing used yet".into(),
    }
}

fn share(used: Option<u64>, limit: Option<u64>) -> f32 {
    match (used, limit) {
        (Some(used), Some(limit)) if limit > 0 => (used as f32 / limit as f32).min(1.),
        _ => 0.,
    }
}

fn stroke(bounds: Bounds<Pixels>, from: f32, to: f32, color: Hsla, window: &mut Window) {
    let center = bounds.center();
    let radius = bounds.size.width.min(bounds.size.height) / 2. - px(1.5);
    let at = |turn: f32| {
        let angle = turn * TAU - FRAC_PI_2;
        point(center.x + radius * angle.cos(), center.y + radius * angle.sin())
    };
    let mut path = PathBuilder::stroke(px(2.));
    path.move_to(at(from));
    path.arc_to(point(radius, radius), px(0.), false, true, at(to));
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

pub fn ring(used: Option<u64>, limit: Option<u64>, p: Palette) -> Stateful<Div> {
    let share = share(used, limit);
    let color = if share >= 0.9 {
        p.danger
    } else if share >= 0.75 {
        p.warning
    } else {
        p.text_muted
    };
    let track = p.text_faint.opacity(0.35);
    let text = label(used, limit);
    div()
        .id("context-ring")
        .size(px(28.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    stroke(bounds, 0., 0.5, track, window);
                    stroke(bounds, 0.5, 1., track, window);
                    if share > 0.5 {
                        stroke(bounds, 0., 0.5, color, window);
                        stroke(bounds, 0.5, share, color, window);
                    } else if share > 0. {
                        stroke(bounds, 0., share, color, window);
                    }
                },
            )
            .size(px(15.)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_say_how_much_is_used() {
        assert_eq!(label(Some(13_107), Some(131_072)), "Context: 13K of 128K tokens used (10%)");
        assert_eq!(label(Some(2_500), None), "Context: 2K tokens used");
        assert_eq!(label(None, Some(65_536)), "Context: 64K tokens, nothing used yet");
        assert_eq!(label(Some(0), None), "Context: nothing used yet");
        assert_eq!(share(Some(200), Some(100)), 1.);
        assert_eq!(share(Some(50), None), 0.);
    }
}