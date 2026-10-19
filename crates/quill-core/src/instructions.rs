use std::fs;
use std::path::Path;

const MAX_BYTES: usize = 32 * 1024;

fn expand_imports(root: &Path, text: &str) -> String {
    text.lines()
        .map(|line| {
            let target = line.trim().strip_prefix('@').filter(|rest| !rest.is_empty() && !rest.contains(' '));
            match target.and_then(|rel| fs::read_to_string(root.join(rel)).ok()) {
                Some(imported) => imported,
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn project_instructions(root: &Path, files: &[String]) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut parts = Vec::new();
    for name in files {
        let Ok(raw) = fs::read_to_string(root.join(name)) else { continue };
        let text = expand_imports(root, &raw).trim().to_string();
        if text.is_empty() || seen.contains(&text) {
            continue;
        }
        seen.push(text.clone());
        parts.push(format!("## {name}\n\n{text}"));
    }
    let mut joined = parts.join("\n\n");
    if joined.len() > MAX_BYTES {
        let mut cut = MAX_BYTES;
        while !joined.is_char_boundary(cut) {
            cut -= 1;
        }
        joined.truncate(cut);
        joined.push_str("\n\n[Instructions cut at 32 KB.]");
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_md_importing_agents_md_is_read_once() {
        let dir = std::env::temp_dir().join(format!("quill-instr-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("AGENTS.md"), "Use tabs.").unwrap();
        fs::write(dir.join("CLAUDE.md"), "@AGENTS.md").unwrap();
        let files: Vec<String> = ["AGENTS.md", "CLAUDE.md"].iter().map(|s| s.to_string()).collect();
        let text = project_instructions(&dir, &files);
        assert_eq!(text.matches("Use tabs.").count(), 1);
        assert!(text.starts_with("## AGENTS.md"));
        fs::remove_dir_all(&dir).unwrap();
    }
}