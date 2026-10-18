use quill_providers::{Message, Tokens};
use serde::{Deserialize, Serialize};

use crate::diff::FileDiff;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Explore,
    Edit,
    Command,
    Plan,
    Question,
    Git,
    Web,
    Desktop,
    Agent,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolView {
    pub id: String,
    pub tool: String,
    pub category: Category,
    pub active: String,
    pub done: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub title: String,
    pub subject: String,
    #[serde(default)]
    pub diffs: Vec<FileDiff>,
    #[serde(default)]
    pub similar: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    Decline,
    AllowSimilar,
    AllowTool,
    AllowAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Active,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub text: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "error", rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Stopped,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    TurnStarted { turn: usize },
    Reasoning { turn: usize, text: String },
    Text { turn: usize, text: String },
    ToolBegin { turn: usize, view: ToolView },
    ToolOutput { turn: usize, id: String, chunk: String },
    ToolEnd { turn: usize, id: String, ok: bool, output: String, diffs: Vec<FileDiff>, image: Option<String> },
    ApprovalRequested { turn: usize, id: String, approval: Approval },
    QuestionAsked { turn: usize, id: String, questions: Vec<Question> },
    PlanUpdated { turn: usize, steps: Vec<PlanStep>, explanation: Option<String> },
    Note { turn: usize, text: String },
    Message { message: Message },
    Compacting,
    Compacted { history: Vec<Message> },
    CompactFailed { reason: String },
    Usage { turn: usize, tokens: Tokens },
    TurnFinished { turn: usize, outcome: Outcome },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_as_tagged_json() {
        let event = Event::TurnFinished { turn: 2, outcome: Outcome::Failed("boom".into()) };
        let text = serde_json::to_string(&event).unwrap();
        assert!(text.contains("\"type\":\"turn_finished\"") && text.contains("\"outcome\":\"failed\""));
        assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), event);
        let message = Event::Message { message: Message::user("hi") };
        assert_eq!(serde_json::from_str::<Event>(&serde_json::to_string(&message).unwrap()).unwrap(), message);
    }
}