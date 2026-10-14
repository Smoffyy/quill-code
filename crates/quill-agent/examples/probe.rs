use std::sync::atomic::AtomicBool;

use quill_agent::{Access, AgentEvent, Task, run, system_prompt};
use quill_llama::Message;

fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| "http://127.0.0.1:9931".into());
    let model = args.next().unwrap_or_default();
    let prompt = args.next().unwrap_or_else(|| "Create hello.py that prints a greeting, then run it.".into());
    let root = std::env::temp_dir().join("quill-probe");
    std::fs::create_dir_all(&root).expect("probe folder");
    let task = Task {
        server_url: url,
        system: system_prompt(&model, Some(&root), false, ""),
        model,
        thinking: Some(false),
        root: Some(root.clone()),
        access: Access::Full,
        history: vec![Message::user(prompt)],
        publisher: None,
    };
    println!("working in {}", root.display());
    let cancel = AtomicBool::new(false);
    let result = run(task, &cancel, |event| match event {
        AgentEvent::Text(text) => print!("{text}"),
        AgentEvent::ToolStarted { kind, target, .. } => println!("\n[{}] {target}", kind.name()),
        AgentEvent::ToolFinished { ok, output, .. } => {
            println!("  -> {} {}", if ok { "ok" } else { "failed" }, output.lines().next().unwrap_or_default())
        }
        _ => {}
    });
    println!("\nresult: {result:?}");
}