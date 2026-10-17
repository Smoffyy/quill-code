mod args;
mod editorconfig;
mod fs;
mod git;
mod paths;
mod plan;
mod shell;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use quill_providers::ToolSpec;
use serde_json::Value;

use crate::diff::FileDiff;
use crate::options::{Limits, Toolsets};
use crate::policy::Effect;
use crate::protocol::{Approval, Category, PlanStep, Question, ToolView};

pub use args::{invalid, parse as parse_args};
pub use git::asks_for_git;
pub use shell::{run as run_command, shell_command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolset {
    Core,
    Git,
}

pub struct Spec {
    pub name: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub aliases: &'static [&'static str],
    pub toolset: Toolset,
    pub category: Category,
    pub active: &'static str,
    pub done: &'static str,
    pub parallel: bool,
    pub description: &'static str,
    pub guide: &'static str,
}

pub trait Host: Sync {
    fn output(&self, chunk: &str);
    fn plan(&self, steps: Vec<PlanStep>, explanation: Option<String>);
    fn ask(&self, questions: &[Question]) -> Option<Vec<String>>;
}

pub struct Ctx<'a> {
    pub root: &'a Path,
    pub limits: &'a Limits,
    pub cancel: &'a AtomicBool,
    pub host: &'a dyn Host,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub ok: bool,
    pub text: String,
    pub diffs: Vec<FileDiff>,
    pub image: Option<String>,
}

impl Output {
    pub fn ok(text: impl Into<String>) -> Output {
        Output { ok: true, text: text.into(), diffs: Vec::new(), image: None }
    }

    pub fn fail(text: impl Into<String>) -> Output {
        Output { ok: false, text: text.into(), diffs: Vec::new(), image: None }
    }

    pub fn settle(result: Result<String, String>) -> Output {
        match result {
            Ok(text) => Output::ok(text),
            Err(error) => Output::fail(error),
        }
    }
}

pub type Action = Box<dyn FnOnce(&Ctx<'_>) -> Output + Send>;

pub struct Prepared {
    pub effect: Effect,
    pub subject: String,
    pub approval: Approval,
    pub action: Action,
}

impl Prepared {
    pub fn new(effect: Effect, subject: impl Into<String>, title: &str, action: Action) -> Prepared {
        let subject = subject.into();
        Prepared {
            effect,
            approval: Approval { title: title.into(), subject: subject.clone(), diffs: Vec::new(), similar: None },
            subject,
            action,
        }
    }

    pub fn read(action: Action) -> Prepared {
        Prepared::new(Effect::Read, "", "", action)
    }
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> &'static Spec;
    fn parameters(&self) -> Value;
    fn target(&self, args: &Value) -> String;
    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String>;
}

pub fn all() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(fs::FindFiles),
        Box::new(fs::Grep),
        Box::new(fs::ReadFile),
        Box::new(fs::WriteFile),
        Box::new(fs::EditFile),
        Box::new(shell::ShellTool),
        Box::new(plan::UpdatePlan),
        Box::new(plan::AskUser),
        Box::new(git::CommitChanges),
        Box::new(git::OpenPullRequest),
    ]
}

pub fn enabled(toolsets: Toolsets, limits: &Limits) -> Vec<Box<dyn Tool>> {
    all()
        .into_iter()
        .filter(|tool| {
            let spec = tool.spec();
            let on = match spec.toolset {
                Toolset::Core => true,
                Toolset::Git => toolsets.git,
            };
            on && !limits.disabled_tools.iter().any(|name| name == spec.name)
        })
        .collect()
}

fn normalize(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

pub fn find<'a>(tools: &'a [Box<dyn Tool>], name: &str) -> Option<&'a dyn Tool> {
    let name = normalize(name);
    let name = name.rsplit('.').next().unwrap_or(&name);
    tools.iter().map(|tool| tool.as_ref()).find(|tool| {
        let spec = tool.spec();
        spec.name == name || spec.aliases.contains(&name)
    })
}

pub fn spec_for(tool: &dyn Tool) -> ToolSpec {
    let spec = tool.spec();
    ToolSpec { name: spec.name.into(), description: spec.description.into(), parameters: tool.parameters() }
}

pub fn view(tool: &dyn Tool, id: &str, args: &Value) -> ToolView {
    let spec = tool.spec();
    ToolView {
        id: id.into(),
        tool: spec.name.into(),
        category: spec.category,
        active: spec.active.into(),
        done: spec.done.into(),
        target: tool.target(args),
    }
}

pub fn unknown_view(id: &str, name: &str) -> ToolView {
    ToolView {
        id: id.into(),
        tool: name.into(),
        category: Category::Other,
        active: "Calling".into(),
        done: "Called".into(),
        target: name.into(),
    }
}

pub fn title(name: &str) -> String {
    all().iter().find(|tool| tool.spec().name == name).map_or_else(|| name.replace('_', " "), |tool| tool.spec().title.to_string())
}

pub(crate) fn file_of(name: &str, arguments: &str) -> Option<(String, bool)> {
    let tools = all();
    let spec = find(&tools, name)?.spec();
    let changed = match spec.category {
        Category::Edit => true,
        _ if spec.name == "read_file" => false,
        _ => return None,
    };
    let path = args::trimmed(&args::parse(arguments), &args::PATH)?.replace('\\', "/");
    Some((path, changed))
}

pub(crate) fn plan_of(name: &str, arguments: &str) -> Option<Vec<PlanStep>> {
    let tools = all();
    (find(&tools, name)?.spec().name == "update_plan").then(|| plan::steps(&args::parse(arguments)))
}

pub fn names(tools: &[Box<dyn Tool>]) -> String {
    tools.iter().map(|tool| tool.spec().name).collect::<Vec<_>>().join(", ")
}

pub fn legacy(kind: &str) -> ToolView {
    let name = match kind {
        "List" => "list_dir",
        "Read" => "read_file",
        "Search" => "grep",
        "Write" => "write_file",
        "Edit" => "edit_file",
        "Run" => "shell",
        "Commit" => "commit_changes",
        "PullRequest" => "open_pull_request",
        other => other,
    };
    if let Some((tool, category, active, done)) = RETIRED.iter().find(|(tool, ..)| *tool == name) {
        return ToolView {
            id: String::new(),
            tool: tool.to_string(),
            category: *category,
            active: active.to_string(),
            done: done.to_string(),
            target: String::new(),
        };
    }
    match all().iter().find(|tool| tool.spec().name == name) {
        Some(tool) => view(tool.as_ref(), "", &Value::Null),
        None => unknown_view("", name),
    }
}

const RETIRED: [(&str, Category, &str, &str); 2] =
    [("list_dir", Category::Explore, "Looking through", "Looked through"), ("apply_patch", Category::Edit, "Patching", "Patched")];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_aliases_resolve() {
        let tools = all();
        assert_eq!(find(&tools, "bash").map(|t| t.spec().name), Some("shell"));
        assert_eq!(find(&tools, "str_replace").map(|t| t.spec().name), Some("edit_file"));
        assert_eq!(find(&tools, "Read-File").map(|t| t.spec().name), Some("read_file"));
        assert_eq!(find(&tools, "functions.grep").map(|t| t.spec().name), Some("grep"));
        assert!(find(&tools, "teleport").is_none());
    }

    #[test]
    fn toolsets_and_disabled_tools_filter_the_list() {
        let without_git = enabled(Toolsets { git: false }, &Limits::default());
        assert!(find(&without_git, "commit_changes").is_none());
        let limits = Limits { disabled_tools: vec!["shell".into()], ..Limits::default() };
        assert!(find(&enabled(Toolsets::default(), &limits), "shell").is_none());
    }

    #[test]
    fn every_name_and_alias_is_unique() {
        let tools = all();
        let mut seen = Vec::new();
        for tool in &tools {
            for name in std::iter::once(&tool.spec().name).chain(tool.spec().aliases) {
                assert!(!seen.contains(name), "{name} appears twice");
                seen.push(*name);
            }
        }
    }

    #[test]
    fn old_tool_kinds_map_to_new_tools() {
        assert_eq!(legacy("Run").tool, "shell");
        assert_eq!(legacy("Search").category, Category::Explore);
        assert_eq!(legacy("List").category, Category::Explore);
        assert_eq!(legacy("apply_patch").category, Category::Edit);
        assert_eq!(legacy("Mystery").category, Category::Other);
    }
}