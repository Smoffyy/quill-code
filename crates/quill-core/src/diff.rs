use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    Gap,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub created: bool,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<DiffLine>,
}

pub fn diff_text(path: &str, before: Option<&str>, after: &str) -> FileDiff {
    let old = before.unwrap_or_default();
    let diff = TextDiff::from_lines(old, after);
    let newline_changed = old.ends_with('\n') != after.ends_with('\n');
    let mut lines = Vec::new();
    let (mut added, mut removed) = (0, 0);
    for (index, group) in diff.grouped_ops(3).iter().enumerate() {
        if index > 0 {
            lines.push(DiffLine { kind: LineKind::Gap, old: None, new: None, text: String::new() });
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => LineKind::Context,
                    ChangeTag::Insert => {
                        added += 1;
                        LineKind::Added
                    }
                    ChangeTag::Delete => {
                        removed += 1;
                        LineKind::Removed
                    }
                };
                lines.push(DiffLine {
                    kind,
                    old: change.old_index().map(|i| i + 1),
                    new: change.new_index().map(|i| i + 1),
                    text: change.value().trim_end_matches(['\n', '\r']).to_string(),
                });
                if newline_changed && kind != LineKind::Context && !change.value().ends_with('\n') {
                    lines.push(DiffLine { kind: LineKind::Note, old: None, new: None, text: "No newline at end of file".into() });
                }
            }
        }
    }
    FileDiff { path: path.to_string(), created: before.is_none(), added, removed, lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_files_are_all_additions() {
        let diff = diff_text("a.py", None, "one\ntwo\n");
        assert!(diff.created);
        assert_eq!((diff.added, diff.removed), (2, 0));
        assert_eq!(diff.lines[1].new, Some(2));
    }

    #[test]
    fn edits_keep_context_and_numbers() {
        let before = "a\nb\nc\nd\n";
        let after = "a\nB\nc\nd\n";
        let diff = diff_text("x", Some(before), after);
        assert_eq!((diff.added, diff.removed), (1, 1));
        let removed = diff.lines.iter().find(|l| l.kind == LineKind::Removed).unwrap();
        assert_eq!((removed.old, removed.text.as_str()), (Some(2), "b"));
        assert!(diff.lines.iter().any(|l| l.kind == LineKind::Context));
    }

    #[test]
    fn a_removed_final_newline_is_noted() {
        let kinds = |diff: FileDiff| diff.lines.iter().map(|l| l.kind).collect::<Vec<_>>();
        let removed = diff_text("x", Some("a\nb\n"), "a\nb");
        assert_eq!(kinds(removed), [LineKind::Context, LineKind::Removed, LineKind::Added, LineKind::Note]);
        assert!(!kinds(diff_text("x", Some("a\nb"), "a\nc")).contains(&LineKind::Note));
    }

    #[test]
    fn distant_changes_are_split_by_a_gap() {
        let before: String = (1..=30).map(|n| format!("{n}\n")).collect();
        let after: String = (1..=30)
            .map(|n| match n {
                2 => "two\n".to_string(),
                28 => "twenty-eight\n".to_string(),
                n => format!("{n}\n"),
            })
            .collect();
        let diff = diff_text("x", Some(&before), &after);
        assert_eq!(diff.lines.iter().filter(|l| l.kind == LineKind::Gap).count(), 1);
    }
}