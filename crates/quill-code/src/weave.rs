use std::time::Instant;

use gpui_kit::{
    Bounds, Context, Hsla, IntoElement, MouseButton, ParentElement, Pixels, Render, Styled, Window, canvas, div, fill, point, prelude::*,
    px, size,
};
use quill_weave::{BRAND_FRAME, Clock, Params, State, depth_order, point_count, project};

use crate::palette::palette;
use crate::ui::name_hash;

const POKE_DECAY: f64 = 3.0;

pub struct WeaveMark {
    target: State,
    params: Params,
    clock: Clock,
    last: Option<Instant>,
    poke: f64,
    opacity: f32,
}

impl WeaveMark {
    pub fn new(target: State) -> Self {
        WeaveMark { target, params: State::Idle.params(), clock: BRAND_FRAME, last: None, poke: 0.0, opacity: 1.0 }
    }

    pub fn seeded(mut self, name: &str) -> Self {
        self.clock = Clock::seeded(name_hash(name) as u64);
        self
    }

    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    pub fn set_state(&mut self, target: State, cx: &mut Context<Self>) {
        if self.target != target {
            if target == State::Idle {
                self.poke = 1.0;
            }
            self.target = target;
            cx.notify();
        }
    }

    fn step(&mut self, reduce_motion: bool) -> bool {
        if reduce_motion {
            self.params = self.target.params();
            self.poke = 0.0;
            self.last = None;
            return false;
        }
        let now = Instant::now();
        let dt = self.last.map_or(1.0 / 60.0, |last| (now - last).as_secs_f64().min(0.05));
        self.last = Some(now);
        self.params = self.params.ease(self.target.params(), dt);
        self.clock.advance(&self.params, dt);
        self.poke = (self.poke - dt * POKE_DECAY).max(0.0);
        true
    }
}

impl Render for WeaveMark {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.step(cx.reduce_motion()) {
            window.request_animation_frame();
        }
        let params = self.params;
        let clock = self.clock;
        let poke = self.poke;
        let opacity = self.opacity;
        let color = palette(cx).text;
        div()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.poke = 1.0;
                    cx.notify();
                }),
            )
            .child(
                canvas(|_, _, _| (), move |bounds, _, window, _| paint(bounds, &params, &clock, poke, opacity, color, window)).size_full(),
            )
    }
}

fn paint(bounds: Bounds<Pixels>, params: &Params, clock: &Clock, poke: f64, opacity: f32, color: Hsla, window: &mut Window) {
    let width = bounds.size.width.as_f32() as f64;
    let height = bounds.size.height.as_f32() as f64;
    let side = width.min(height);
    if side <= 0.0 {
        return;
    }
    let radius = side * 0.38 * (1.0 + 0.22 * poke);
    let base = (0.85 / window.scale_factor() as f64).max(radius * 0.05);
    let cx0 = bounds.origin.x.as_f32() as f64 + width / 2.0;
    let cy0 = bounds.origin.y.as_f32() as f64 + height / 2.0;
    let mut dots = Vec::new();
    let mut order = Vec::new();
    project(point_count(side), params, clock, &mut dots);
    depth_order(&dots, &mut order);
    for i in order {
        let dot = dots[i];
        let r = base * dot.size * (1.0 + 0.5 * poke);
        let alpha = (dot.alpha + 0.55 * poke).min(1.0) as f32 * opacity;
        let x = cx0 + dot.x * radius - r;
        let y = cy0 + dot.y * radius - r;
        let bounds = Bounds::new(point(px(x as f32), px(y as f32)), size(px((r * 2.0) as f32), px((r * 2.0) as f32)));
        window.paint_quad(fill(bounds, color.opacity(alpha)).corner_radii(px(r as f32)));
    }
}