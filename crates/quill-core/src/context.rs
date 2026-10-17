use std::path::Path;
use std::sync::atomic::AtomicBool;

use quill_providers::{ChatRequest, Error, Event, Message, Provider, Role, Tuning};

use crate::protocol::{PlanStep, StepStatus};
use crate::tools;

const SUMMARY: &str = "Summary of the conversation so far:";

const SUMMARIZE: &str = "You are compacting a coding session so the assistant can carry on in a fresh context without losing anything that matters. Write the summary under these headings:

1. Request: everything the user asked for, in their own words where the wording matters.
2. Decisions and constraints: choices made, the user's preferences and any rules to keep following.
3. Files: every file read, created or changed, with its path, what it holds or what was changed, and the exact code the next steps depend on.
4. Commands and results: commands run, what they showed, errors met and how they were fixed.
5. Current work: exactly what was being done when this summary was made.
6. Next step: the very next action to take.
7. Remaining: what is left to do after that.

Keep exact names, paths, identifiers, values and error messages. Write only what the conversation shows; never guess. Reply with the summary only.";

const CONTINUE: &str = "The conversation above was summarised because the model's context was nearly full. Carry on with the task from where it stopped, without repeating steps the summary says are done.";

const REREAD: &str = "Before you use the exact contents of any file listed above, read it again with read_file. The summary describes those files but is not a copy of them, so never quote or answer from it when the exact text matters.";

const RECENT: usize = 12;

const RESTORED: usize = 5;

const RESTORED_HEADING: &str = "Current contents of the files you worked with most recently, read again from disk just now:";

const ACKNOWLEDGED: &str = "I have the summary of our work so far.";

pub fn estimate(history: &[Message]) -> u64 {
    let chars: usize = history
        .iter()
        .map(|m| {
            m.content.len()
                + m.reasoning.as_ref().map_or(0, String::len)
                + m.images.len() * 3000
                + m.tool_calls.iter().map(|c| c.function.name.len() + c.function.arguments.len()).sum::<usize>()
        })
        .sum();
    (chars / 4) as u64
}

pub fn limit(window: u64) -> u64 {
    let reserve = (window / 4).clamp(4096, 20_000).min(window / 2);
    window - reserve
}

pub fn over(used: u64, window: Option<u64>) -> bool {
    window.is_some_and(|window| used > limit(window))
}

pub fn overflowed(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("context") && ["exceed", "too long", "too large", "overflow", "length", "size"].iter().any(|word| error.contains(word))
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    match text[end..].lines().count() {
        0 | 1 => format!("{} [cut here: the rest of this text was not kept]", &text[..end]),
        lines => format!("{} [cut here: {} more lines were not kept]", &text[..end], lines - 1),
    }
}

fn line(message: &Message, recent: bool) -> String {
    let room = if recent { 3 } else { 1 };
    match message.role {
        Role::System => format!("System: {}", clip(&message.content, 2000)),
        Role::User => format!("User: {}", clip(&message.content, 6000 * room)),
        Role::Assistant => {
            let calls: Vec<String> =
                message.tool_calls.iter().map(|c| format!("[{} {}]", c.function.name, clip(&c.function.arguments, 600 * room))).collect();
            format!("Assistant: {} {}", clip(&message.content, 4000 * room), calls.join(" ")).trim_end().to_string()
        }
        Role::Tool => format!("Tool result: {}", clip(&message.content, 1500 * room)),
    }
}

pub fn transcript(messages: &[Message], max_chars: usize) -> String {
    let recent = messages.len().saturating_sub(RECENT);
    let lines: Vec<String> = messages.iter().enumerate().map(|(index, message)| line(message, index >= recent)).collect();
    let text = lines.join("\n\n");
    if text.len() <= max_chars {
        return text;
    }
    let first = lines.first().map(|first| clip(first, max_chars / 4)).unwrap_or_default();
    let mut start = text.len() - max_chars.saturating_sub(first.len()).max(1);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{first}\n\n[middle part cut]\n{}", &text[start..])
}

pub fn summarize(
    provider: &dyn Provider,
    model: &str,
    tuning: &Tuning,
    window: Option<u64>,
    history: &[Message],
    cancel: &AtomicBool,
) -> Result<String, Error> {
    let budget = window.map_or(48_000, |w| (w as usize).saturating_mul(2).max(8000));
    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![Message::system(SUMMARIZE), Message::user(transcript(history, budget))],
        reasoning: tuning.quiet.clone(),
        kwargs: tuning.kwargs.clone(),
        context: window,
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

fn requests(history: &[Message]) -> Vec<&str> {
    let start = history.iter().rposition(|m| m.role == Role::Assistant && m.tool_calls.is_empty()).map_or(0, |index| index + 1);
    history[start..].iter().filter(|m| m.role == Role::User).map(|m| m.content.as_str()).collect()
}

pub fn recent(history: &[Message], budget: u64) -> usize {
    let mut start = history.len();
    for (index, message) in history.iter().enumerate().rev() {
        if message.role == Role::User {
            break;
        }
        if message.role == Role::Assistant && !message.tool_calls.is_empty() {
            if estimate(&history[index..]) > budget {
                break;
            }
            start = index;
        }
    }
    start
}

fn files(history: &[Message]) -> Vec<(String, bool)> {
    let mut files: Vec<(String, bool)> = Vec::new();
    for call in history.iter().flat_map(|m| &m.tool_calls) {
        let Some((path, changed)) = tools::file_of(&call.function.name, &call.function.arguments) else { continue };
        match files.iter_mut().find(|(known, _)| *known == path) {
            Some(file) => file.1 |= changed,
            None => files.push((path, changed)),
        }
    }
    files
}

fn plan(history: &[Message]) -> Option<Vec<PlanStep>> {
    history
        .iter()
        .rev()
        .flat_map(|m| m.tool_calls.iter().rev())
        .find_map(|call| tools::plan_of(&call.function.name, &call.function.arguments))
}

fn facts(history: &[Message]) -> String {
    let mut text = String::new();
    let files = files(history);
    if !files.is_empty() {
        let listed: Vec<String> =
            files.iter().map(|(path, changed)| format!("- {path} ({})", if *changed { "changed" } else { "read" })).collect();
        text.push_str(&format!("\n\n---\n\nFiles from this session:\n{}", listed.join("\n")));
    }
    if let Some(steps) = plan(history).filter(|steps| !steps.is_empty()) {
        let listed: Vec<String> = steps
            .iter()
            .map(|step| {
                let mark = match step.status {
                    StepStatus::Done => "done",
                    StepStatus::Active => "in progress",
                    StepStatus::Pending => "to do",
                };
                format!("- [{mark}] {}", step.text)
            })
            .collect();
        text.push_str(&format!("\n\n---\n\nThe plan when the summary was made:\n{}", listed.join("\n")));
    }
    text
}

fn paths(history: &[Message]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for call in history.iter().rev().flat_map(|m| m.tool_calls.iter().rev()) {
        if let Some((path, _)) = tools::file_of(&call.function.name, &call.function.arguments)
            && !seen.contains(&path)
        {
            seen.push(path);
        }
    }
    seen
}

pub fn restore(root: &Path, history: &[Message], kept: usize, budget: u64) -> Vec<(String, String)> {
    let shown = paths(&history[kept..]);
    let seen: Vec<String> = paths(&history[..kept]).into_iter().filter(|path| !shown.contains(path)).collect();
    let mut room = (budget * 4) as usize;
    let mut restored = Vec::new();
    for path in seen.into_iter().take(RESTORED) {
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else { continue };
        if room < 200 || text.contains('\0') {
            break;
        }
        let kept = clip(&text, room);
        room = room.saturating_sub(kept.len());
        restored.push((path, kept));
    }
    restored
}

pub fn rebuild(summary: &str, history: &[Message], mid_turn: bool, kept: usize, restored: &[(String, String)]) -> Vec<Message> {
    let mut text = format!("{SUMMARY}\n\n{summary}{}", facts(history));
    if !restored.is_empty() {
        let files: Vec<String> = restored.iter().map(|(path, contents)| format!("### {path}\n```\n{contents}\n```")).collect();
        text.push_str(&format!("\n\n---\n\n{RESTORED_HEADING}\n\n{}", files.join("\n\n")));
    }
    let reread = if files(history).is_empty() { String::new() } else { format!(" {REREAD}") };
    if !mid_turn {
        text.push_str(&format!("\n\n{}", reread.trim_start()));
        return vec![Message::user(text.trim_end().to_string()), Message::assistant(ACKNOWLEDGED, Vec::new())];
    }
    let asked: Vec<String> = requests(history).iter().map(|text| clip(text, 6000)).collect();
    if !asked.is_empty() {
        text.push_str(&format!("\n\n---\n\nWhat the user asked for in this turn:\n\n{}", asked.join("\n\n")));
    }
    text.push_str(&format!("\n\n---\n\n{CONTINUE}{reread}"));
    let mut rebuilt = vec![Message::user(text)];
    rebuilt.extend(history[kept..].iter().cloned());
    rebuilt
}

pub fn trim_outputs(history: &mut [Message], keep: usize) -> bool {
    let tools: Vec<usize> = history.iter().enumerate().filter(|(_, m)| m.role == Role::Tool).map(|(i, _)| i).collect();
    let mut changed = false;
    for &index in tools.iter().rev().skip(keep) {
        if history[index].content.len() > 600 {
            history[index].content = clip(&history[index].content, 400);
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
            Message::assistant("", vec![ToolCall::new("b", "shell", "{}")]),
            Message::tool("b", "ok"),
        ]
    }

    #[test]
    fn compaction_starts_near_the_end_of_the_window() {
        assert_eq!(limit(131_072), 111_072);
        assert_eq!(limit(32_768), 24_576);
        assert_eq!(limit(8192), 4096);
        assert!(over(30_000, Some(32_768)) && !over(20_000, Some(32_768)) && !over(1_000_000, None));
        assert_eq!(estimate(&history()[..3]), 504);
    }

    #[test]
    fn a_turn_compacted_midway_keeps_its_request_and_latest_steps() {
        let history = history();
        let rebuilt = rebuild("They read a file.", &history, true, recent(&history, 1000), &[]);
        assert_eq!(rebuilt.len(), 3);
        assert!(rebuilt[0].content.contains("They read a file.") && rebuilt[0].content.contains("\n\nsecond\n\n"));
        assert!(rebuilt[0].content.ends_with(CONTINUE));
        assert_eq!(rebuilt[1].tool_calls[0].function.name, "shell");
        assert_eq!(rebuilt[2].role, Role::Tool);
        assert_eq!(rebuild("s", &history, true, history.len(), &[]).len(), 1);
        let between = rebuild("s", &history, false, history.len(), &[]);
        assert_eq!((between[0].role, between[1].role), (Role::User, Role::Assistant));
    }

    #[test]
    fn the_files_touched_are_listed_so_they_get_read_again() {
        let history = vec![
            Message::user("fix it"),
            Message::assistant("", vec![ToolCall::new("a", "read_file", r#"{"path":"src\\main.rs"}"#)]),
            Message::tool("a", "fn main() {}"),
            Message::assistant("", vec![ToolCall::new("b", "edit_file", r#"{"file_path":"src/main.rs","old_text":"a","new_text":"b"}"#)]),
            Message::tool("b", "ok"),
            Message::assistant("", vec![ToolCall::new("c", "read_file", r#"{"path":"Cargo.toml"}"#)]),
            Message::tool("c", "[package]"),
        ];
        let text = facts(&history);
        assert!(text.contains("- src/main.rs (changed)\n- Cargo.toml (read)"), "{text}");
        assert!(!text.contains(REREAD));
        assert!(rebuild("s", &history, true, history.len(), &[])[0].content.ends_with(REREAD));
        assert!(rebuild("s", &history, false, history.len(), &[])[0].content.ends_with(REREAD));
        assert!(facts(&[Message::user("hi")]).is_empty());
    }

    #[test]
    fn the_latest_files_come_back_from_disk() {
        let root = std::env::temp_dir().join(format!("quill-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "first file").unwrap();
        std::fs::write(root.join("b.txt"), "second file").unwrap();
        let history = vec![
            Message::assistant("", vec![ToolCall::new("a", "read_file", r#"{"path":"a.txt"}"#)]),
            Message::tool("a", "first file"),
            Message::assistant("", vec![ToolCall::new("b", "edit_file", r#"{"path":"b.txt","old_text":"x","new_text":"y"}"#)]),
            Message::tool("b", "ok"),
            Message::assistant("", vec![ToolCall::new("c", "read_file", r#"{"path":"gone.txt"}"#)]),
            Message::tool("c", "missing"),
        ];
        let restored = restore(&root, &history, history.len(), 1000);
        assert_eq!(restored, vec![("b.txt".to_string(), "second file".to_string()), ("a.txt".to_string(), "first file".to_string())]);
        let rebuilt = rebuild("s", &history, true, history.len(), &restored);
        assert!(rebuilt[0].content.contains("### b.txt\n```\nsecond file\n```"));
        assert!(restore(&root, &history, history.len(), 10).is_empty());
        assert_eq!(restore(&root, &history, 2, 1000), vec![("a.txt".to_string(), "first file".to_string())]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn context_errors_are_recognised() {
        assert!(overflowed("The server answered 400: the request exceeds the available context size, try increasing it"));
        assert!(overflowed("This model's maximum context length is 32768 tokens"));
        assert!(!overflowed("Nothing is answering at that address."));
    }

    #[test]
    fn old_tool_output_is_trimmed() {
        let mut history = history();
        assert!(trim_outputs(&mut history, 1));
        assert!(history[2].content.len() < 600);
        assert!(!trim_outputs(&mut history, 1));
    }

    #[test]
    fn transcripts_keep_the_start_and_the_end_when_too_long() {
        let text = transcript(&history(), 200);
        assert!(text.starts_with("User: first") && text.contains("[middle part cut]") && text.ends_with("ok"));
    }
}