use std::sync::atomic::AtomicBool;

use quill_providers::{ChatRequest, Effort, Error, Event, Message, Provider, Role};

const SUMMARY: &str = "Summary of the conversation so far:";

const SUMMARIZE: &str = "You summarize a coding session so it can continue in a fresh context. Write a compact summary covering: what the user asked for, decisions made, files created or changed (with paths), commands run and their results, the current state, and what is left to do. Keep exact names, paths and error messages that matter. Reply with the summary only.";

pub fn estimate(history: &[Message]) -> u64 {
    let chars: usize = history
        .iter()
        .map(|m| {
            m.content.len()
                + m.images.len() * 3000
                + m.tool_calls.iter().map(|c| c.function.name.len() + c.function.arguments.len()).sum::<usize>()
        })
        .sum();
    (chars / 4) as u64 + 2000
}

pub fn over(used: u64, window: Option<u64>, threshold: f32) -> bool {
    window.is_some_and(|window| used as f32 > window as f32 * threshold)
}

pub fn cut(history: &[Message]) -> Option<usize> {
    history.iter().rposition(|m| m.role == Role::User).filter(|index| *index > 0)
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

pub fn transcript(messages: &[Message], max_chars: usize) -> String {
    let lines: Vec<String> = messages
        .iter()
        .map(|m| match m.role {
            Role::System => format!("System: {}", clip(&m.content, 2000)),
            Role::User => format!("User: {}", clip(&m.content, 6000)),
            Role::Assistant => {
                let calls: Vec<String> =
                    m.tool_calls.iter().map(|c| format!("[{} {}]", c.function.name, clip(&c.function.arguments, 400))).collect();
                format!("Assistant: {} {}", clip(&m.content, 4000), calls.join(" ")).trim_end().to_string()
            }
            Role::Tool => format!("Tool result: {}", clip(&m.content, 1500)),
        })
        .collect();
    let text = lines.join("\n\n");
    if text.len() <= max_chars {
        return text;
    }
    let mut start = text.len() - max_chars;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[earlier part cut]\n{}", &text[start..])
}

pub fn summarize(
    provider: &dyn Provider,
    model: &str,
    effort: Option<Effort>,
    window: Option<u64>,
    head: &[Message],
    cancel: &AtomicBool,
) -> Result<String, Error> {
    let budget = window.map_or(48_000, |w| (w as usize).saturating_mul(2).max(8000));
    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![Message::system(SUMMARIZE), Message::user(transcript(head, budget))],
        thinking: Some(false),
        effort,
        ..ChatRequest::default()
    };
    let mut text = String::new();
    provider.stream(&request, cancel, &mut |event| {
        if let Event::Content(chunk) = event {
            text.push_str(&chunk);
        }
    })?;
    let text = match text.rfind("</think>") {
        Some(end) => text[end + 8..].trim().to_string(),
        None => text.trim().to_string(),
    };
    if text.is_empty() { Err(Error("The model returned an empty summary.".into())) } else { Ok(text) }
}

pub fn join(summary: &str, mut tail: Vec<Message>) -> Vec<Message> {
    if let Some(first) = tail.first_mut() {
        first.content = format!("{SUMMARY}\n\n{summary}\n\n---\n\n{}", first.content);
    }
    tail
}

pub fn trim_outputs(history: &mut [Message], keep: usize) -> bool {
    let tools: Vec<usize> = history.iter().enumerate().filter(|(_, m)| m.role == Role::Tool).map(|(i, _)| i).collect();
    let mut changed = false;
    for &index in tools.iter().rev().skip(keep) {
        if history[index].content.len() > 600 {
            history[index].content = format!("{} [older output trimmed to save space]", clip(&history[index].content, 400));
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_providers::ToolCall;

    fn history() -> Vec<Message> {
        vec![
            Message::user("first"),
            Message::assistant("", vec![ToolCall::new("a", "read_file", "{}")]),
            Message::tool("a", "x".repeat(2000)),
            Message::assistant("done", Vec::new()),
            Message::user("second"),
            Message::assistant("ok", Vec::new()),
        ]
    }

    #[test]
    fn compaction_keeps_the_latest_request() {
        let history = history();
        assert_eq!(cut(&history), Some(4));
        assert_eq!(cut(&history[..4]), None);
        let joined = join("They read a file.", history[4..].to_vec());
        assert!(joined[0].content.starts_with(SUMMARY) && joined[0].content.ends_with("second"));
        assert_eq!(joined.len(), 2);
    }

    #[test]
    fn thresholds_and_estimates_decide_when() {
        assert!(over(900, Some(1000), 0.8));
        assert!(!over(700, Some(1000), 0.8));
        assert!(!over(1_000_000, None, 0.8));
        assert!(estimate(&history()) > 2000);
    }

    #[test]
    fn old_tool_output_is_trimmed() {
        let mut history = history();
        assert!(trim_outputs(&mut history, 0));
        assert!(history[2].content.len() < 600);
        assert!(!trim_outputs(&mut history, 0));
    }

    #[test]
    fn transcripts_keep_the_end_when_too_long() {
        let text = transcript(&history(), 50);
        assert!(text.starts_with("[earlier part cut]") && text.ends_with("ok"));
    }
}