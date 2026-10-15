use std::path::{Component, Path, PathBuf};

pub fn normalize(root: &Path, rel: &str) -> Result<PathBuf, String> {
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
    Ok(out)
}

pub fn inside(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().to_lowercase().replace('/', "\\");
    let root = root.to_string_lossy().to_lowercase().replace('/', "\\");
    let root = root.trim_end_matches('\\');
    path == root || path.starts_with(&format!("{root}\\"))
}

pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let path = normalize(root, rel)?;
    if inside(&path, root) { Ok(path) } else { Err(outside()) }
}

fn outside() -> String {
    "That path is outside the project. Use a path relative to the project root, such as \"src/main.rs\".".into()
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).map(|rel| rel.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_cannot_leave_the_project() {
        let root = PathBuf::from(r"C:\work\proj");
        assert!(resolve(&root, "src/main.rs").is_ok());
        assert!(resolve(&root, "../other").is_err());
        assert!(resolve(&root, r"C:\Windows\System32").is_err());
        assert!(resolve(&root, r"c:\WORK\proj\a.txt").is_ok());
        assert!(resolve(&root, r"C:\work\project2\a.txt").is_err());
        assert!(normalize(&root, r"C:\Windows\win.ini").is_ok());
    }

    #[test]
    fn display_paths_are_relative() {
        let root = PathBuf::from(r"C:\work\proj");
        assert_eq!(display(&root, &root.join("src").join("a.rs")), "src/a.rs");
    }
}