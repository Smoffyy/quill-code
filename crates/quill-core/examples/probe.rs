use std::sync::Arc;
use std::sync::mpsc::channel;

use quill_core::{Access, Decision, Event, Input, Limits, Op, Toolsets, TurnContext, spawn};
use quill_providers::OpenAi;

fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| "http://127.0.0.1:9931".into());
    let model = args.next().unwrap_or_default();
    let prompt = args.next().unwrap_or_else(|| "Create hello.py that prints a greeting, then run it.".into());
    let root = std::env::temp_dir().join("quill-probe");
    std::fs::create_dir_all(&root).expect("probe folder");
    let (tx, rx) = channel();
    let session = spawn(Vec::new(), move |event| {
        let _ = tx.send(event);
    });
    let context = TurnContext {
        provider: Arc::new(OpenAi::new(&url)),
        model,
        summarizer: None,
        tuning: Default::default(),
        context_window: None,
        root: Some(root.clone()),
        access: Access::Full,
        toolsets: Toolsets { git: false },
        personal: String::new(),
        instructions: vec!["AGENTS.md".into()],
        limits: Limits::default(),
        rules: Vec::new(),
    };
    println!("working in {}", root.display());
    session.send(Op::Turn { turn: 0, input: Input { text: prompt, images: Vec::new() }, context, cancel: Arc::default() });
    for event in rx {
        match event {
            Event::Text { text, .. } => print!("{text}"),
            Event::ToolBegin { view, .. } => println!("\n[{}] {}", view.tool, view.target),
            Event::ToolEnd { ok, output, diffs, .. } => {
                println!("  -> {} {} ({} diffs)", if ok { "ok" } else { "failed" }, output.lines().next().unwrap_or_default(), diffs.len())
            }
            Event::ApprovalRequested { id, approval, .. } => {
                println!("\n  approving: {} {}", approval.title, approval.subject);
                session.send(Op::Decide { id, decision: Decision::Approve });
            }
            Event::QuestionAsked { id, questions, .. } => {
                let mut answers = Vec::new();
                for question in questions {
                    println!("\n  question: {} {:?}", question.question, question.options);
                    answers.push(question.options.first().cloned().unwrap_or_else(|| "Yes".into()));
                }
                session.send(Op::Answer { id, answers });
            }
            Event::PlanUpdated { steps, .. } => {
                for step in steps {
                    println!("  plan: {:?} {}", step.status, step.text);
                }
            }
            Event::Usage { tokens, .. } => println!("\n  usage: {} in, {} out", tokens.prompt, tokens.completion),
            Event::TurnFinished { outcome, .. } => {
                println!("\nresult: {outcome:?}");
                break;
            }
            _ => {}
        }
    }
}