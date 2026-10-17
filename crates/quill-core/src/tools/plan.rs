use serde_json::{Value, json};

use super::args;
use super::{Ctx, Output, Prepared, Spec, Tool, Toolset};
use crate::protocol::{Category, PlanStep, Question, StepStatus};

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

pub(crate) fn steps(args: &Value) -> Vec<PlanStep> {
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
    description: "Ask the user one to four questions and wait for the answers. Use it only when you cannot continue without decisions from them. Put each choice in options, never as a list inside the question text: the user picks one with the arrow keys or types their own answer.",
    guide: "ask_user asks the user up to four questions at once and waits. Use it only when you are blocked on decisions that are theirs to make, and give each question short options.",
};

fn question(value: &Value) -> Option<Question> {
    let question = args::trimmed(value, &["question", "text", "prompt"])?.to_string();
    let options = ["options", "choices"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_array))
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().or_else(|| item["label"].as_str()))
                .map(|option| option.trim().to_string())
                .filter(|option| !option.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Some(Question { question, options })
}

fn questions(args: &Value) -> Vec<Question> {
    match args.get("questions").and_then(Value::as_array) {
        Some(items) => items.iter().filter_map(question).collect(),
        None => question(args).into_iter().collect(),
    }
}

fn reply(question: &Question, answer: &str) -> String {
    if question.options.is_empty() || question.options.iter().any(|option| option == answer) {
        answer.to_string()
    } else {
        format!("{answer} (in their own words, not one of the options)")
    }
}

fn answered(questions: &[Question], answers: &[String]) -> String {
    if let ([question], [answer]) = (questions, answers) {
        return format!("The user answered: {}", reply(question, answer));
    }
    let pairs: Vec<String> = questions
        .iter()
        .zip(answers)
        .map(|(question, answer)| format!("{}\nAnswer: {}", question.question, reply(question, answer)))
        .collect();
    format!("The user answered:\n\n{}", pairs.join("\n\n"))
}

impl Tool for AskUser {
    fn spec(&self) -> &'static Spec {
        &ASK
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "One to four questions, asked together.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string", "description": "The question to ask." },
                            "options": { "type": "array", "items": { "type": "string" }, "description": "Two to four short answers to choose from. The user can always type their own." }
                        },
                        "required": ["question"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    fn target(&self, args: &Value) -> String {
        questions(args).first().map(|question| question.question.clone()).unwrap_or_default()
    }

    fn prepare(&self, args: &Value, _: &Ctx<'_>) -> Result<Prepared, String> {
        let questions = questions(args);
        if questions.is_empty() {
            return Err("ask_user needs \"questions\": a list of objects with a \"question\" and its \"options\".".into());
        }
        Ok(Prepared::read(Box::new(move |ctx: &Ctx<'_>| match ctx.host.ask(&questions) {
            Some(answers) => Output::ok(answered(&questions, &answers)),
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

    #[test]
    fn questions_come_one_or_several_at_a_time() {
        let one = json!({ "question": "Which?", "options": ["a", "b"] });
        assert_eq!(questions(&one), vec![Question { question: "Which?".into(), options: vec!["a".into(), "b".into()] }]);
        let several = json!({ "questions": [{ "question": "Name?" }, { "question": "Tests?", "options": [{ "label": "Yes" }] }] });
        let parsed = questions(&several);
        assert_eq!((parsed.len(), parsed[1].options.clone()), (2, vec!["Yes".to_string()]));
        assert_eq!(answered(&parsed[..1], &["x".into()]), "The user answered: x");
        assert!(answered(&parsed, &["x".into(), "Yes".into()]).contains("Tests?\nAnswer: Yes"));
        assert!(answered(&parsed[1..], &["Maybe".into()]).ends_with("Maybe (in their own words, not one of the options)"));
    }
}