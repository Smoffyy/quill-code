use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    PowerShell,
    Pwsh,
    Cmd,
    Bash,
    Sh,
}

impl Default for Shell {
    fn default() -> Self {
        if cfg!(windows) { Shell::PowerShell } else { Shell::Sh }
    }
}

impl Shell {
    pub const ALL: [Shell; 5] = [Shell::PowerShell, Shell::Pwsh, Shell::Cmd, Shell::Bash, Shell::Sh];

    pub fn label(self) -> &'static str {
        match self {
            Shell::PowerShell => "Windows PowerShell",
            Shell::Pwsh => "PowerShell 7",
            Shell::Cmd => "Command Prompt",
            Shell::Bash => "Bash",
            Shell::Sh => "sh",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalNewline {
    #[default]
    Keep,
    Remove,
    Add,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toolsets {
    pub git: bool,
}

impl Default for Toolsets {
    fn default() -> Self {
        Toolsets { git: true }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    pub max_steps: usize,
    pub command_timeout: u64,
    pub max_command_timeout: u64,
    pub max_output: usize,
    pub max_read_lines: usize,
    pub max_matches: usize,
    pub auto_compact: bool,
    pub parallel_tools: bool,
    pub shell: Shell,
    pub ignored: Vec<String>,
    pub safe_commands: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub final_newline: FinalNewline,
}

pub const IGNORED: [&str; 13] =
    [".git", "node_modules", "target", "dist", "build", ".venv", "venv", "__pycache__", ".next", ".idea", ".vs", "bin", "obj"];

pub const SAFE_COMMANDS: [&str; 34] = [
    "git status",
    "git diff",
    "git log",
    "git show",
    "git branch",
    "git remote -v",
    "git rev-parse",
    "Get-ChildItem",
    "gci",
    "ls",
    "dir",
    "Get-Content",
    "gc",
    "cat",
    "type",
    "Select-String",
    "sls",
    "Select-Object",
    "Where-Object",
    "Sort-Object",
    "Measure-Object",
    "Format-Table",
    "Format-List",
    "Get-Location",
    "pwd",
    "Get-Item",
    "Test-Path",
    "Resolve-Path",
    "rg",
    "findstr",
    "where",
    "Get-Command",
    "echo",
    "Write-Output",
];

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_steps: 100,
            command_timeout: 120,
            max_command_timeout: 600,
            max_output: 30_000,
            max_read_lines: 1500,
            max_matches: 200,
            auto_compact: true,
            parallel_tools: true,
            shell: Shell::default(),
            ignored: IGNORED.iter().map(|s| s.to_string()).collect(),
            safe_commands: SAFE_COMMANDS.iter().map(|s| s.to_string()).collect(),
            disabled_tools: Vec::new(),
            final_newline: FinalNewline::Keep,
        }
    }
}