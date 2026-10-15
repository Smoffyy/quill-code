use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use quill_core::tools::legacy;
use quill_core::{Category, ClaudeLink, FileDiff, PlanStep, ToolView};
use quill_providers::{Message, Tokens};
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
#[serde(from = "RawTool")]
pub struct ToolItem {
    pub id: String,
    pub tool: String,
    pub category: Category,
    pub active: String,
    pub done: String,
    pub target: String,
    pub status: ToolStatus,
    pub output: String,
    pub diffs: Vec<FileDiff>,
    pub image: Option<String>,
    pub started: u64,
    pub ended: Option<u64>,
}

#[derive(Deserialize)]
struct RawTool {
    id: String,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    category: Option<Category>,
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    done: Option<String>,
    #[serde(default)]
    target: String,
    status: ToolStatus,
    #[serde(default)]
    output: String,
    #[serde(default)]
    diffs: Vec<FileDiff>,
    #[serde(default)]
    diff: Option<FileDiff>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    started: u64,
    #[serde(default)]
    ended: Option<u64>,
}

impl From<RawTool> for ToolItem {
    fn from(raw: RawTool) -> ToolItem {
        let view = match (raw.tool, raw.category, raw.active, raw.done) {
            (Some(tool), Some(category), Some(active), Some(done)) => {
                ToolView { id: raw.id.clone(), tool, category, active, done, target: String::new() }
            }
            (tool, ..) => legacy(&tool.or(raw.kind).unwrap_or_default()),
        };
        let diffs = if raw.diffs.is_empty() { raw.diff.into_iter().collect() } else { raw.diffs };
        ToolItem {
            id: raw.id,
            tool: view.tool,
            category: view.category,
            active: view.active,
            done: view.done,
            target: raw.target,
            status: raw.status,
            output: raw.output,
            diffs,
            image: raw.image,
            started: raw.started,
            ended: raw.ended,
        }
    }
}

impl ToolItem {
    pub fn new(view: ToolView) -> ToolItem {
        ToolItem {
            id: view.id,
            tool: view.tool,
            category: view.category,
            active: view.active,
            done: view.done,
            target: view.target,
            status: ToolStatus::Running,
            output: String::new(),
            diffs: Vec::new(),
            image: None,
            started: now_ms(),
            ended: None,
        }
    }

    pub fn working(&self) -> bool {
        matches!(self.status, ToolStatus::Running | ToolStatus::Waiting)
    }

    pub fn seconds(&self) -> Option<f32> {
        let ended = self.ended?;
        (self.started > 0 && ended >= self.started).then(|| (ended - self.started) as f32 / 1000.0)
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.output.lines().next()?.strip_prefix("Exit code ")?.trim().parse().ok()
    }

    pub fn verb(&self) -> &str {
        if self.working() {
            &self.active
        } else if self.diffs.len() == 1 && self.diffs[0].created {
            "Created"
        } else {
            &self.done
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Item {
    Reasoning(String),
    Text(String),
    Tool(ToolItem),
    Plan { steps: Vec<PlanStep>, explanation: Option<String> },
    Question { id: String, question: String, options: Vec<String>, answer: Option<String> },
    Steer(String),
    Note(String),
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
    #[serde(default)]
    pub cost: Option<f64>,
    #[serde(default)]
    pub usage: Tokens,
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
            cost: None,
            usage: Tokens::default(),
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

    pub fn set_plan(&mut self, steps: Vec<PlanStep>, explanation: Option<String>) {
        let found = self.items.iter_mut().find_map(|item| match item {
            Item::Plan { steps, explanation } => Some((steps, explanation)),
            _ => None,
        });
        match found {
            Some((old_steps, old_explanation)) => {
                *old_steps = steps;
                *old_explanation = explanation;
            }
            None => self.items.push(Item::Plan { steps, explanation }),
        }
    }

    pub fn tool_mut(&mut self, id: &str) -> Option<&mut ToolItem> {
        self.items.iter_mut().rev().find_map(|item| match item {
            Item::Tool(tool) if tool.id == id => Some(tool),
            _ => None,
        })
    }

    pub fn answer(&mut self, id: &str, text: &str) {
        for item in &mut self.items {
            if let Item::Question { id: question, answer, .. } = item
                && question == id
            {
                *answer = Some(text.to_string());
            }
        }
    }

    pub fn finish(&mut self, state: TurnState) {
        if !self.working() {
            return;
        }
        self.worked_ms = Some(now_ms().saturating_sub(self.started));
        for item in &mut self.items {
            if let Item::Tool(tool) = item
                && tool.working()
            {
                tool.status = ToolStatus::Failed;
            }
        }
        if state == TurnState::Done {
            self.promote_reasoning();
        }
        self.state = state;
    }

    fn promote_reasoning(&mut self) {
        let last = self.items.iter_mut().rev().find(|item| !matches!(item, Item::Text(t) if t.trim().is_empty()));
        if let Some(item) = last
            && let Item::Reasoning(text) = item
        {
            let text = std::mem::take(text).trim().to_string();
            *item = Item::Text(text);
        }
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
        self.items
            .iter()
            .filter_map(|item| match item {
                Item::Tool(ToolItem { diffs, status: ToolStatus::Done, .. }) => Some(diffs.iter()),
                _ => None,
            })
            .flatten()
    }

    pub fn summary(&self) -> String {
        let files = self.changes().len();
        let (mut commands, mut reads, mut searches, mut other, mut asked) = (0, 0, 0, 0, 0);
        for item in &self.items {
            if let Item::Tool(tool) = item {
                match (tool.category, tool.tool.as_str()) {
                    (Category::Command, _) => commands += 1,
                    (Category::Explore, "read_file") => reads += 1,
                    (Category::Explore, _) => searches += 1,
                    (Category::Question, _) => asked += 1,
                    (Category::Edit | Category::Plan, _) => {}
                    _ => other += 1,
                }
            }
        }
        let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = Vec::new();
        if files > 0 {
            parts.push(format!("changed {}", count(files, "file", "files")));
        }
        if commands > 0 {
            parts.push(format!("ran {}", count(commands, "command", "commands")));
        }
        if reads > 0 {
            parts.push(format!("read {}", count(reads, "file", "files")));
        }
        if searches > 0 {
            parts.push(format!("searched {}", count(searches, "time", "times")));
        }
        if asked > 0 {
            parts.push(format!("asked you {}", count(asked, "question", "questions")));
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
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub claude: Option<ClaudeLink>,
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
            context_window: None,
            provider: None,
            claude: None,
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
    use quill_core::{StepStatus, diff_text};

    fn tool(id: &str, name: &str, diffs: Vec<FileDiff>) -> Item {
        let mut item = ToolItem::new(legacy(name));
        item.id = id.into();
        item.target = "x".into();
        item.status = ToolStatus::Done;
        item.diffs = diffs;
        Item::Tool(item)
    }

    #[test]
    fn streamed_text_joins_until_a_tool_runs() {
        let mut turn = Turn::new("p".into(), Vec::new());
        turn.push_text("Hel");
        turn.push_text("lo");
        turn.items.push(tool("1", "read_file", Vec::new()));
        turn.push_text("Done");
        assert_eq!(turn.items.len(), 3);
        assert!(turn.final_text().is_none());
        turn.finish(TurnState::Done);
        assert_eq!(turn.final_text(), Some(2));
    }

    #[test]
    fn reasoning_that_ends_a_turn_becomes_the_answer() {
        let mut turn = Turn::new("p".into(), Vec::new());
        turn.push_reasoning("Checking");
        turn.items.push(tool("1", "shell", Vec::new()));
        turn.push_reasoning("All updated.");
        turn.push_text("\n");
        turn.finish(TurnState::Done);
        assert_eq!(turn.final_text(), Some(2));
        assert!(matches!(&turn.items[0], Item::Reasoning(_)));
        assert!(matches!(&turn.items[2], Item::Text(t) if t == "All updated."));
    }

    #[test]
    fn summary_reads_like_a_sentence() {
        let mut turn = Turn::new("p".into(), Vec::new());
        turn.items.push(tool("1", "write_file", vec![diff_text("a.py", None, "x\n")]));
        turn.items.push(tool("2", "shell", Vec::new()));
        turn.items.push(tool("3", "read_file", Vec::new()));
        turn.items.push(tool("4", "list_dir", Vec::new()));
        assert_eq!(turn.summary(), "Changed 1 file, ran 1 command, read 1 file, and searched 1 time");
        let mut reads = Turn::new("p".into(), Vec::new());
        reads.items.push(tool("1", "read_file", Vec::new()));
        assert_eq!(reads.summary(), "Read 1 file");
    }

    #[test]
    fn changes_add_up_per_file() {
        let mut turn = Turn::new("p".into(), Vec::new());
        turn.items.push(tool("1", "write_file", vec![diff_text("a.py", None, "x\ny\n")]));
        turn.items.push(tool("2", "edit_file", vec![diff_text("a.py", Some("x\ny\n"), "x\nz\n")]));
        let changes = turn.changes();
        assert_eq!(changes.len(), 1);
        assert_eq!((changes[0].added, changes[0].removed, changes[0].created), (3, 1, true));
    }

    #[test]
    fn plans_update_in_place() {
        let mut turn = Turn::new("p".into(), Vec::new());
        let step = |status| vec![PlanStep { text: "a".into(), status }];
        turn.set_plan(step(StepStatus::Active), None);
        turn.push_text("working");
        turn.set_plan(step(StepStatus::Done), Some("finished".into()));
        assert_eq!(turn.items.len(), 2);
        assert!(matches!(&turn.items[0], Item::Plan { steps, .. } if steps[0].status == StepStatus::Done));
    }

    #[test]
    fn unfinished_turns_stop_on_recovery() {
        let mut thread = Thread::new(None, "hi");
        let mut turn = Turn::new("hi".into(), Vec::new());
        let mut running = ToolItem::new(legacy("shell"));
        running.id = "1".into();
        turn.items.push(Item::Tool(running));
        thread.turns.push(turn);
        thread.recover();
        assert!(thread.turns[0].state == TurnState::Stopped);
        assert!(matches!(&thread.turns[0].items[0], Item::Tool(t) if t.status == ToolStatus::Failed));
    }

    #[test]
    fn version_one_threads_still_load() {
        let old = r#"{"Tool":{"id":"c1","kind":"Run","target":"npm test","status":"Done","output":"ok","diff":null}}"#;
        let Item::Tool(tool) = serde_json::from_str::<Item>(old).unwrap() else { panic!("a tool item") };
        assert_eq!((tool.tool.as_str(), tool.category, tool.target.as_str()), ("shell", Category::Command, "npm test"));
        let saved = serde_json::to_string(&Item::Tool(tool)).unwrap();
        let Item::Tool(again) = serde_json::from_str::<Item>(&saved).unwrap() else { panic!("a tool item") };
        assert_eq!(again.tool, "shell");
    }

    #[test]
    fn interrupted_tool_calls_get_answers() {
        let mut history = vec![
            Message::user("go"),
            Message::assistant(
                "",
                vec![quill_providers::ToolCall::new("a", "read_file", "{}"), quill_providers::ToolCall::new("b", "list_files", "{}")],
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