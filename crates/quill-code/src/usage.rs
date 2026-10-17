use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use quill_providers::Tokens;
use serde::{Deserialize, Serialize};

use crate::config::app_dir;
use crate::text::{Line, bold, dim, pad, truncate};
use crate::thread::{Thread, Turn, now_ms};

const DAY: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub at: u64,
    pub thread: u64,
    pub provider: String,
    pub model: String,
    pub tokens: Tokens,
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
    (turn.usage.total() > 0).then(|| Record {
        at: now_ms(),
        thread,
        provider: provider.unwrap_or_default().to_string(),
        model: turn.model.clone().unwrap_or_default(),
        tokens: turn.usage,
    })
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
    turns: usize,
}

impl Sum {
    fn add(&mut self, tokens: Tokens) {
        self.tokens.add(tokens);
        self.turns += 1;
    }
}

pub fn tokens_line(tokens: Tokens) -> String {
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

fn heading(title: &str) -> Line {
    Line::styled(title, bold())
}

pub fn report(thread: Option<&Thread>, records: &[Record], width: usize) -> Vec<Line> {
    let mut lines = Vec::new();
    if let Some(thread) = thread {
        let mut total = Sum::default();
        for turn in &thread.turns {
            total.add(turn.usage);
        }
        lines.push(heading("This thread").with(format!("  {}", tokens_line(total.tokens)), dim()));
        for (index, turn) in thread.turns.iter().enumerate().filter(|(_, t)| t.usage.total() > 0) {
            let model = turn.model.as_deref().unwrap_or_default();
            lines.push(Line::plain(format!("  {}. {model}", index + 1)).with(format!("  {}", tokens_line(turn.usage)), dim()));
        }
        lines.push(Line::default());
    }
    let now = now_ms();
    let mut windows = [Sum::default(); 4];
    let mut models: HashMap<String, Sum> = HashMap::new();
    for record in records {
        for (sum, days) in windows.iter_mut().zip([1, 7, 30, u64::MAX / DAY]) {
            if now.saturating_sub(record.at) < days.saturating_mul(DAY) {
                sum.add(record.tokens);
            }
        }
        models.entry(record.model.clone()).or_default().add(record.tokens);
    }
    lines.push(heading("Everything"));
    for (label, sum) in ["Today", "Past 7 days", "Past 30 days", "All time"].iter().zip(windows) {
        lines.push(Line::plain(format!("  {}", pad(label, 14))).with(format!("{} tokens", count(sum.tokens.total())), bold()));
    }
    let mut models: Vec<(String, Sum)> = models.into_iter().collect();
    models.sort_by_key(|(_, sum)| std::cmp::Reverse(sum.tokens.total()));
    if !models.is_empty() {
        lines.push(Line::default());
        lines.push(heading("By model"));
        let widest = models.iter().map(|(model, _)| model.chars().count()).max().unwrap_or(0).min(40);
        for (model, sum) in models {
            let turns = format!("{} {}", sum.turns, if sum.turns == 1 { "turn" } else { "turns" });
            lines.push(
                Line::plain(format!("  {}  ", pad(&model, widest)))
                    .with(format!("{} tokens", count(sum.tokens.total())), bold())
                    .with(format!(" · {turns}"), dim()),
            );
        }
    }
    lines.iter().map(|line| truncate(line, width)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_naturally() {
        assert_eq!(count(950), "950");
        assert_eq!(count(52_300), "52.3K");
        assert_eq!(count(2_500_000), "2.50M");
    }

    #[test]
    fn the_report_adds_up_by_window_and_model() {
        let tokens = Tokens { prompt: 1000, completion: 500, ..Tokens::default() };
        let old = now_ms() - 10 * DAY;
        let records = vec![
            Record { at: now_ms(), thread: 1, provider: "local".into(), model: "gemma".into(), tokens },
            Record { at: old, thread: 2, provider: "local".into(), model: "qwen3.5-4b".into(), tokens },
        ];
        let lines: Vec<String> = report(None, &records, 80).iter().map(Line::text).collect();
        assert!(lines.iter().any(|line| line.starts_with("  Today") && line.contains("1.5K tokens")));
        assert!(lines.iter().any(|line| line.starts_with("  All time") && line.contains("3.0K tokens")));
        assert!(lines.iter().any(|line| line.contains("gemma") && line.contains("1 turn")));
    }
}