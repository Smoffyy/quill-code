use serde_json::{Value, json};

use super::args;
use super::{Ctx, Output, Prepared, Spec, Tool, Toolset};
use crate::protocol::{Category, PlanStep, StepStatus};

pub struct UpdatePlan;

const PLAN: Spec = Spec {
    name: "update_plan",
    title: "Plan",
    summary: "Shows a checklist of the work and keeps it current.",
    aliases: &["plan", "todo", "todo_write", "todowrite", "set_plan", "update_todos", "write_todos"],
    toolset: Toolset::Core,
    category: Category::Plan,
    active: "Planning",
    done: "Updated the plan",
    parallel: false,
    description: "Show the user your plan as a short checklist and keep it current. Send the whole list each time, with each step pending, in_progress or completed. Keep exactly one step in_progress while you work.",
    guide: "update_plan shows your plan as a checklist. For work with several steps, set a plan first and update it as each step finishes.",
};

fn status(value: &str) -> StepStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "in_progress" | "in-progress" | "active" | "doing" | "current" | "started" => StepStatus::Active,
        "completed" | "complete" | "done" | "finished" => StepStatus::Done,
        _ => StepStatus::Pending,
    }
}

fn steps(args: &Value) -> Vec<PlanStep> {
    let list = ["plan", "steps", "items", "todos"].iter().find_map(|key| args.get(*key).and_then(Value::as_array));
    list.map(|items| {
        items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(PlanStep { text: text.trim().to_string(), status: StepStatus::Pending }),
                _ => {
                    let text = args::trimmed(item, &["step", "text", "content", "title", "task"])?;
                    let state = args::text(item, &["status", "state"]).map_or(StepStatus::Pending, status);
                    Some(PlanStep { text: text.to_string(), status: state })
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

impl Tool for UpdatePlan {
    fn spec(&self) -> &'static Spec {
        &PLAN
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "explanation": { "type": "string", "description": "Optional note on what changed." },
                "plan": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": { "type": "string" },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                        },
                        "required": ["step", "status"]
                    }
                }
            },
            "required": ["plan"]
        })
    }

    fn target(&self, args: &Value) -> String {
        let steps = steps(args);
        let done = steps.iter().filter(|step| step.status == StepStatus::Done).count();
        if steps.is_empty() { String::new() } else { format!("{done} of {} done", steps.len()) }
    }

    fn prepare(&self, args: &Value, _: &Ctx<'_>) -> Result<Prepared, String> {
        let steps = steps(args);
        if steps.is_empty() {
            return Err("update_plan needs \"plan\": a list of {step, status} items.".into());
        }
        let explanation = args::trimmed(args, &["explanation", "note"]).map(str::to_string);
        Ok(Prepared::read(Box::new(move |ctx: &Ctx<'_>| {
            ctx.host.plan(steps, explanation);
            Output::ok("Plan updated.")
        })))
    }
}

pub struct AskUser;

const ASK: Spec = Spec {
    name: "ask_user",
    title: "Ask you",
    summary: "Asks you a question and waits for the answer.",
    aliases: &["ask", "ask_question", "question", "ask_followup_question", "request_user_input"],
    toolset: Toolset::Core,
    category: Category::Question,
    active: "Asking",
    done: "Asked",
    parallel: false,
    description: "Ask the user a question and wait for the answer. Use it only when you cannot continue without a decision from them. Offer short options when there are clear choices.",
    guide: "ask_user asks the user a question and waits. Use it only when you are blocked on a decision that is theirs to make.",
};

impl Tool for AskUser {
    fn spec(&self) -> &'static Spec {
        &ASK
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "The question to ask." },
                "options": { "type": "array", "items": { "type": "string" }, "description": "Optional short answers to choose from." }
            },
            "required": ["question"]
        })
    }

    fn target(&self, args: &Value) -> String {
        args::trimmed(args, &["question", "text", "prompt"]).unwrap_or_default().to_string()
    }

    fn prepare(&self, args: &Value, _: &Ctx<'_>) -> Result<Prepared, String> {
        let question = args::trimmed(args, &["question", "text", "prompt"]).ok_or("ask_user needs a \"question\".")?.to_string();
        let options: Vec<String> = ["options", "choices"]
            .iter()
            .find_map(|key| args.get(*key).and_then(Value::as_array))
            .map(|items| items.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        Ok(Prepared::read(Box::new(move |ctx: &Ctx<'_>| match ctx.host.ask(&question, &options) {
            Some(answer) => Output::ok(format!("The user answered: {answer}")),
            None => Output::fail("The user did not answer."),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_accept_many_shapes() {
        let codex = json!({ "plan": [{ "step": "Read", "status": "completed" }, { "step": "Fix", "status": "in_progress" }, { "step": "Test", "status": "pending" }] });
        let parsed = steps(&codex);
        assert_eq!(parsed.iter().map(|s| s.status).collect::<Vec<_>>(), vec![StepStatus::Done, StepStatus::Active, StepStatus::Pending]);
        assert_eq!(UpdatePlan.target(&codex), "1 of 3 done");
        let loose = json!({ "todos": ["one", { "content": "two", "state": "done" }] });
        assert_eq!(steps(&loose).len(), 2);
        assert_eq!(steps(&loose)[1].status, StepStatus::Done);
    }
}