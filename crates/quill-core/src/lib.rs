mod claude_code;
mod context;
pub mod diff;
mod instructions;
mod options;
mod policy;
mod prompt;
mod protocol;
mod session;
pub mod tools;
mod turn;

pub use claude_code::{ClaudeCode, claude_version, complete as claude_complete, find_claude};
pub use diff::{DiffLine, FileDiff, LineKind, diff_text};
pub use instructions::project_instructions;
pub use options::{Limits, Shell, Toolsets};
pub use policy::{Access, Effect, Rule, RuleAction, Verdict, read_only, verdict};
pub use protocol::{Approval, Category, ClaudeLink, Decision, Event, Outcome, PlanStep, StepStatus, ToolView};
pub use session::{Engine, Input, Op, SessionHandle, Summarizer, TurnContext, spawn};