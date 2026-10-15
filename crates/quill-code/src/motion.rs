use std::time::Duration;

use gpui_kit::base::animation::ease_out_cubic;
use gpui_kit::{
    Animation, AnimationExt, App, ElementId, Global, IntoElement, Styled, Svg, Transformation, percentage, pulsating_between, px,
};
use quill_config::Motion;

pub const QUICK: u64 = 140;
pub const BASE: u64 = 220;

#[derive(Clone, Copy)]
pub struct Pace {
    scale: f32,
}

impl Global for Pace {}

pub fn apply(motion: Motion, cx: &mut App) {
    cx.set_reduce_motion(motion == Motion::Off);
    let scale = match motion {
        Motion::Full => 1.0,
        Motion::Reduced => 0.5,
        Motion::Off => 0.0,
    };
    cx.set_global(Pace { scale });
}

pub fn duration(ms: u64, cx: &App) -> Duration {
    let scale = cx.try_global::<Pace>().map_or(1.0, |pace| pace.scale);
    Duration::from_millis((ms as f32 * scale).round() as u64)
}

pub fn rise<I: Into<ElementId>, E: IntoElement + Styled + 'static>(
    id: I,
    element: E,
    distance: f32,
    cx: &App,
) -> impl IntoElement + use<I, E> {
    element.relative().with_animation(id, Animation::new(duration(BASE, cx)).with_easing(ease_out_cubic), move |el, t| {
        el.opacity(t).top(px((1.0 - t) * distance))
    })
}

pub fn pop<I: Into<ElementId>, E: IntoElement + Styled + 'static>(
    id: I,
    element: E,
    from_below: bool,
    cx: &App,
) -> impl IntoElement + use<I, E> {
    let distance = if from_below { 6.0 } else { -6.0 };
    element.relative().with_animation(id, Animation::new(duration(QUICK, cx)).with_easing(ease_out_cubic), move |el, t| {
        el.opacity(t).top(px((1.0 - t) * distance))
    })
}

pub fn spin(id: impl Into<ElementId>, svg: Svg) -> impl IntoElement {
    svg.with_animation(id, Animation::new(Duration::from_millis(900)).repeat(), |svg, t| {
        svg.with_transformation(Transformation::rotate(percentage(t)))
    })
}

pub fn shimmer<I: Into<ElementId>, E: IntoElement + Styled + 'static>(id: I, element: E) -> impl IntoElement + use<I, E> {
    element.with_animation(id, Animation::new(Duration::from_millis(1600)).repeat().with_easing(pulsating_between(0.45, 1.0)), |el, t| {
        el.opacity(t)
    })
}