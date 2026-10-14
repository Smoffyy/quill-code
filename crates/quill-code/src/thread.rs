use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use quill_agent::{FileDiff, ToolKind};
use quill_llama::Message;
use serde::{Deserialize, Serialize};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum TurnState {
    Working,
    Done,
    Stopped,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Waiting,
    Done,
    Failed,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ToolItem {
    pub id: String,
    pub kind: ToolKind,
    pub target: String,
    pub status: ToolStatus,
    pub output: String,
    pub diff: Option<FileDiff>,
    #[serde(skip)]
    pub open: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Item {
    Reasoning(String),
    Text(String),
    Tool(ToolItem),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Turn {
    pub prompt: String,
    pub started: u64,
    pub worked_ms: Option<u64>,
    pub items: Vec<Item>,
    pub state: TurnState,
    #[serde(default)]
    pub attachments: Vec<String>,
    #[serde(default)]
    pub images: Vec<PathBuf>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(skip)]
    pub expanded: bool,
}

pub struct FileChanges {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub created: bool,
}

impl Turn {
    pub fn new(prompt: String, attachments: Vec<String>) -> Turn {
        Turn {
            prompt,
            started: now_ms(),
            worked_ms: None,
            items: Vec::new(),
            state: TurnState::Working,
            attachments,
            images: Vec::new(),
            model: None,
            expanded: false,
        }
    }

    pub fn working(&self) -> bool {
        self.state == TurnState::Working
    }

    pub fn elapsed_secs(&self) -> u64 {
        self.worked_ms.unwrap_or_else(|| now_ms().saturating_sub(self.started)) / 1000
    }

    pub fn push_reasoning(&mut self, text: &str) {
        match self.items.last_mut() {
            Some(Item::Reasoning(existing)) => existing.push_str(text),
            _ => self.items.push(Item::Reasoning(text.to_string())),
        }
    }

    pub fn push_text(&mut self, text: &str) {
        match self.items.last_mut() {
            Some(Item::Text(existing)) => existing.push_str(text),
            _ => self.items.push(Item::Text(text.to_string())),
        }
    }

    pub fn tool_mut(&mut self, id: &str) -> Option<&mut ToolItem> {
        self.items.iter_mut().rev().find_map(|item| match item {
            Item::Tool(tool) if tool.id == id => Some(tool),
            _ => None,
        })
    }

    pub fn finish(&mut self, state: TurnState) {
        if !self.working() {
            return;
        }
        self.worked_ms = Some(now_ms().saturating_sub(self.started));
        for item in &mut self.items {
            if let Item::Tool(tool) = item
                && matches!(tool.status, ToolStatus::Running | ToolStatus::Waiting)
            {
                tool.status = ToolStatus::Failed;
            }
        }
        self.state = state;
    }

    pub fn final_text(&self) -> Option<usize> {
        if self.working() {
            return None;
        }
        let last = self.items.iter().rposition(|item| matches!(item, Item::Text(t) if !t.trim().is_empty()))?;
        let tools_after = self.items[last..].iter().any(|item| matches!(item, Item::Tool(_)));
        (!tools_after).then_some(last)
    }

    pub fn has_steps(&self) -> bool {
        let final_text = self.final_text();
        self.items
            .iter()
            .enumerate()
            .any(|(index, item)| Some(index) != final_text && !matches!(item, Item::Text(t) if t.trim().is_empty()))
    }

    pub fn changes(&self) -> Vec<FileChanges> {
        let mut by_path: BTreeMap<String, FileChanges> = BTreeMap::new();
        for diff in self.diffs() {
            let entry = by_path.entry(diff.path.clone()).or_insert(FileChanges {
                path: diff.path.clone(),
                added: 0,
                removed: 0,
                created: diff.created,
            });
            entry.added += diff.added;
            entry.removed += diff.removed;
        }
        by_path.into_values().collect()
    }

    pub fn diffs(&self) -> impl Iterator<Item = &FileDiff> {
        self.items.iter().filter_map(|item| match item {
            Item::Tool(ToolItem { diff: Some(diff), status: ToolStatus::Done, .. }) => Some(diff),
            _ => None,
        })
    }

    pub fn summary(&self) -> String {
        let files = self.changes().len();
        let mut commands = 0;
        let mut other = 0;
        for item in &self.items {
            if let Item::Tool(tool) = item {
                match tool.kind {
                    ToolKind::Run => commands += 1,
                    ToolKind::Write | ToolKind::Edit => {}
                    _ => other += 1,
                }
            }
        }
        let mut parts = Vec::new();
        if files > 0 {
            parts.push(format!("changed {files} {}", if files == 1 { "file" } else { "files" }));
        }
        if commands > 0 {
            parts.push(format!("ran {commands} {}", if commands == 1 { "command" } else { "commands" }));
        }
        if other > 0 {
            let noun = if other == 1 { "action" } else { "actions" };
            parts.push(format!("performed {other} {}{noun}", if parts.is_empty() { "" } else { "other " }));
        }
        let sentence = match parts.len() {
            0 => return "Thought it through".into(),
            1 => parts.remove(0),
            2 => parts.join(" and "),
            _ => {
                let last = parts.pop().unwrap_or_default();
                format!("{}, and {last}", parts.join(", "))
            }
        };
        let mut chars = sentence.chars();
        chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: u64,
    pub title: String,
    pub project: Option<PathBuf>,
    pub created: u64,
    pub updated: u64,
    pub turns: Vec<Turn>,
    pub history: Vec<Message>,
    #[serde(default)]
    pub worktree: Option<PathBuf>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub context_used: Option<u64>,
}

impl Thread {
    pub fn new(project: Option<PathBuf>, prompt: &str) -> Thread {
        let now = now_ms();
        Thread {
            id: now,
            title: title_from(prompt),
            project,
            created: now,
            updated: now,
            turns: Vec::new(),
            history: Vec::new(),
            worktree: None,
            model: None,
            context_used: None,
        }
    }

    pub fn workdir(&self) -> Option<PathBuf> {
        self.worktree.clone().or_else(|| self.project.clone())
    }

    pub fn recover(&mut self) {
        for turn in &mut self.turns {
            if turn.working() {
                turn.finish(TurnState::Stopped);
            }
        }
        repair_history(&mut self.history);
    }
}

pub fn repair_history(history: &mut Vec<Message>) {
    let Some(start) = history.iter().rposition(|m| !m.tool_calls.is_empty()) else { return };
    let answered: Vec<String> = history[start + 1..].iter().filter_map(|m| m.tool_call_id.clone()).collect();
    let missing: Vec<String> = history[start].tool_calls.iter().map(|call| call.id.clone()).filter(|id| !answered.contains(id)).collect();
    for id in missing {
        history.push(Message::tool(id, "Stopped by the user before this ran."));
    }
}

pub fn title_from(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or_default().trim();
    if line.chars().count() > 60 { format!("{}…", line.chars().take(59).collect::<String>().trim_end()) } else { line.to_string() }
}

pub fn ago(ms: u64) -> String {
    let secs = now_ms().saturating_sub(ms) / 1000;
    match secs {
        0..=44 => "now".into(),
        45..=3599 => format!("{}m", (secs / 60).max(1)),
        3600..=86_399 => format!("{}h", secs / 3600),
        86_400..=604_799 => format!("{}d", secs / 86_400),
        _ => format!("{}w", secs / 604_800),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_agent::diff_text;

    fn tool(id: &str, kind: ToolKind, diff: Option<FileDiff>) -> Item {
        Item::Tool(ToolItem { id: id.into(), kind, target: "x".into(), status: ToolStatus::Done, output: String::new(), diff, open: false })
    }

    #[test]
    fn streamed_text_joins_until_a_tool_runs() {
        let mut turn = Turn::new("p".into(), Vec::new());
        turn.push_text("Hel");
        turn.push_text("lo");
        turn.items.push(tool("1", ToolKind::Read, None));
        turn.push_text("Done");
        assert_eq!(turn.items.len(), 3);
        assert!(turn.final_text().is_none());
        turn.finish(TurnState::Done);
        assert_eq!(turn.final_text(), Some(2));
    }

    #[test]
    fn summary_reads_like_a_sentence() {
        let mut turn = Turn::new("p".into(), Vec::new());
        turn.items.push(tool("1", ToolKind::Write, Some(diff_text("a.py", None, "x\n"))));
        turn.items.push(tool("2", ToolKind::Run, None));
        turn.items.push(tool("3", ToolKind::Read, None));
        turn.items.push(tool("4", ToolKind::List, None));
        assert_eq!(turn.summary(), "Changed 1 file, ran 1 command, and performed 2 other actions");
        let mut reads = Turn::new("p".into(), Vec::new());
        reads.items.push(tool("1", ToolKind::Read, None));
        assert_eq!(reads.summary(), "Performed 1 action");
    }

    #[test]
    fn changes_add_up_per_file() {
        let mut turn = Turn::new("p".into(), Vec::new());
        turn.items.push(tool("1", ToolKind::Write, Some(diff_text("a.py", None, "x\ny\n"))));
        turn.items.push(tool("2", ToolKind::Edit, Some(diff_text("a.py", Some("x\ny\n"), "x\nz\n"))));
        let changes = turn.changes();
        assert_eq!(changes.len(), 1);
        assert_eq!((changes[0].added, changes[0].removed, changes[0].created), (3, 1, true));
    }

    #[test]
    fn unfinished_turns_stop_on_recovery() {
        let mut thread = Thread::new(None, "hi");
        let mut turn = Turn::new("hi".into(), Vec::new());
        turn.items.push(Item::Tool(ToolItem {
            id: "1".into(),
            kind: ToolKind::Run,
            target: "x".into(),
            status: ToolStatus::Running,
            output: String::new(),
            diff: None,
            open: false,
        }));
        thread.turns.push(turn);
        thread.recover();
        assert!(thread.turns[0].state == TurnState::Stopped);
        assert!(matches!(&thread.turns[0].items[0], Item::Tool(t) if t.status == ToolStatus::Failed));
    }

    #[test]
    fn interrupted_tool_calls_get_answers() {
        let mut history = vec![
            Message::user("go"),
            Message::assistant(
                "",
                vec![quill_llama::ToolCall::new("a", "read_file", "{}"), quill_llama::ToolCall::new("b", "list_files", "{}")],
            ),
            Message::tool("a", "ok"),
        ];
        repair_history(&mut history);
        assert_eq!(history.len(), 4);
        assert_eq!(history[3].tool_call_id.as_deref(), Some("b"));
        repair_history(&mut history);
        assert_eq!(history.len(), 4);
    }

    #[test]
    fn titles_and_ages_read_naturally() {
        assert_eq!(title_from("Fix the build\nplease"), "Fix the build");
        assert_eq!(ago(now_ms()), "now");
        assert_eq!(ago(now_ms() - 5 * 60_000), "5m");
    }
}