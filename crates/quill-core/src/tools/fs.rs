use std::fs;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::{Value, json};

use super::args::{self, CONTENT, GLOB, NEW, OLD, PATH, PATTERN};
use super::paths::{display, inside, normalize, resolve};
use super::{Action, Ctx, Output, Prepared, Spec, Tool, Toolset};
use crate::diff::{FileDiff, diff_text};
use crate::options::Limits;
use crate::policy::Effect;
use crate::protocol::Category;

const MAX_FILE: u64 = 1_000_000;

fn walker(dir: &Path, limits: &Limits, depth: Option<usize>) -> ignore::Walk {
    let ignored = limits.ignored.clone();
    let mut builder = WalkBuilder::new(dir);
    builder
        .hidden(false)
        .require_git(false)
        .max_depth(depth)
        .filter_entry(move |entry| {
            let dir = entry.file_type().is_some_and(|t| t.is_dir());
            !(dir && entry.depth() > 0 && ignored.iter().any(|name| entry.file_name().to_string_lossy().eq_ignore_ascii_case(name)))
        })
        .sort_by_file_path(|a, b| {
            b.is_dir().cmp(&a.is_dir()).then_with(|| a.to_string_lossy().to_lowercase().cmp(&b.to_string_lossy().to_lowercase()))
        });
    builder.build()
}

fn files(dir: &Path, limits: &Limits) -> Vec<PathBuf> {
    walker(dir, limits, None)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .map(|entry| entry.into_path())
        .collect()
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err("That is a binary file, so it cannot be shown as text.".into());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn locate(root: &Path, args: &Value) -> Result<(PathBuf, Effect), String> {
    let path = normalize(root, args::text(args, &PATH).unwrap_or_default())?;
    let effect = if inside(&path, root) { Effect::Read } else { Effect::ReadOutside };
    Ok((path, effect))
}

fn reading(effect: Effect, path: &Path, action: Action) -> Prepared {
    match effect {
        Effect::Read => Prepared::read(action),
        _ => Prepared::new(effect, path.display().to_string(), "Wants to read outside the project", action),
    }
}

fn glob_set(pattern: &str) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for part in pattern.split(',').map(str::trim).filter(|part| !part.is_empty()) {
        let glob = GlobBuilder::new(&part.replace('\\', "/")).case_insensitive(true).build().map_err(|e| e.to_string())?;
        builder.add(glob);
    }
    builder.build().map_err(|e| e.to_string())
}

fn clip_line(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.chars().count() > 300 { format!("{}…", trimmed.chars().take(300).collect::<String>()) } else { trimmed.to_string() }
}

pub struct ListDir;

const LIST: Spec = Spec {
    name: "list_dir",
    title: "List folders",
    summary: "Shows how a folder is laid out, skipping ignored folders.",
    aliases: &["list_files", "list", "ls", "list_directory", "tree"],
    toolset: Toolset::Core,
    category: Category::Explore,
    active: "Looking through",
    done: "Looked through",
    parallel: true,
    description: "List the files and folders in a folder as a tree. Folders such as node_modules and anything in .gitignore are skipped. Use it first to see how a project is laid out.",
    guide: "list_dir shows how a folder is laid out. Start with it in a project you have not seen.",
};

impl Tool for ListDir {
    fn spec(&self) -> &'static Spec {
        &LIST
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Folder relative to the project root. Leave empty for the root." },
                "depth": { "type": "integer", "description": "How many levels to show, from 1 to 5. Default 2." }
            }
        })
    }

    fn target(&self, args: &Value) -> String {
        args::trimmed(args, &PATH).map_or(".".into(), |path| path.replace('\\', "/"))
    }

    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String> {
        let (dir, effect) = locate(ctx.root, args)?;
        let depth = args::number(args, &["depth", "levels", "max_depth"]).unwrap_or(2).clamp(1, 5) as usize;
        let root = ctx.root.to_path_buf();
        let shown = dir.clone();
        Ok(reading(effect, &shown, Box::new(move |ctx: &Ctx<'_>| Output::settle(list(&root, &dir, depth, ctx.limits)))))
    }
}

fn list(root: &Path, dir: &Path, depth: usize, limits: &Limits) -> Result<String, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a folder.", display(root, dir)));
    }
    let mut lines = Vec::new();
    let mut more = 0;
    for entry in walker(dir, limits, Some(depth)).filter_map(Result::ok).filter(|entry| entry.depth() > 0) {
        if lines.len() >= limits.max_list {
            more += 1;
            continue;
        }
        let indent = "  ".repeat(entry.depth() - 1);
        let slash = if entry.file_type().is_some_and(|t| t.is_dir()) { "/" } else { "" };
        lines.push(format!("{indent}{}{slash}", entry.file_name().to_string_lossy()));
    }
    if lines.is_empty() {
        return Ok("The folder is empty.".into());
    }
    if more > 0 {
        lines.push(format!("... and {more} more entries. List a subfolder to see them."));
    }
    Ok(lines.join("\n"))
}

pub struct FindFiles;

const FIND: Spec = Spec {
    name: "find_files",
    title: "Find files",
    summary: "Finds files by name or path pattern.",
    aliases: &["glob", "find", "find_file", "file_search", "find_by_name"],
    toolset: Toolset::Core,
    category: Category::Explore,
    active: "Looking for",
    done: "Looked for",
    parallel: true,
    description: "Find files whose path matches a glob pattern such as **/*.rs or src/**/test_*.py, case-insensitive. Separate several patterns with commas.",
    guide: "find_files finds files by name or path pattern.",
};

impl Tool for FindFiles {
    fn spec(&self) -> &'static Spec {
        &FIND
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern, for example **/*.tsx or *config*." },
                "path": { "type": "string", "description": "Optional folder to search in." }
            },
            "required": ["pattern"]
        })
    }

    fn target(&self, args: &Value) -> String {
        args::trimmed(args, &PATTERN).unwrap_or_default().to_string()
    }

    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String> {
        let pattern = args::trimmed(args, &PATTERN).ok_or("find_files needs a \"pattern\", such as **/*.rs.")?.to_string();
        let set = glob_set(&pattern)?;
        let (dir, effect) = locate(ctx.root, args)?;
        let root = ctx.root.to_path_buf();
        let shown = dir.clone();
        Ok(reading(
            effect,
            &shown,
            Box::new(move |ctx: &Ctx<'_>| {
                let mut found = Vec::new();
                let mut extra = 0;
                for path in files(&dir, ctx.limits) {
                    let rel = display(&dir, &path);
                    if set.is_match(&rel) {
                        if found.len() < ctx.limits.max_matches {
                            found.push(display(&root, &path));
                        } else {
                            extra += 1;
                        }
                    }
                }
                if found.is_empty() {
                    return Output::ok(format!("No files match \"{pattern}\"."));
                }
                if extra > 0 {
                    found.push(format!("... and {extra} more files. Use a narrower pattern."));
                }
                Output::ok(found.join("\n"))
            }),
        ))
    }
}

pub struct Grep;

const GREP: Spec = Spec {
    name: "grep",
    title: "Search files",
    summary: "Searches file contents for text or a pattern.",
    aliases: &["search", "search_files", "find_in_files", "search_code", "rg", "ripgrep", "grep_search"],
    toolset: Toolset::Core,
    category: Category::Explore,
    active: "Searching for",
    done: "Searched for",
    parallel: true,
    description: "Search the contents of every text file for a regular expression, case-insensitive unless case_sensitive is true. Returns path:line: text for each match. Plain text works too.",
    guide: "grep searches file contents with a regular expression. Use it to find where something is defined or used.",
};

impl Tool for Grep {
    fn spec(&self) -> &'static Spec {
        &GREP
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regular expression or plain text to look for." },
                "path": { "type": "string", "description": "Optional folder or file to search in." },
                "glob": { "type": "string", "description": "Optional file filter, for example *.rs." },
                "case_sensitive": { "type": "boolean", "description": "Match letter case exactly. Default false." },
                "context": { "type": "integer", "description": "Lines of context to show around each match, 0 to 5. Default 0." }
            },
            "required": ["pattern"]
        })
    }

    fn target(&self, args: &Value) -> String {
        args::text(args, &PATTERN).unwrap_or_default().to_string()
    }

    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String> {
        let pattern = args::text(args, &PATTERN).filter(|p| !p.trim().is_empty()).ok_or("grep needs a \"pattern\".")?.to_string();
        let case_sensitive = args::flag(args, &["case_sensitive", "match_case"]);
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .or_else(|_| RegexBuilder::new(&regex::escape(&pattern)).case_insensitive(!case_sensitive).build())
            .map_err(|e| e.to_string())?;
        let filter = args::trimmed(args, &GLOB).map(glob_set).transpose()?;
        let context = args::number(args, &["context", "context_lines", "-C"]).unwrap_or(0).min(5) as usize;
        let (base, effect) = locate(ctx.root, args)?;
        let root = ctx.root.to_path_buf();
        let shown = base.clone();
        Ok(reading(
            effect,
            &shown,
            Box::new(move |ctx: &Ctx<'_>| {
                let candidates = if base.is_file() { vec![base.clone()] } else { files(&base, ctx.limits) };
                let mut out = Vec::new();
                let mut matches = 0;
                let mut extra = 0;
                for file in candidates {
                    if filter.as_ref().is_some_and(|set| !set.is_match(display(&base, &file))) {
                        continue;
                    }
                    if fs::metadata(&file).map_or(true, |m| m.len() > MAX_FILE) {
                        continue;
                    }
                    let Ok(text) = read_text(&file) else { continue };
                    let lines: Vec<&str> = text.lines().collect();
                    let rel = display(&root, &file);
                    let mut printed: Option<usize> = None;
                    for (index, line) in lines.iter().enumerate() {
                        if !regex.is_match(line) {
                            continue;
                        }
                        if matches >= ctx.limits.max_matches {
                            extra += 1;
                            continue;
                        }
                        matches += 1;
                        let from = index.saturating_sub(context);
                        let to = (index + context).min(lines.len() - 1);
                        if context > 0 && printed.is_some_and(|last| from > last + 1) {
                            out.push("--".to_string());
                        }
                        for (n, text) in lines.iter().enumerate().take(to + 1).skip(from) {
                            if printed.is_some_and(|last| n <= last) {
                                continue;
                            }
                            let sep = if n == index { ':' } else { '-' };
                            out.push(format!("{rel}{sep}{}{sep} {}", n + 1, clip_line(text)));
                            printed = Some(n);
                        }
                    }
                }
                if out.is_empty() {
                    return Output::ok(format!("No matches for \"{pattern}\"."));
                }
                if extra > 0 {
                    out.push(format!("... and {extra} more matches. Narrow the search with a longer pattern, a path or a glob."));
                }
                Output::ok(out.join("\n"))
            }),
        ))
    }
}

pub struct ReadFile;

const READ: Spec = Spec {
    name: "read_file",
    title: "Read files",
    summary: "Reads a file, or part of one, with line numbers.",
    aliases: &["read", "view", "cat", "open_file", "view_file", "get_file"],
    toolset: Toolset::Core,
    category: Category::Explore,
    active: "Reading",
    done: "Read",
    parallel: true,
    description: "Read a text file with line numbers. Read a file before changing it. In a large file, read only the lines you need with start_line and end_line.",
    guide: "read_file reads a file with line numbers. Read a file before you change it, and in a large file read only the lines you need.",
};

impl Tool for ReadFile {
    fn spec(&self) -> &'static Spec {
        &READ
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path relative to the project root." },
                "start_line": { "type": "integer", "description": "First line to show, starting at 1." },
                "end_line": { "type": "integer", "description": "Last line to show." }
            },
            "required": ["path"]
        })
    }

    fn target(&self, args: &Value) -> String {
        args::text(args, &PATH).unwrap_or_default().trim().replace('\\', "/")
    }

    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String> {
        args::trimmed(args, &PATH).ok_or("read_file needs a \"path\".")?;
        let (path, effect) = locate(ctx.root, args)?;
        let start = args::number(args, &["start_line", "offset", "start", "from", "line"]).unwrap_or(1).max(1) as usize;
        let end = args::number(args, &["end_line", "end", "to"])
            .map(|end| end as usize)
            .or_else(|| args::number(args, &["limit", "lines", "count"]).map(|n| start + n as usize - 1));
        let root = ctx.root.to_path_buf();
        let shown = path.clone();
        Ok(reading(effect, &shown, Box::new(move |ctx: &Ctx<'_>| Output::settle(read(&root, &path, start, end, ctx.limits)))))
    }
}

fn read(root: &Path, path: &Path, start: usize, end: Option<usize>, limits: &Limits) -> Result<String, String> {
    if path.is_dir() {
        return Err(format!("{} is a folder. Use list_dir to see what is in it.", display(root, path)));
    }
    if !path.is_file() {
        return Err(format!("{} does not exist. Use list_dir or find_files to see what is there.", display(root, path)));
    }
    let text = read_text(path)?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok("The file is empty.".into());
    }
    if start > lines.len() {
        return Err(format!("The file only has {} lines.", lines.len()));
    }
    let end = end.unwrap_or(lines.len()).min(lines.len()).min(start + limits.max_read_lines - 1).max(start);
    let mut out: Vec<String> = (start..=end)
        .map(|n| if lines[n - 1].trim().is_empty() { format!("{n:>5}") } else { format!("{n:>5}  {}", lines[n - 1]) })
        .collect();
    if end < lines.len() || start > 1 {
        out.push(format!("(showing lines {start}-{end} of {}; pass start_line to read more)", lines.len()));
    }
    Ok(out.join("\n"))
}

struct Change {
    path: PathBuf,
    after: String,
    diff: FileDiff,
}

fn planned(change: Change, title: &str) -> Prepared {
    let summary = |diff: &FileDiff| {
        let verb = if diff.created { "Created" } else { "Updated" };
        format!("{verb} {} (+{} -{})", diff.path, diff.added, diff.removed)
    };
    let diff = change.diff.clone();
    let mut prepared = Prepared::new(
        Effect::Write,
        diff.path.clone(),
        title,
        Box::new(move |_: &Ctx<'_>| match write(&change.path, &change.after) {
            Ok(()) => Output { diffs: vec![change.diff.clone()], ..Output::ok(summary(&change.diff)) },
            Err(error) => Output::fail(error),
        }),
    );
    prepared.approval.diffs = vec![diff];
    prepared
}

pub fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(path, text).map_err(|e| e.to_string())
}

fn target_file(root: &Path, args: &Value, tool: &str) -> Result<(PathBuf, Option<String>), String> {
    let rel = args::trimmed(args, &PATH).ok_or(format!("{tool} needs a \"path\"."))?;
    let path = resolve(root, rel)?;
    if path.is_dir() {
        return Err(format!("{} is a folder, not a file.", display(root, &path)));
    }
    let before = if path.is_file() { Some(read_text(&path)?) } else { None };
    Ok((path, before))
}

pub struct WriteFile;

const WRITE: Spec = Spec {
    name: "write_file",
    title: "Write files",
    summary: "Creates a file or replaces a whole file.",
    aliases: &["write", "create_file", "save_file", "create"],
    toolset: Toolset::Core,
    category: Category::Edit,
    active: "Writing",
    done: "Wrote",
    parallel: false,
    description: "Create a file or replace its whole content. Use it for new files; prefer edit_file or apply_patch for changes to existing files.",
    guide: "write_file creates a new file or replaces a whole file.",
};

impl Tool for WriteFile {
    fn spec(&self) -> &'static Spec {
        &WRITE
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path relative to the project root." },
                "content": { "type": "string", "description": "The complete content of the file." }
            },
            "required": ["path", "content"]
        })
    }

    fn target(&self, args: &Value) -> String {
        args::text(args, &PATH).unwrap_or_default().trim().replace('\\', "/")
    }

    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String> {
        let (path, before) = target_file(ctx.root, args, "write_file")?;
        let after = args::text(args, &CONTENT).ok_or("write_file needs \"content\": the complete text of the file.")?.to_string();
        let diff = diff_text(&display(ctx.root, &path), before.as_deref(), &after);
        let title = if diff.created { "Wants to create a file" } else { "Wants to rewrite a file" };
        Ok(planned(Change { path, after, diff }, title))
    }
}

pub struct EditFile;

const EDIT: Spec = Spec {
    name: "edit_file",
    title: "Edit files",
    summary: "Replaces exact text in a file, one or several places at once.",
    aliases: &["edit", "str_replace", "replace_in_file", "apply_edit", "multi_edit", "multiedit"],
    toolset: Toolset::Core,
    category: Category::Edit,
    active: "Editing",
    done: "Edited",
    parallel: false,
    description: "Replace exact text in a file. old_text must match the file exactly, including spaces, and appear once unless replace_all is true. To make several changes to one file at once, pass edits: a list of {old_text, new_text, replace_all}. All edits apply or none do.",
    guide: "edit_file replaces exact text in a file. Copy old_text exactly from read_file output without the line numbers, with enough surrounding lines to make it unique. Pass edits to change several places at once.",
};

impl Tool for EditFile {
    fn spec(&self) -> &'static Spec {
        &EDIT
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path relative to the project root." },
                "old_text": { "type": "string", "description": "Exact text to replace, copied from the file." },
                "new_text": { "type": "string", "description": "Replacement text. Use an empty string to delete." },
                "replace_all": { "type": "boolean", "description": "Replace every occurrence." },
                "edits": {
                    "type": "array",
                    "description": "Several replacements in one file, applied in order.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_text": { "type": "string" },
                            "new_text": { "type": "string" },
                            "replace_all": { "type": "boolean" }
                        },
                        "required": ["old_text", "new_text"]
                    }
                }
            },
            "required": ["path"]
        })
    }

    fn target(&self, args: &Value) -> String {
        args::text(args, &PATH).unwrap_or_default().trim().replace('\\', "/")
    }

    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String> {
        let (path, before) = target_file(ctx.root, args, "edit_file")?;
        let current = before.as_deref().ok_or(format!("{} does not exist. Use write_file to create it.", display(ctx.root, &path)))?;
        let edits = edits(args)?;
        let mut after = current.to_string();
        for (index, (old, new, all)) in edits.iter().enumerate() {
            after = replace(&after, old, new, *all)
                .map_err(|error| if edits.len() > 1 { format!("Edit {}: {error}", index + 1) } else { error })?;
        }
        let diff = diff_text(&display(ctx.root, &path), before.as_deref(), &after);
        Ok(planned(Change { path, after, diff }, "Wants to change a file"))
    }
}

fn edits(args: &Value) -> Result<Vec<(String, String, bool)>, String> {
    let one = |value: &Value| -> Result<(String, String, bool), String> {
        let old = args::text(value, &OLD).ok_or("edit_file needs \"old_text\", copied exactly from the file.")?;
        let new = args::text(value, &NEW).ok_or("edit_file needs \"new_text\". Use an empty string to delete.")?;
        Ok((old.to_string(), new.to_string(), args::flag(value, &["replace_all", "all"])))
    };
    match args.get("edits").and_then(Value::as_array) {
        Some(list) if !list.is_empty() => list.iter().map(one).collect(),
        _ => Ok(vec![one(args)?]),
    }
}

pub fn replace(current: &str, old: &str, new: &str, all: bool) -> Result<String, String> {
    if old.is_empty() {
        return Err("old_text is empty. Copy the exact text you want to replace from the file.".into());
    }
    let crlf = current.contains("\r\n") && !old.contains("\r\n");
    let (old, new) = if crlf { (old.replace('\n', "\r\n"), new.replace('\n', "\r\n")) } else { (old.to_string(), new.to_string()) };
    match current.matches(old.as_str()).count() {
        0 => loose_replace(current, &old, &new).ok_or_else(|| {
            "old_text was not found in the file. Read the file again and copy the text exactly, including indentation.".into()
        }),
        1 => Ok(current.replacen(old.as_str(), &new, 1)),
        _ if all => Ok(current.replace(old.as_str(), &new)),
        n => Err(format!("old_text appears {n} times. Include more surrounding lines so it is unique, or set replace_all.")),
    }
}

fn without_numbers(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let numbered: Vec<Option<(usize, &str)>> = lines
        .iter()
        .map(|line| {
            let trimmed = line.trim_start();
            let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
            let number = trimmed[..digits].parse().ok()?;
            let rest = &trimmed[digits..];
            let body = rest.strip_prefix("  ").or_else(|| rest.strip_prefix('\t')).or_else(|| rest.strip_prefix('→'));
            match body {
                Some(body) => Some((number, body)),
                None if rest.trim().is_empty() => Some((number, "")),
                None => None,
            }
        })
        .collect();
    let consecutive = numbered.windows(2).all(|pair| matches!(pair, [Some((a, _)), Some((b, _))] if b == &(a + 1)));
    if lines.is_empty() || numbered.iter().any(Option::is_none) || !consecutive {
        return text.to_string();
    }
    numbered.into_iter().flatten().map(|(_, body)| body).collect::<Vec<_>>().join("\n")
}

fn loose_replace(current: &str, old: &str, new: &str) -> Option<String> {
    let old = without_numbers(old.trim_matches(['\r', '\n']));
    let new = without_numbers(new.trim_matches(['\r', '\n']));
    let wanted: Vec<&str> = old.lines().collect();
    let lines: Vec<&str> = current.lines().collect();
    if wanted.is_empty() || wanted.len() > lines.len() {
        return None;
    }
    for pass in 0..2 {
        let same = |a: &str, b: &str| if pass == 0 { a.trim_end() == b.trim_end() } else { a.trim() == b.trim() };
        let hits: Vec<usize> =
            (0..=lines.len() - wanted.len()).filter(|at| wanted.iter().enumerate().all(|(k, want)| same(lines[at + k], want))).collect();
        if let [at] = hits.as_slice() {
            let sep = if current.contains("\r\n") { "\r\n" } else { "\n" };
            let mut out: Vec<&str> = lines[..*at].to_vec();
            out.extend(new.lines());
            out.extend(&lines[at + wanted.len()..]);
            let mut text = out.join(sep);
            if current.ends_with('\n') && !text.is_empty() {
                text.push_str(sep);
            }
            return Some(text);
        }
        if hits.len() > 1 {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PlanStep;
    use crate::tools::Host;
    use std::sync::atomic::AtomicBool;

    struct Quiet;

    impl Host for Quiet {
        fn output(&self, _: &str) {}
        fn plan(&self, _: Vec<PlanStep>, _: Option<String>) {}
        fn ask(&self, _: &str, _: &[String]) -> Option<String> {
            None
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-tools-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run(tool: &dyn Tool, root: &Path, args: Value) -> Result<Output, String> {
        let limits = Limits::default();
        let cancel = AtomicBool::new(false);
        let ctx = Ctx { root, limits: &limits, cancel: &cancel, host: &Quiet };
        let prepared = tool.prepare(&args, &ctx)?;
        Ok((prepared.action)(&ctx))
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
    fn small_model_mistakes_still_edit() {
        let file = "def greet(name):\n    return \"Hello \" + name\n\nprint(greet(\"world\"))\n";
        let spaces = "def greet(name):\n    return \"Hello \" + name\n  \nprint(greet(\"world\"))";
        let fixed = replace(file, spaces, "def greet(name):\n    return \"Hi \" + name\n\nprint(greet(\"world\"))", false).unwrap();
        assert_eq!(fixed, "def greet(name):\n    return \"Hi \" + name\n\nprint(greet(\"world\"))\n");
        let numbered = "    1  def greet(name):\n    2      return \"Hello \" + name";
        let fixed = replace(file, numbered, "def greet(person):\n    return \"Hello \" + person", false).unwrap();
        assert!(fixed.starts_with("def greet(person):\n    return \"Hello \" + person\n\n"));
        let indented = "  def greet(name):\n      return \"Hello \" + name";
        assert!(replace(file, indented, "x", false).unwrap().starts_with("x\n"));
        assert!(replace("a\nb\na\nb\n", "a \nb", "c", false).is_err());
    }

    #[test]
    fn empty_lines_show_no_trailing_spaces() {
        let root = temp_root("blank");
        fs::write(root.join("a.txt"), "one\n\ntwo\n").unwrap();
        let text = run(&ReadFile, &root, json!({ "path": "a.txt" })).unwrap().text;
        assert!(text.lines().nth(1) == Some("    2"), "{text:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn files_are_written_edited_listed_and_searched() {
        let root = temp_root("write");
        let created = run(&WriteFile, &root, json!({ "path": "src/a.txt", "content": "hi\n" })).unwrap();
        assert!(created.ok && created.diffs[0].created);
        let edited = run(&EditFile, &root, json!({ "file_path": "src/a.txt", "old_str": "hi", "new_str": "hello" })).unwrap();
        assert_eq!((edited.diffs[0].added, edited.diffs[0].removed), (1, 1));
        assert_eq!(fs::read_to_string(root.join("src/a.txt")).unwrap(), "hello\n");
        let listing = run(&ListDir, &root, json!({})).unwrap().text;
        assert!(listing.contains("src/") && listing.contains("  a.txt"), "{listing}");
        assert!(run(&ReadFile, &root, json!({ "path": "src/a.txt" })).unwrap().text.contains("    1  hello"));
        assert!(run(&Grep, &root, json!({ "pattern": "HELLO" })).unwrap().text.contains("src/a.txt:1: hello"));
        assert!(run(&FindFiles, &root, json!({ "pattern": "**/*.txt" })).unwrap().text.contains("src/a.txt"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn several_edits_apply_together_or_not_at_all() {
        let root = temp_root("multi");
        fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let edits = json!({ "path": "a.txt", "edits": [
            { "old_text": "one", "new_text": "1" },
            { "old_text": "three", "new_text": "3" }
        ]});
        assert!(run(&EditFile, &root, edits).unwrap().ok);
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "1\ntwo\n3\n");
        let broken = json!({ "path": "a.txt", "edits": [
            { "old_text": "two", "new_text": "2" },
            { "old_text": "missing", "new_text": "x" }
        ]});
        assert!(run(&EditFile, &root, broken).unwrap_err().starts_with("Edit 2:"));
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "1\ntwo\n3\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn ignored_folders_and_gitignore_are_skipped() {
        let root = temp_root("ignore");
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "needle").unwrap();
        fs::write(root.join(".gitignore"), "secret.txt\n").unwrap();
        fs::write(root.join("secret.txt"), "needle").unwrap();
        fs::write(root.join("kept.txt"), "needle").unwrap();
        let found = run(&Grep, &root, json!({ "pattern": "needle" })).unwrap().text;
        assert!(found.contains("kept.txt") && !found.contains("node_modules") && !found.contains("secret"), "{found}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reading_outside_the_project_needs_a_check() {
        let root = temp_root("outside");
        let limits = Limits::default();
        let cancel = AtomicBool::new(false);
        let ctx = Ctx { root: &root, limits: &limits, cancel: &cancel, host: &Quiet };
        let outside = std::env::temp_dir().join("elsewhere.txt");
        let prepared = ReadFile.prepare(&json!({ "path": outside.to_string_lossy() }), &ctx).unwrap();
        assert_eq!(prepared.effect, Effect::ReadOutside);
        assert!(WriteFile.prepare(&json!({ "path": outside.to_string_lossy(), "content": "x" }), &ctx).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}