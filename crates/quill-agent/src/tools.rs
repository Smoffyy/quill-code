use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::diff::{FileDiff, diff_text};

const IGNORED: [&str; 12] =
    [".git", "node_modules", "target", "dist", "build", ".venv", "venv", "__pycache__", ".next", ".idea", ".vs", "bin"];
const MAX_LIST: usize = 400;
const MAX_READ_LINES: usize = 1500;
const MAX_MATCHES: usize = 100;
const MAX_OUTPUT: usize = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    List,
    Read,
    Search,
    Write,
    Edit,
    Run,
    Commit,
    PullRequest,
}

impl ToolKind {
    pub fn resolve(name: &str) -> Option<ToolKind> {
        let name = name.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        match name.as_str() {
            "list_files" | "list" | "ls" | "list_dir" | "list_directory" | "tree" => Some(ToolKind::List),
            "read_file" | "read" | "view" | "cat" | "open_file" | "view_file" => Some(ToolKind::Read),
            "search" | "search_files" | "grep" | "find_in_files" | "search_code" => Some(ToolKind::Search),
            "write_file" | "write" | "create_file" | "save_file" => Some(ToolKind::Write),
            "edit_file" | "edit" | "str_replace" | "replace_in_file" | "apply_edit" => Some(ToolKind::Edit),
            "run_command" | "run" | "bash" | "shell" | "powershell" | "exec" | "execute_command" | "terminal" => Some(ToolKind::Run),
            "commit_changes" | "commit" | "git_commit" | "create_commit" => Some(ToolKind::Commit),
            "open_pull_request" | "pull_request" | "create_pull_request" | "open_pr" | "create_pr" => Some(ToolKind::PullRequest),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ToolKind::List => "list_files",
            ToolKind::Read => "read_file",
            ToolKind::Search => "search",
            ToolKind::Write => "write_file",
            ToolKind::Edit => "edit_file",
            ToolKind::Run => "run_command",
            ToolKind::Commit => "commit_changes",
            ToolKind::PullRequest => "open_pull_request",
        }
    }
}

pub fn schemas(git: bool) -> Vec<Value> {
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": { "type": "object", "properties": properties, "required": required }
            }
        })
    };
    let mut tools = vec![
        tool(
            "list_files",
            "List files and folders in the project, a few levels deep. Use it first to see how the project is laid out.",
            json!({ "path": { "type": "string", "description": "Folder relative to the project root. Leave empty for the root." } }),
            &[],
        ),
        tool(
            "read_file",
            "Read a text file with line numbers. Always read a file before editing it.",
            json!({
                "path": { "type": "string", "description": "File path relative to the project root." },
                "start_line": { "type": "integer", "description": "First line to show, starting at 1." },
                "end_line": { "type": "integer", "description": "Last line to show." }
            }),
            &["path"],
        ),
        tool(
            "search",
            "Search every text file in the project for a piece of text, case-insensitive. Returns matching lines with their paths and line numbers.",
            json!({
                "query": { "type": "string", "description": "Text to look for." },
                "path": { "type": "string", "description": "Optional folder to limit the search to." }
            }),
            &["query"],
        ),
        tool(
            "write_file",
            "Create a file or replace its whole content. Use it for new files; prefer edit_file for changes to existing files.",
            json!({
                "path": { "type": "string", "description": "File path relative to the project root." },
                "content": { "type": "string", "description": "The complete content of the file." }
            }),
            &["path", "content"],
        ),
        tool(
            "edit_file",
            "Replace an exact piece of text in a file. old_text must match the file exactly, including spaces, and appear once unless replace_all is true.",
            json!({
                "path": { "type": "string", "description": "File path relative to the project root." },
                "old_text": { "type": "string", "description": "Exact text to replace, copied from the file." },
                "new_text": { "type": "string", "description": "Replacement text. Use an empty string to delete." },
                "replace_all": { "type": "boolean", "description": "Replace every occurrence." }
            }),
            &["path", "old_text", "new_text"],
        ),
        tool(
            "run_command",
            &format!(
                "Run a {} command in the project folder and return its output and exit code. Use it to build, test and run code.",
                shell_name()
            ),
            json!({
                "command": { "type": "string", "description": format!("The {} command to run.", shell_name()) },
                "timeout_seconds": { "type": "integer", "description": "How long to wait before stopping it. Default 120, maximum 600." }
            }),
            &["command"],
        ),
    ];
    if git {
        tools.push(tool(
            "commit_changes",
            "Stage every change in the project and commit it. Give a branch to commit on a new branch, which a pull request needs when you are on the main branch or not on a branch.",
            json!({
                "message": { "type": "string", "description": "Commit message: a short summary line, optionally followed by a blank line and details." },
                "branch": { "type": "string", "description": "Optional branch to create or switch to before committing, for example fix-login." }
            }),
            &["message"],
        ));
        tools.push(tool(
            "open_pull_request",
            "Push the current branch to GitHub and open a pull request into the main branch. Commit your work with commit_changes first.",
            json!({
                "title": { "type": "string", "description": "Pull request title." },
                "body": { "type": "string", "description": "What changed and why, in Markdown." }
            }),
            &["title"],
        ));
    }
    tools
}

pub fn parse_args(arguments: &str) -> Value {
    let text = arguments.trim();
    if text.is_empty() {
        return json!({});
    }
    serde_json::from_str(text).unwrap_or_else(|_| json!({ "__invalid": text }))
}

pub fn arg<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| args.get(*key).and_then(Value::as_str))
}

fn arg_number(args: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        let value = args.get(*key)?;
        value.as_u64().or_else(|| value.as_str()?.trim().parse().ok())
    })
}

fn arg_bool(args: &Value, key: &str) -> bool {
    args.get(key).is_some_and(|v| v.as_bool().unwrap_or_else(|| v.as_str() == Some("true")))
}

pub const PATH_KEYS: [&str; 5] = ["path", "file_path", "file", "filename", "filepath"];
const CONTENT_KEYS: [&str; 6] = ["content", "contents", "text", "file_text", "code", "body"];
const OLD_KEYS: [&str; 6] = ["old_text", "old_str", "old_string", "old", "search", "find"];
const NEW_KEYS: [&str; 6] = ["new_text", "new_str", "new_string", "new", "replace", "replacement"];
pub const COMMAND_KEYS: [&str; 3] = ["command", "cmd", "script"];
pub const MESSAGE_KEYS: [&str; 3] = ["message", "commit_message", "msg"];
pub const BRANCH_KEYS: [&str; 3] = ["branch", "branch_name", "new_branch"];
pub const TITLE_KEYS: [&str; 2] = ["title", "pr_title"];
pub const BODY_KEYS: [&str; 3] = ["body", "description", "pr_body"];
const QUERY_KEYS: [&str; 4] = ["query", "pattern", "text", "q"];

pub fn describe(kind: ToolKind, args: &Value) -> String {
    match kind {
        ToolKind::Run => arg(args, &COMMAND_KEYS).unwrap_or_default().trim().to_string(),
        ToolKind::Search => arg(args, &QUERY_KEYS).unwrap_or_default().to_string(),
        ToolKind::Commit => arg(args, &MESSAGE_KEYS).unwrap_or_default().lines().next().unwrap_or_default().trim().to_string(),
        ToolKind::PullRequest => arg(args, &TITLE_KEYS).unwrap_or_default().trim().to_string(),
        _ => {
            let path = arg(args, &PATH_KEYS).unwrap_or_default().trim().replace('\\', "/");
            if path.is_empty() && kind == ToolKind::List { ".".into() } else { path }
        }
    }
}

fn same_or_inside(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().to_lowercase().replace('/', "\\");
    let root = root.to_string_lossy().to_lowercase().replace('/', "\\");
    let root = root.trim_end_matches('\\');
    path == root || path.starts_with(&format!("{root}\\"))
}

pub fn resolve_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim().trim_matches('"');
    let candidate = Path::new(rel);
    let joined = if candidate.is_absolute() { candidate.to_path_buf() } else { root.join(candidate) };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                if !out.pop() {
                    return Err(outside());
                }
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    if same_or_inside(&out, root) { Ok(out) } else { Err(outside()) }
}

fn outside() -> String {
    "That path is outside the project. Use a path relative to the project root, such as \"src/main.rs\".".into()
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).map(|rel| rel.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

fn ignored(name: &str) -> bool {
    IGNORED.contains(&name)
}

fn sorted_entries(dir: &Path) -> Vec<(String, PathBuf, bool)> {
    let Ok(read) = fs::read_dir(dir) else { return Vec::new() };
    let mut entries: Vec<_> = read
        .filter_map(Result::ok)
        .map(|e| {
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            (e.file_name().to_string_lossy().into_owned(), e.path(), is_dir)
        })
        .collect();
    entries.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    entries
}

pub fn list(root: &Path, args: &Value) -> Result<String, String> {
    let dir = resolve_path(root, arg(args, &PATH_KEYS).unwrap_or_default())?;
    if !dir.is_dir() {
        return Err(format!("{} is not a folder.", display(root, &dir)));
    }
    let mut lines = Vec::new();
    let mut more = 0;
    walk_tree(&dir, 0, 3, &mut lines, &mut more);
    if lines.is_empty() {
        return Ok("The folder is empty.".into());
    }
    if more > 0 {
        lines.push(format!("... and {more} more entries. List a subfolder to see them."));
    }
    Ok(lines.join("\n"))
}

fn walk_tree(dir: &Path, depth: usize, max_depth: usize, lines: &mut Vec<String>, more: &mut usize) {
    for (name, path, is_dir) in sorted_entries(dir) {
        if lines.len() >= MAX_LIST {
            *more += 1;
            continue;
        }
        let indent = "  ".repeat(depth);
        if is_dir {
            if ignored(&name) {
                lines.push(format!("{indent}{name}/ (skipped)"));
                continue;
            }
            lines.push(format!("{indent}{name}/"));
            if depth + 1 < max_depth {
                walk_tree(&path, depth + 1, max_depth, lines, more);
            }
        } else {
            lines.push(format!("{indent}{name}"));
        }
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err("That is a binary file, so it cannot be shown as text.".into());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn read(root: &Path, args: &Value) -> Result<String, String> {
    let rel = arg(args, &PATH_KEYS).ok_or("read_file needs a \"path\".")?;
    let path = resolve_path(root, rel)?;
    if !path.is_file() {
        return Err(format!("{} does not exist. Use list_files to see what is there.", display(root, &path)));
    }
    let text = read_text(&path)?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok("The file is empty.".into());
    }
    let start = arg_number(args, &["start_line", "start", "from"]).unwrap_or(1).max(1) as usize;
    let end =
        arg_number(args, &["end_line", "end", "to"]).map_or(lines.len(), |e| e as usize).min(lines.len()).min(start + MAX_READ_LINES - 1);
    if start > lines.len() {
        return Err(format!("The file only has {} lines.", lines.len()));
    }
    let mut out: Vec<String> = (start..=end).map(|n| format!("{n:>5}  {}", lines[n - 1])).collect();
    if end < lines.len() || start > 1 {
        out.push(format!("(showing lines {start}-{end} of {}; pass start_line to read more)", lines.len()));
    }
    Ok(out.join("\n"))
}

fn walk_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for (name, path, is_dir) in sorted_entries(dir) {
        if files.len() >= 5000 {
            return;
        }
        if is_dir {
            if !ignored(&name) {
                walk_files(&path, files);
            }
        } else {
            files.push(path);
        }
    }
}

pub fn search(root: &Path, args: &Value) -> Result<String, String> {
    let query = arg(args, &QUERY_KEYS).filter(|q| !q.trim().is_empty()).ok_or("search needs a \"query\".")?;
    let dir = resolve_path(root, arg(args, &PATH_KEYS).unwrap_or_default())?;
    let needle = query.to_lowercase();
    let mut files = Vec::new();
    walk_files(&dir, &mut files);
    let mut matches = Vec::new();
    let mut extra = 0;
    for file in files {
        if fs::metadata(&file).map_or(true, |m| m.len() > 1_000_000) {
            continue;
        }
        let Ok(text) = read_text(&file) else { continue };
        for (index, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(&needle) {
                if matches.len() < MAX_MATCHES {
                    let shown: String = line.trim().chars().take(200).collect();
                    matches.push(format!("{}:{}: {shown}", display(root, &file), index + 1));
                } else {
                    extra += 1;
                }
            }
        }
    }
    if matches.is_empty() {
        return Ok(format!("No matches for \"{query}\"."));
    }
    if extra > 0 {
        matches.push(format!("... and {extra} more matches. Narrow the search with a longer query or a path."));
    }
    Ok(matches.join("\n"))
}

pub struct Change {
    pub path: PathBuf,
    pub after: String,
    pub diff: FileDiff,
}

pub fn plan_change(root: &Path, kind: ToolKind, args: &Value) -> Result<Change, String> {
    let rel = arg(args, &PATH_KEYS).ok_or(format!("{} needs a \"path\".", kind.name()))?;
    let path = resolve_path(root, rel)?;
    if path.is_dir() {
        return Err(format!("{} is a folder, not a file.", display(root, &path)));
    }
    let before = if path.is_file() { Some(read_text(&path)?) } else { None };
    let after = match kind {
        ToolKind::Write => arg(args, &CONTENT_KEYS).ok_or("write_file needs \"content\": the complete text of the file.")?.to_string(),
        _ => {
            let current = before.as_deref().ok_or(format!("{} does not exist. Use write_file to create it.", display(root, &path)))?;
            let old = arg(args, &OLD_KEYS).ok_or("edit_file needs \"old_text\", copied exactly from the file.")?;
            let new = arg(args, &NEW_KEYS).ok_or("edit_file needs \"new_text\". Use an empty string to delete.")?;
            replace(current, old, new, arg_bool(args, "replace_all"))?
        }
    };
    let diff = diff_text(&display(root, &path), before.as_deref(), &after);
    Ok(Change { path, after, diff })
}

fn replace(current: &str, old: &str, new: &str, all: bool) -> Result<String, String> {
    if old.is_empty() {
        return Err("old_text is empty. Copy the exact text you want to replace from the file.".into());
    }
    let crlf = current.contains("\r\n") && !old.contains("\r\n");
    let (old, new) = if crlf { (old.replace('\n', "\r\n"), new.replace('\n', "\r\n")) } else { (old.to_string(), new.to_string()) };
    match current.matches(old.as_str()).count() {
        0 => Err("old_text was not found in the file. Read the file again and copy the text exactly, including indentation.".into()),
        1 => Ok(current.replacen(old.as_str(), &new, 1)),
        _ if all => Ok(current.replace(old.as_str(), &new)),
        n => Err(format!("old_text appears {n} times. Include more surrounding lines so it is unique, or set replace_all.")),
    }
}

pub fn apply(change: &Change) -> Result<(), String> {
    if let Some(dir) = change.path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&change.path, &change.after).map_err(|e| e.to_string())
}

pub fn command_timeout(args: &Value) -> Duration {
    Duration::from_secs(arg_number(args, &["timeout_seconds", "timeout"]).unwrap_or(120).clamp(1, 600))
}

pub fn shell_name() -> &'static str {
    if cfg!(windows) { "PowerShell" } else { "sh" }
}

pub fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

fn shell(command: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; $ProgressPreference='SilentlyContinue'; {command}"),
        ]);
        hide_window(&mut cmd);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    }
}

#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_window(_: &mut Command) {}

fn cap(text: String) -> String {
    if text.len() <= MAX_OUTPUT {
        return text;
    }
    let mut head = MAX_OUTPUT * 2 / 3;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - MAX_OUTPUT / 3;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}\n... [output cut] ...\n{}", &text[..head], &text[tail..])
}

pub fn run(root: &Path, command: &str, timeout: Duration, cancel: &AtomicBool) -> (bool, String) {
    let child = shell(command).current_dir(root).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => return (false, format!("The command could not start: {e}")),
    };
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = stdout.as_mut() {
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });
    let err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });
    let started = Instant::now();
    let (status, note) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), None),
            Ok(None) if cancel.load(Ordering::Relaxed) => {
                let _ = child.kill();
                break (None, Some("Stopped by the user.".to_string()));
            }
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                break (None, Some(format!("Stopped after {} seconds without finishing.", timeout.as_secs())));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => break (None, Some(e.to_string())),
        }
    };
    let _ = child.wait();
    let mut text = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let err_text = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    if !err_text.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&err_text);
    }
    let text = text.trim_end().to_string();
    let ok = status.is_some_and(|s| s.success());
    let mut report = match (status.and_then(|s| s.code()), note) {
        (_, Some(note)) => note,
        (Some(code), None) => format!("Exit code {code}"),
        (None, None) => "Exited".into(),
    };
    if !text.is_empty() {
        report.push('\n');
        report.push_str(&text);
    }
    (ok, cap(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-tools-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn aliases_resolve_to_tools() {
        assert_eq!(ToolKind::resolve("bash"), Some(ToolKind::Run));
        assert_eq!(ToolKind::resolve("str_replace"), Some(ToolKind::Edit));
        assert_eq!(ToolKind::resolve("Read-File"), Some(ToolKind::Read));
        assert_eq!(ToolKind::resolve("teleport"), None);
    }

    #[test]
    fn paths_cannot_leave_the_project() {
        let root = PathBuf::from(r"C:\work\proj");
        assert!(resolve_path(&root, "src/main.rs").is_ok());
        assert!(resolve_path(&root, "../other").is_err());
        assert!(resolve_path(&root, r"C:\Windows\System32").is_err());
        assert!(resolve_path(&root, r"c:\WORK\proj\a.txt").is_ok());
        assert!(resolve_path(&root, r"C:\work\project2\a.txt").is_err());
    }

    #[test]
    fn edits_need_a_unique_match() {
        assert_eq!(replace("a b a", "b", "c", false).unwrap(), "a c a");
        assert!(replace("a b a", "a", "c", false).unwrap_err().contains("2 times"));
        assert_eq!(replace("a b a", "a", "c", true).unwrap(), "c b c");
        assert!(replace("x", "y", "z", false).is_err());
        assert_eq!(replace("one\r\ntwo\r\n", "one\ntwo", "1\n2", false).unwrap(), "1\r\n2\r\n");
    }

    #[test]
    fn files_are_planned_then_written() {
        let root = temp_root("write");
        let change = plan_change(&root, ToolKind::Write, &json!({ "path": "src/a.txt", "content": "hi\n" })).unwrap();
        assert!(change.diff.created);
        apply(&change).unwrap();
        let edit = plan_change(&root, ToolKind::Edit, &json!({ "file_path": "src/a.txt", "old_str": "hi", "new_str": "hello" })).unwrap();
        assert_eq!((edit.diff.added, edit.diff.removed), (1, 1));
        apply(&edit).unwrap();
        assert_eq!(fs::read_to_string(root.join("src/a.txt")).unwrap(), "hello\n");
        let listing = list(&root, &json!({})).unwrap();
        assert!(listing.contains("src/") && listing.contains("  a.txt"));
        assert!(read(&root, &json!({ "path": "src/a.txt" })).unwrap().contains("    1  hello"));
        assert!(search(&root, &json!({ "query": "HELLO" })).unwrap().contains("src/a.txt:1: hello"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn commands_report_exit_codes() {
        let root = temp_root("run");
        let cancel = AtomicBool::new(false);
        let (ok, output) = run(&root, "echo quill", Duration::from_secs(60), &cancel);
        assert!(ok, "{output}");
        assert!(output.starts_with("Exit code 0") && output.contains("quill"));
        let (ok, output) = run(&root, "exit 3", Duration::from_secs(60), &cancel);
        assert!(!ok);
        assert!(output.starts_with("Exit code 3"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn broken_arguments_are_kept_for_the_error() {
        assert_eq!(parse_args("{\"path\": \"a\"}")["path"], "a");
        assert!(parse_args("{oops").get("__invalid").is_some());
        assert_eq!(parse_args(""), json!({}));
    }
}