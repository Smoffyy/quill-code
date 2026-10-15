use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use gpui_kit::{
    Context, Div, Entity, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Window, div, prelude::*, px,
};
use quill_config::claude_latest;
use quill_providers::Tokens;
use serde::{Deserialize, Serialize};

use crate::backend::claude_label;
use crate::config::app_dir;
use crate::icons::Icon;
use crate::palette::{Palette, palette};
use crate::thread::{Thread, Turn, now_ms};
use crate::ui::icon;
use crate::workspace::Workspace;

const DAY: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub at: u64,
    pub thread: u64,
    pub provider: String,
    pub model: String,
    pub tokens: Tokens,
    pub cost: Option<f64>,
}

fn ledger_path() -> PathBuf {
    app_dir().join("usage.jsonl")
}

pub fn load(threads: &[Thread]) -> Vec<Record> {
    if let Ok(text) = fs::read_to_string(ledger_path()) {
        return text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
    }
    let mut records: Vec<Record> =
        threads
            .iter()
            .flat_map(|thread| {
                thread.turns.iter().filter(|turn| !turn.working()).filter_map(|turn| {
                    record(thread.id, thread.provider.as_deref(), turn).map(|record| Record { at: turn.started, ..record })
                })
            })
            .collect();
    records.sort_by_key(|record| record.at);
    records.iter().for_each(append);
    records
}

pub fn append(record: &Record) {
    let Ok(line) = serde_json::to_string(record) else { return };
    let _ = fs::create_dir_all(app_dir());
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(ledger_path()) {
        let _ = writeln!(file, "{line}");
    }
}

pub fn record(thread: u64, provider: Option<&str>, turn: &Turn) -> Option<Record> {
    (turn.usage.total() > 0 || turn.cost.is_some()).then(|| Record {
        at: now_ms(),
        thread,
        provider: provider.unwrap_or_default().to_string(),
        model: turn.model.clone().unwrap_or_default(),
        tokens: turn.usage,
        cost: turn.cost,
    })
}

pub fn money(usd: f64) -> String {
    if usd == 0.0 {
        "$0".into()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

pub fn count(tokens: u64) -> String {
    match tokens {
        0..1000 => tokens.to_string(),
        1000..1_000_000 => format!("{:.1}K", tokens as f64 / 1000.0),
        _ => format!("{:.2}M", tokens as f64 / 1_000_000.0),
    }
}

#[derive(Default, Clone, Copy)]
struct Sum {
    tokens: Tokens,
    cost: f64,
    priced: bool,
    turns: usize,
}

impl Sum {
    fn add(&mut self, tokens: Tokens, cost: Option<f64>) {
        self.tokens.add(tokens);
        self.cost += cost.unwrap_or(0.0);
        self.priced |= cost.is_some();
        self.turns += 1;
    }
}

pub struct UsageView {
    workspace: Entity<Workspace>,
    _subscription: Subscription,
}

impl UsageView {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&workspace, |_, _, cx| cx.notify());
        UsageView { workspace, _subscription: subscription }
    }
}

fn section(title: &'static str, p: Palette) -> Div {
    div().pt(px(14.)).pb(px(8.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text_faint).child(title)
}

fn model_name(label: &str) -> String {
    claude_latest(label).map_or_else(|| label.to_string(), claude_label)
}

fn tokens_line(tokens: Tokens) -> String {
    if tokens.total() == 0 {
        return String::new();
    }
    let mut line = format!("{} in", count(tokens.prompt));
    if tokens.cache_read > 0 {
        line.push_str(&format!(" ({} cached)", count(tokens.cache_read)));
    }
    line.push_str(&format!(" · {} out", count(tokens.completion)));
    line
}

fn cost_text(sum: &Sum) -> String {
    if sum.priced { money(sum.cost) } else { "Free".into() }
}

fn tile(label: &'static str, sum: Sum, p: Palette) -> Div {
    div()
        .flex_1()
        .min_w(px(120.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .p(px(10.))
        .rounded(px(10.))
        .bg(p.fill)
        .child(div().text_size(px(12.)).text_color(p.text_faint).child(label))
        .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(money(sum.cost)))
        .child(div().text_size(px(11.5)).text_color(p.text_faint).child(format!("{} tokens", count(sum.tokens.total()))))
}

fn row(left: String, middle: String, right: String, p: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .py(px(6.))
        .border_b_1()
        .border_color(p.border_soft)
        .text_size(px(12.5))
        .child(div().flex_1().min_w_0().text_ellipsis().whitespace_nowrap().overflow_hidden().text_color(p.text).child(left))
        .child(div().flex_shrink_0().text_color(p.text_faint).child(middle))
        .child(div().w(px(64.)).flex_shrink_0().flex().justify_end().text_color(p.text_muted).child(right))
}

impl Render for UsageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let workspace = self.workspace.read(cx);
        let mut body = div().id("usage-body").size_full().overflow_y_scroll().px(px(16.)).pb(px(20.)).flex().flex_col();
        match workspace.active_thread() {
            Some(thread) => {
                let mut total = Sum::default();
                for turn in &thread.turns {
                    total.add(turn.usage, turn.cost);
                }
                body = body.child(section("This chat", p)).child(
                    div()
                        .flex()
                        .items_end()
                        .gap(px(10.))
                        .child(div().text_size(px(26.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(cost_text(&total)))
                        .child(div().pb(px(5.)).text_size(px(12.5)).text_color(p.text_faint).child(tokens_line(total.tokens))),
                );
                for (index, turn) in thread.turns.iter().enumerate().filter(|(_, t)| t.usage.total() > 0 || t.cost.is_some()) {
                    let model = model_name(turn.model.as_deref().unwrap_or_default());
                    let cost = turn.cost.map_or_else(|| "Free".to_string(), money);
                    body = body.child(row(format!("{}. {model}", index + 1), tokens_line(turn.usage), cost, p));
                }
            }
            None => {
                body = body
                    .child(section("This chat", p))
                    .child(div().text_size(px(13.)).text_color(p.text_faint).child("Open a thread to see what it has used."));
            }
        }
        let now = now_ms();
        let mut windows = [Sum::default(); 4];
        let mut models: HashMap<String, Sum> = HashMap::new();
        for record in &workspace.usage {
            for (sum, days) in windows.iter_mut().zip([1, 7, 30, u64::MAX / DAY]) {
                if now.saturating_sub(record.at) < days.saturating_mul(DAY) {
                    sum.add(record.tokens, record.cost);
                }
            }
            models.entry(model_name(&record.model)).or_default().add(record.tokens, record.cost);
        }
        let [day, week, month, all] = windows;
        body = body.child(section("Everything", p)).child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .child(tile("Today", day, p))
                .child(tile("Past 7 days", week, p))
                .child(tile("Past 30 days", month, p))
                .child(tile("All time", all, p)),
        );
        let mut models: Vec<(String, Sum)> = models.into_iter().collect();
        models.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost).then(b.1.tokens.total().cmp(&a.1.tokens.total())));
        if !models.is_empty() {
            body = body.child(section("By model", p));
            for (model, sum) in models {
                let turns = format!("{} {}", sum.turns, if sum.turns == 1 { "turn" } else { "turns" });
                body = body.child(row(model, format!("{turns} · {} tokens", count(sum.tokens.total())), cost_text(&sum), p));
            }
        }
        body.child(
            div()
                .pt(px(14.))
                .flex()
                .items_start()
                .gap(px(6.))
                .text_size(px(11.5))
                .text_color(p.text_faint)
                .child(div().mt(px(2.)).child(icon(Icon::Info, 12., p.text_faint)))
                .child(div().flex_1().min_w_0().child(SharedString::from(
                    "Claude Code reports its own cost. Anthropic API costs are worked out from Anthropic's published prices, cache included; set a price for any other model in Settings, Models. Local models are free.",
                ))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_naturally() {
        assert_eq!(money(0.0023), "$0.0023");
        assert_eq!(money(1.5), "$1.50");
        assert_eq!(count(950), "950");
        assert_eq!(count(52_300), "52.3K");
        assert_eq!(count(2_500_000), "2.50M");
    }
}