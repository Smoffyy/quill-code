use std::sync::Arc;
use std::sync::mpsc::channel;

use quill_core::{Access, ClaudeCode, Decision, Engine, Event, Input, Limits, Op, Toolsets, TurnContext, find_claude, spawn};
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
    let engine = if url == "claude" {
        Engine::ClaudeCode(ClaudeCode { binary: find_claude(None).expect("Claude Code is not installed"), resume: None })
    } else {
        Engine::Model(Arc::new(OpenAi::new(&url)))
    };
    let context = TurnContext {
        engine,
        model,
        summarizer: None,
        thinking: Some(false),
        effort: None,
        max_output: None,
        server_compaction: false,
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
            Event::QuestionAsked { id, question, options, .. } => {
                println!("\n  question: {question} {options:?}");
                session.send(Op::Answer { id, text: options.first().cloned().unwrap_or_else(|| "Yes".into()) });
            }
            Event::PlanUpdated { steps, .. } => {
                for step in steps {
                    println!("  plan: {:?} {}", step.status, step.text);
                }
            }
            Event::Usage { tokens, .. } => println!("\n  usage: {} in, {} out", tokens.prompt, tokens.completion),
            Event::Cost { usd, .. } => println!("  cost: ${usd:.4}"),
            Event::ContextWindow { tokens } => println!("  window: {tokens}"),
            Event::ClaudeSession { link } => println!("  claude session {} at {}", link.session, link.synced),
            Event::TurnFinished { outcome, .. } => {
                println!("\nresult: {outcome:?}");
                break;
            }
            _ => {}
        }
    }
}