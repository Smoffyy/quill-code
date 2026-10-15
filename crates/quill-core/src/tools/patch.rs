use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use super::args::{self, PATCH};
use super::fs::write;
use super::paths::{display, resolve};
use super::{Ctx, Output, Prepared, Spec, Tool, Toolset};
use crate::diff::{FileDiff, diff_text};
use crate::policy::Effect;
use crate::protocol::Category;

#[derive(Debug, PartialEq)]
enum FileOp {
    Add { path: String, content: String },
    Delete { path: String },
    Update { path: String, to: Option<String>, chunks: Vec<Chunk> },
}

#[derive(Debug, Default, PartialEq)]
struct Chunk {
    anchor: Option<String>,
    old: Vec<String>,
    new: Vec<String>,
    eof: bool,
}

fn header<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix).map(str::trim).filter(|path| !path.is_empty())
}

fn is_file_header(line: &str) -> bool {
    ["*** Add File:", "*** Delete File:", "*** Update File:", "*** End Patch"].iter().any(|h| line.starts_with(h))
}

fn parse(text: &str) -> Result<Vec<FileOp>, String> {
    let lines: Vec<&str> = text.lines().map(|line| line.trim_end_matches('\r')).collect();
    let mut ops = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if let Some(path) = header(line, "*** Add File:") {
            let mut content = Vec::new();
            while i < lines.len() && !lines[i].starts_with("***") {
                content.push(lines[i].strip_prefix('+').unwrap_or(lines[i]));
                i += 1;
            }
            let content = if content.is_empty() { String::new() } else { format!("{}\n", content.join("\n")) };
            ops.push(FileOp::Add { path: path.to_string(), content });
        } else if let Some(path) = header(line, "*** Delete File:") {
            ops.push(FileOp::Delete { path: path.to_string() });
        } else if let Some(path) = header(line, "*** Update File:") {
            let to = lines.get(i).and_then(|next| header(next, "*** Move to:")).map(str::to_string);
            if to.is_some() {
                i += 1;
            }
            let mut chunks: Vec<Chunk> = Vec::new();
            while i < lines.len() && !is_file_header(lines[i]) {
                let line = lines[i];
                i += 1;
                if let Some(anchor) = line.strip_prefix("@@") {
                    let anchor = anchor.trim().trim_end_matches("@@").trim();
                    chunks.push(Chunk { anchor: (!anchor.is_empty()).then(|| anchor.to_string()), ..Chunk::default() });
                    continue;
                }
                if chunks.is_empty() {
                    chunks.push(Chunk::default());
                }
                let chunk = chunks.last_mut().expect("a chunk was just added");
                if line == "*** End of File" {
                    chunk.eof = true;
                } else if let Some(removed) = line.strip_prefix('-') {
                    chunk.old.push(removed.to_string());
                } else if let Some(added) = line.strip_prefix('+') {
                    chunk.new.push(added.to_string());
                } else {
                    let context = line.strip_prefix(' ').unwrap_or(line).to_string();
                    chunk.old.push(context.clone());
                    chunk.new.push(context);
                }
            }
            chunks.retain(|chunk| !chunk.old.is_empty() || !chunk.new.is_empty());
            ops.push(FileOp::Update { path: path.to_string(), to, chunks });
        }
    }
    if ops.is_empty() {
        return Err("The patch has no file changes. Wrap it in *** Begin Patch and *** End Patch, with *** Add File:, *** Update File: or *** Delete File: sections.".into());
    }
    Ok(ops)
}

fn find_block(lines: &[String], from: usize, block: &[String], eof: bool) -> Option<usize> {
    if block.len() > lines.len() {
        return None;
    }
    let last = lines.len() - block.len();
    for pass in 0..3 {
        let same = |a: &str, b: &str| match pass {
            0 => a == b,
            1 => a.trim_end() == b.trim_end(),
            _ => a.trim() == b.trim(),
        };
        let fits = |at: usize| block.iter().enumerate().all(|(k, line)| same(&lines[at + k], line));
        if eof && last >= from && fits(last) {
            return Some(last);
        }
        if let Some(at) = (from.min(last + 1)..=last).find(|at| fits(*at)) {
            return Some(at);
        }
    }
    None
}

fn apply_chunks(label: &str, original: &str, chunks: &[Chunk]) -> Result<String, String> {
    let crlf = original.contains("\r\n");
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    let mut cursor = 0;
    for (index, chunk) in chunks.iter().enumerate() {
        if let Some(anchor) = &chunk.anchor
            && let Some(found) = (cursor..lines.len()).find(|at| lines[*at].trim() == anchor.trim())
        {
            cursor = found + 1;
        }
        if chunk.old.is_empty() {
            let at = if chunk.eof { lines.len() } else { cursor.min(lines.len()) };
            lines.splice(at..at, chunk.new.iter().cloned());
            cursor = at + chunk.new.len();
            continue;
        }
        let at =
            find_block(&lines, cursor, &chunk.old, chunk.eof).or_else(|| find_block(&lines, 0, &chunk.old, chunk.eof)).ok_or(format!(
                "{label}: the lines in change {} were not found. Read the file again and copy the context and removed lines exactly.",
                index + 1
            ))?;
        lines.splice(at..at + chunk.old.len(), chunk.new.iter().cloned());
        cursor = at + chunk.new.len();
    }
    let sep = if crlf { "\r\n" } else { "\n" };
    let mut out = lines.join(sep);
    if !lines.is_empty() && (original.ends_with('\n') || original.is_empty()) {
        out.push_str(sep);
    }
    Ok(out)
}

struct Planned {
    writes: Vec<(PathBuf, Option<String>)>,
    diffs: Vec<FileDiff>,
}

fn plan(root: &std::path::Path, ops: Vec<FileOp>) -> Result<Planned, String> {
    let mut planned = Planned { writes: Vec::new(), diffs: Vec::new() };
    let read = |path: &PathBuf, label: &str| {
        fs::read_to_string(path).map_err(|_| format!("{label} does not exist. Use *** Add File: to create it."))
    };
    for op in ops {
        match op {
            FileOp::Add { path, content } => {
                let target = resolve(root, &path)?;
                let before = fs::read_to_string(&target).ok();
                planned.diffs.push(diff_text(&display(root, &target), before.as_deref(), &content));
                planned.writes.push((target, Some(content)));
            }
            FileOp::Delete { path } => {
                let target = resolve(root, &path)?;
                let before = read(&target, &path)?;
                planned.diffs.push(diff_text(&display(root, &target), Some(&before), ""));
                planned.writes.push((target, None));
            }
            FileOp::Update { path, to, chunks } => {
                let source = resolve(root, &path)?;
                let before = read(&source, &path)?;
                let after = apply_chunks(&path, &before, &chunks)?;
                let target = match &to {
                    Some(to) => resolve(root, to)?,
                    None => source.clone(),
                };
                planned.diffs.push(diff_text(&display(root, &target), Some(&before), &after));
                if target != source {
                    planned.writes.push((source, None));
                }
                planned.writes.push((target, Some(after)));
            }
        }
    }
    Ok(planned)
}

pub struct ApplyPatch;

const PATCH_SPEC: Spec = Spec {
    name: "apply_patch",
    title: "Apply patches",
    summary: "Changes several places or files in one step.",
    aliases: &["patch", "applypatch", "apply_diff"],
    toolset: Toolset::Core,
    category: Category::Edit,
    active: "Patching",
    done: "Patched",
    parallel: false,
    description: "Apply a patch that adds, updates, moves or deletes files, all at once. Format:\n*** Begin Patch\n*** Update File: src/app.py\n@@ def greet():\n-    print(\"hi\")\n+    print(\"hello\")\n*** Add File: notes.txt\n+first line\n*** Delete File: old.txt\n*** End Patch\nIn an update, lines starting with a space are unchanged context, - removes a line and + adds one. Give about three lines of context around each change. @@ followed by a line such as a function header narrows where the change goes. *** Move to: new/path after an Update File line renames the file.",
    guide: "apply_patch changes one or more files in one step using the *** Begin Patch format. Prefer it for edits that touch several places or files.",
};

impl Tool for ApplyPatch {
    fn spec(&self) -> &'static Spec {
        &PATCH_SPEC
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": { "type": "string", "description": "The whole patch, from *** Begin Patch to *** End Patch." }
            },
            "required": ["patch"]
        })
    }

    fn target(&self, args: &Value) -> String {
        let patch = args::text(args, &PATCH).unwrap_or_default();
        let paths: Vec<&str> = patch
            .lines()
            .filter_map(|line| ["*** Add File:", "*** Update File:", "*** Delete File:"].iter().find_map(|h| header(line, h)))
            .collect();
        match paths.as_slice() {
            [] => String::new(),
            [one] => one.replace('\\', "/"),
            [first, rest @ ..] => format!("{} and {} more", first.replace('\\', "/"), rest.len()),
        }
    }

    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String> {
        let patch = args::text(args, &PATCH).ok_or("apply_patch needs a \"patch\".")?;
        let planned = plan(ctx.root, parse(patch)?)?;
        let subject = match planned.diffs.as_slice() {
            [one] => one.path.clone(),
            many => format!("{} files", many.len()),
        };
        let diffs = planned.diffs.clone();
        let mut prepared = Prepared::new(
            Effect::Write,
            subject,
            "Wants to apply a patch",
            Box::new(move |_: &Ctx<'_>| {
                for (path, content) in &planned.writes {
                    let result = match content {
                        Some(text) => write(path, text),
                        None => fs::remove_file(path).map_err(|e| e.to_string()),
                    };
                    if let Err(error) = result {
                        return Output::fail(format!("{}: {error}", path.display()));
                    }
                }
                let summary: Vec<String> = planned
                    .diffs
                    .iter()
                    .map(|diff| {
                        let verb = if diff.created {
                            "Added"
                        } else if diff.added == 0 && diff.removed > 0 {
                            "Changed"
                        } else {
                            "Updated"
                        };
                        format!("{verb} {} (+{} -{})", diff.path, diff.added, diff.removed)
                    })
                    .collect();
                Output { diffs: planned.diffs.clone(), ..Output::ok(summary.join("\n")) }
            }),
        );
        prepared.approval.diffs = diffs;
        Ok(prepared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn patches_are_parsed_into_file_operations() {
        let patch = "*** Begin Patch\n*** Add File: a.txt\n+one\n+two\n*** Update File: b.py\n*** Move to: c.py\n@@ def f():\n-    x = 1\n+    x = 2\n     return x\n*** Delete File: d.txt\n*** End Patch";
        let ops = parse(patch).unwrap();
        assert_eq!(ops[0], FileOp::Add { path: "a.txt".into(), content: "one\ntwo\n".into() });
        assert_eq!(
            ops[1],
            FileOp::Update {
                path: "b.py".into(),
                to: Some("c.py".into()),
                chunks: vec![Chunk {
                    anchor: Some("def f():".into()),
                    old: lines(&["    x = 1", "    return x"]),
                    new: lines(&["    x = 2", "    return x"]),
                    eof: false
                }]
            }
        );
        assert_eq!(ops[2], FileOp::Delete { path: "d.txt".into() });
        assert!(parse("nothing here").is_err());
    }

    #[test]
    fn chunks_apply_with_lenient_matching() {
        let original = "fn a() {\n    one();\n}\n\nfn b() {\n    one();\n}\n";
        let chunks = vec![Chunk { anchor: Some("fn b() {".into()), old: lines(&["    one();"]), new: lines(&["    two();"]), eof: false }];
        assert_eq!(apply_chunks("x", original, &chunks).unwrap(), "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n");
        let loose = vec![Chunk { anchor: None, old: lines(&["fn a() {  "]), new: lines(&["fn first() {"]), eof: false }];
        assert!(apply_chunks("x", original, &loose).unwrap().starts_with("fn first() {\n"));
        let missing = vec![Chunk { anchor: None, old: lines(&["nope"]), new: Vec::new(), eof: false }];
        assert!(apply_chunks("x", original, &missing).unwrap_err().contains("change 1"));
    }

    #[test]
    fn line_endings_and_appends_are_kept() {
        let chunks = vec![Chunk { anchor: None, old: Vec::new(), new: lines(&["three"]), eof: true }];
        assert_eq!(apply_chunks("x", "one\r\ntwo\r\n", &chunks).unwrap(), "one\r\ntwo\r\nthree\r\n");
    }

    #[test]
    fn a_whole_patch_changes_files_together() {
        let root = std::env::temp_dir().join(format!("quill-patch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("keep.txt"), "a\nb\nc\n").unwrap();
        fs::write(root.join("gone.txt"), "bye\n").unwrap();
        let patch = "*** Begin Patch\n*** Update File: keep.txt\n a\n-b\n+B\n c\n*** Add File: new/n.txt\n+hello\n*** Delete File: gone.txt\n*** End Patch";
        let planned = plan(&root, parse(patch).unwrap()).unwrap();
        assert_eq!(planned.diffs.len(), 3);
        for (path, content) in &planned.writes {
            match content {
                Some(text) => write(path, text).unwrap(),
                None => fs::remove_file(path).unwrap(),
            }
        }
        assert_eq!(fs::read_to_string(root.join("keep.txt")).unwrap(), "a\nB\nc\n");
        assert_eq!(fs::read_to_string(root.join("new/n.txt")).unwrap(), "hello\n");
        assert!(!root.join("gone.txt").exists());
        assert!(plan(&root, parse("*** Update File: missing.txt\n-x\n+y").unwrap()).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}