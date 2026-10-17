use std::path::Path;

use crate::instructions::project_instructions;
use crate::options::{FinalNewline, Shell};
use crate::tools::{Tool, Toolset};

const POWERSHELL: &str = "# Commands

shell runs Windows PowerShell, so write PowerShell, not bash or cmd:
- Use cmdlets such as Get-ChildItem, Get-Content, Select-String, New-Item, Copy-Item, Move-Item and Remove-Item. Do not use ls -la, grep, cat with flags, rm -rf, touch or export.
- Separate commands with a semicolon. Set environment variables with $env:NAME = 'value'.
- Paths use backslashes, and paths with spaces need quotes.
- Run programs installed on the computer, such as python, node, npm, cargo, dotnet or go, by name.";

const CMD: &str = "# Commands

shell runs the Windows Command Prompt, so write cmd syntax. Chain commands with &&, set variables with set NAME=value and quote paths that contain spaces.";

const POSIX: &str = "# Commands

shell runs a POSIX shell, so write sh syntax. Combine commands with && and quote paths that contain spaces. Run programs installed on the computer by name.";

const COMMAND_RULES: &str = "Commands must finish on their own. Pass flags that skip prompts, such as -y, and do not start servers, watchers or anything else that waits forever; a command is stopped when its timeout runs out. To look at files, use find_files, grep and read_file rather than shell commands. For anything else, use the system's own commands and the programs installed on the computer. Never write Python or other scripts to read or change files; change files with edit_file and write_file.";

const WORKFLOW: &str = "# How to work

1. Understand the request. If it is unclear in a way that changes the result, ask with ask_user. Otherwise make a sensible choice and mention it.
2. For work with more than two steps, write a short plan with update_plan, keep one step in_progress, and update it as you go.
3. Explore before changing anything: find the relevant files and read them.
4. Make focused changes that follow the project's existing style, naming and structure. Do not reformat or rewrite code the task does not touch.
5. Check your work: build, run or test the code with shell when the project allows it, and fix whatever broke.
6. When a tool call fails, read the error and change your approach. Never repeat a call that failed in exactly the same way.
7. When the user declines a change or a command, do not try it again in another form. Ask what they would like instead.

Keep going until the task is done. Do not stop between ordinary steps to ask for permission; the app asks the user when it needs to.";

const GIT: &str = "# Git

The project uses Git. To commit or open a pull request, use commit_changes and open_pull_request rather than running git yourself. Commit or open a pull request only when the user asks for it. Put work meant for a pull request on a new branch with a short, descriptive name such as fix-login. Write commit messages as one short summary line in the imperative mood, followed by details only when they help.";

const KEEP_NEWLINE: &str = "A file keeps its final newline, or its lack of one, unless you change it; read_file says when a file ends with a newline. To remove a final newline, call edit_file with old_text set to the last two lines followed by the line break after them, and new_text set to the same two lines without it. To add one, do the opposite. Never rewrite the file for this, and never join lines or delete any other line break.";

const REMOVE_NEWLINE: &str = "Quill removes the final newline from every file you write or edit, so files end right after their last character; files that need one, such as patches, keep it. Never join lines or delete line breaks to do that yourself. To remove the final newline from a file without changing anything else, call edit_file with its last line as both old_text and new_text.";

const ADD_NEWLINE: &str = "Quill ends every file you write or edit with exactly one newline, so never add or remove one yourself.";

const REPLY: &str = "# Your reply

When you finish, reply with a short summary of what you changed and how you checked it. The user sees every tool call and diff, so do not paste file contents back. Use Markdown, with fenced code blocks for code. Be direct and concise.";

pub struct PromptContext<'a> {
    pub model: &'a str,
    pub root: Option<&'a Path>,
    pub tools: &'a [Box<dyn Tool>],
    pub shell: Shell,
    pub personal: &'a str,
    pub instructions: &'a [String],
    pub final_newline: FinalNewline,
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

pub fn system_prompt(ctx: &PromptContext<'_>) -> String {
    let mut sections = vec![format!(
        "You are Quill, the coding agent built into Quill Code. You run on the user's own {} computer and are powered by the {} model. If the user asks which model you are, say so.",
        os_name(),
        ctx.model
    )];
    match ctx.root {
        Some(root) => {
            sections.push(format!(
                "# Your workspace\n\nYou work in the project folder {}. Paths you give to tools are relative to it. You change files only inside it; reading elsewhere may need the user's OK. Use your tools to do the work rather than describing what you would do.",
                root.display()
            ));
            let lines: Vec<String> = ctx.tools.iter().map(|tool| format!("- {}", tool.spec().guide)).collect();
            let newline = match ctx.final_newline {
                FinalNewline::Keep => KEEP_NEWLINE,
                FinalNewline::Remove => REMOVE_NEWLINE,
                FinalNewline::Add => ADD_NEWLINE,
            };
            sections.push(format!("# Your tools\n\n{}\n\n{newline}", lines.join("\n")));
            let shell = match ctx.shell {
                Shell::PowerShell | Shell::Pwsh => POWERSHELL,
                Shell::Cmd => CMD,
                Shell::Bash | Shell::Sh => POSIX,
            };
            sections.push(format!("{shell}\n\n{COMMAND_RULES}"));
            sections.push(WORKFLOW.to_string());
            if ctx.tools.iter().any(|tool| tool.spec().toolset == Toolset::Git) {
                sections.push(GIT.to_string());
            }
            sections.push(REPLY.to_string());
            let project = project_instructions(root, ctx.instructions);
            if !project.is_empty() {
                sections.push(format!("# Project instructions\n\n{project}"));
            }
        }
        None => sections.push(
            "No project is open in this conversation, so you have no tools. Answer from what you know, and when the user wants you to work on their files, suggest running Quill Code in the project folder. Use Markdown, with fenced code blocks for code. Be direct and concise.".into(),
        ),
    }
    let personal = ctx.personal.trim();
    if !personal.is_empty() {
        sections.push(format!("# The user's instructions\n\n{personal}"));
    }
    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{Limits, Toolsets};
    use crate::tools::enabled;

    fn prompt(root: Option<&Path>, git: bool, personal: &str) -> String {
        let tools = enabled(Toolsets { git }, &Limits::default());
        let instructions = vec!["AGENTS.md".to_string()];
        system_prompt(&PromptContext {
            model: "qwen",
            root,
            tools: &tools,
            shell: Shell::PowerShell,
            personal,
            instructions: &instructions,
            final_newline: FinalNewline::Keep,
        })
    }

    #[test]
    fn quill_is_the_assistant_whatever_the_model() {
        let text = prompt(None, false, "");
        assert!(text.starts_with("You are Quill"));
        assert!(text.contains("powered by the qwen model"));
        assert!(text.contains("no tools"));
    }

    #[test]
    fn projects_get_tools_shell_and_workflow() {
        let dir = std::env::temp_dir();
        let text = prompt(Some(&dir), false, "");
        assert!(text.contains("edit_file") && text.contains("read_file") && text.contains("# How to work"));
        assert!(text.contains("Windows PowerShell"));
        assert!(text.contains("A file keeps its final newline"));
        assert!(!text.contains("open_pull_request"));
        assert!(prompt(Some(&dir), true, "").contains("open_pull_request"));
        assert!(prompt(None, false, "Use tabs.").ends_with("# The user's instructions\n\nUse tabs."));
    }
}