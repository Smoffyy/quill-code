use std::fs;
use std::path::Path;

use globset::GlobBuilder;

pub fn final_newline(path: &Path) -> Option<bool> {
    for dir in path.ancestors().skip(1) {
        let Ok(text) = fs::read_to_string(dir.join(".editorconfig")) else { continue };
        let rel = path.strip_prefix(dir).map(|rel| rel.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        let (value, root) = read(&text, &rel);
        if value.is_some() || root {
            return value;
        }
    }
    None
}

fn read(text: &str, rel: &str) -> (Option<bool>, bool) {
    let mut root = false;
    let mut section = None;
    let mut value = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(pattern) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
            section = Some(matches(pattern, rel));
            continue;
        }
        let Some((key, val)) = line.split_once('=') else { continue };
        let (key, val) = (key.trim().to_ascii_lowercase(), val.trim().to_ascii_lowercase());
        match section {
            None if key == "root" => root = val == "true",
            Some(true) if key == "insert_final_newline" => value = val.parse().ok(),
            _ => {}
        }
    }
    (value, root)
}

fn matches(pattern: &str, rel: &str) -> bool {
    let pattern = match pattern.strip_prefix('/') {
        Some(anchored) => anchored.to_string(),
        None if pattern.contains('/') => pattern.to_string(),
        None => format!("**/{pattern}"),
    };
    GlobBuilder::new(&pattern).literal_separator(true).build().is_ok_and(|glob| glob.compile_matcher().is_match(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_matching_section_decides() {
        let text = "root = true\n\n[*]\ninsert_final_newline = true\n\n[*.{md,txt}]\ninsert_final_newline = false\n\n[docs/*.js]\ninsert_final_newline = false";
        assert_eq!(read(text, "src/main.rs"), (Some(true), true));
        assert_eq!(read(text, "notes/a.md"), (Some(false), true));
        assert_eq!(read(text, "docs/a.js"), (Some(false), true));
        assert_eq!(read(text, "src/docs/a.js"), (Some(true), true));
        assert_eq!(read("[*.py]\ninsert_final_newline = true", "a.rs"), (None, false));
    }
}