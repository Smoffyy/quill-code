use std::fs;
use std::path::Path;

const SKIP: [&str; 9] = [".git", "node_modules", "target", "dist", "build", ".venv", "venv", "__pycache__", ".next"];
const MAX_FILES: usize = 8000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    Slash(String),
    Mention { start: usize, query: String },
}

pub fn trigger(text: &str) -> Option<Trigger> {
    if let Some(rest) = text.strip_prefix('/')
        && !rest.contains(char::is_whitespace)
    {
        return Some(Trigger::Slash(rest.to_lowercase()));
    }
    let start = text.rfind(char::is_whitespace).map_or(0, |i| i + 1);
    let token = &text[start..];
    token.strip_prefix('@').map(|query| Trigger::Mention { start, query: query.to_lowercase() })
}

pub fn file_index(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    files
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        if files.len() >= MAX_FILES {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if !SKIP.contains(&name.as_str()) {
                walk(root, &path, files);
            }
        } else if let Ok(rel) = path.strip_prefix(root) {
            files.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn subsequence(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars();
    needle.chars().all(|n| chars.any(|h| h == n))
}

pub fn rank(files: &[String], query: &str, limit: usize) -> Vec<String> {
    let query = query.to_lowercase();
    let mut scored: Vec<(u8, usize, &String)> = files
        .iter()
        .filter_map(|path| {
            let lower = path.to_lowercase();
            let name = lower.rsplit('/').next().unwrap_or(&lower);
            let score = if query.is_empty() {
                3
            } else if name.starts_with(&query) {
                0
            } else if name.contains(&query) {
                1
            } else if lower.contains(&query) {
                2
            } else if subsequence(&lower, &query) {
                4
            } else {
                return None;
            };
            Some((score, path.len(), path))
        })
        .collect();
    scored.sort();
    scored.into_iter().take(limit).map(|(_, _, path)| path.clone()).collect()
}

pub fn mentioned(text: &str) -> Vec<String> {
    let mut paths: Vec<String> = text
        .split_whitespace()
        .filter_map(|word| word.strip_prefix('@'))
        .map(|path| path.trim_end_matches([',', '.', ';', ':', ')', '!', '?']).to_string())
        .filter(|path| !path.is_empty())
        .collect();
    paths.dedup();
    paths
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    New,
    Model,
    Reasoning,
    Mode,
    Review,
    Files,
    Terminal,
    Commit,
    Init,
    Compact,
}

impl Command {
    pub const ALL: [Command; 10] = [
        Command::New,
        Command::Init,
        Command::Model,
        Command::Reasoning,
        Command::Mode,
        Command::Review,
        Command::Files,
        Command::Terminal,
        Command::Commit,
        Command::Compact,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Command::New => "new",
            Command::Model => "model",
            Command::Reasoning => "think",
            Command::Mode => "mode",
            Command::Review => "review",
            Command::Files => "files",
            Command::Terminal => "terminal",
            Command::Commit => "commit",
            Command::Init => "init",
            Command::Compact => "compact",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Command::New => "Start a fresh thread in this project",
            Command::Model => "Pick a different model for the next reply",
            Command::Reasoning => "Turn reasoning on or off",
            Command::Mode => "Choose how much the agent may do without asking",
            Command::Review => "Look over what changed in the last reply",
            Command::Files => "Browse the project's files",
            Command::Terminal => "Open a PowerShell terminal in the project",
            Command::Commit => "Commit your work, then push or open a pull request",
            Command::Init => "Have the agent write an AGENTS.md that explains this project",
            Command::Compact => "Summarise earlier turns to free up the model's context",
        }
    }

    pub fn matching(query: &str) -> Vec<Command> {
        Command::ALL.into_iter().filter(|c| c.name().starts_with(query)).collect()
    }
}

pub const INIT_PROMPT: &str = "Look through this project and write an AGENTS.md at its root for future coding agents. Cover what the project is, how it is laid out, how to build, run and test it, and the conventions the code follows. Read the important files before writing, keep it concise, and if an AGENTS.md already exists, improve it instead of starting over.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_come_from_the_text_being_typed() {
        assert_eq!(trigger("/mo"), Some(Trigger::Slash("mo".into())));
        assert_eq!(trigger("/model x"), None);
        assert_eq!(trigger("fix @src/ma"), Some(Trigger::Mention { start: 4, query: "src/ma".into() }));
        assert_eq!(trigger("@"), Some(Trigger::Mention { start: 0, query: String::new() }));
        assert_eq!(trigger("email me@x"), None);
        assert_eq!(trigger("plain text"), None);
    }

    #[test]
    fn file_names_rank_before_paths() {
        let files: Vec<String> =
            ["docs/main-notes.md", "src/main.rs", "src/domain.rs", "README.md"].iter().map(|s| s.to_string()).collect();
        assert_eq!(rank(&files, "main", 2), vec!["src/main.rs", "docs/main-notes.md"]);
        assert_eq!(rank(&files, "srdom", 5), vec!["src/domain.rs"]);
        assert_eq!(rank(&files, "", 2).len(), 2);
    }

    #[test]
    fn mentions_are_collected_from_a_prompt() {
        assert_eq!(mentioned("compare @a.rs and @src/b.rs, please"), vec!["a.rs", "src/b.rs"]);
        assert!(mentioned("no mentions here").is_empty());
    }

    #[test]
    fn commands_filter_by_prefix() {
        assert_eq!(Command::matching("te"), vec![Command::Terminal]);
        assert_eq!(Command::matching("").len(), Command::ALL.len());
    }
}