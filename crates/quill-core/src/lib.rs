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

pub use context::limit as compact_limit;
pub use diff::{DiffLine, FileDiff, LineKind, diff_text};
pub use instructions::project_instructions;
pub use options::{FinalNewline, Limits, Shell, Toolsets};
pub use policy::{Access, Effect, Rule, RuleAction, Verdict, read_only, verdict};
pub use protocol::{Approval, Category, Decision, Event, Outcome, PlanStep, Question, StepStatus, ToolView};
pub use session::{Input, Op, SessionHandle, Summarizer, TurnContext, spawn};