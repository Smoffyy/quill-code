use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use quill_git::hidden;

pub fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

pub fn git_available() -> bool {
    hidden("git").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub fn short_path(path: &Path) -> String {
    let home = quill_config::home_dir();
    let shown = match path.strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => Path::new("~").join(rest),
        _ => path.to_path_buf(),
    };
    let parts: Vec<String> = shown.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if parts.len() <= 4 {
        return shown.display().to_string();
    }
    let separator = std::path::MAIN_SEPARATOR_STR;
    let head = if parts[0] == separator { "" } else { parts[0].as_str() };
    format!("{head}{separator}…{separator}{}", parts[parts.len() - 2..].join(separator))
}

fn key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.trim_end_matches('/');
    if cfg!(windows) { text.to_lowercase() } else { text.to_string() }
}

pub fn same(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

pub fn within(path: &Path, folder: &Path) -> bool {
    let (path, folder) = (key(path), key(folder));
    path == folder || path.starts_with(&format!("{folder}/"))
}

pub fn clean_name(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect::<String>()
        .trim_matches(['-', '.'])
        .to_string()
}

pub fn open_url(url: &str) {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("explorer");
        command.arg(url);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    let _ = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

pub fn edit(path: &Path) -> Result<(), String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .filter(|editor| !editor.trim().is_empty())
        .unwrap_or_else(|| if cfg!(windows) { "notepad".into() } else { "vi".into() });
    let mut words = editor.split_whitespace();
    let program = words.next().unwrap_or_default();
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(program);
        command
    } else {
        Command::new(program)
    };
    let status = command
        .args(words)
        .arg(path)
        .status()
        .map_err(|e| format!("{program} could not start: {e}. Set VISUAL or EDITOR to your editor."))?;
    if status.success() { Ok(()) } else { Err(format!("{program} exited with {status}.")) }
}

pub fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe_for_folders() {
        assert_eq!(clean_name("  My App! "), "My-App");
        assert_eq!(clean_name("../x"), "x");
    }

    #[test]
    fn folders_contain_their_subfolders() {
        assert!(within(Path::new("/work/app/src"), Path::new("/work/app")));
        assert!(within(Path::new("/work/app"), Path::new("/work/app/")));
        assert!(!within(Path::new("/work/apple"), Path::new("/work/app")));
    }

    #[test]
    fn paths_compare_the_way_the_system_does() {
        assert!(same(Path::new("/work/app/"), Path::new("/work/app")));
        assert_eq!(same(Path::new("C:\\Work\\App"), Path::new("c:/work/app")), cfg!(windows));
        assert!(!same(Path::new("/work/app"), Path::new("/work/other")));
    }

    #[test]
    fn long_paths_are_shortened() {
        let separator = std::path::MAIN_SEPARATOR_STR;
        assert_eq!(short_path(Path::new("/a/b/c/d/e/f")), format!("{separator}…{separator}e{separator}f"));
        assert_eq!(short_path(Path::new("/a/b")), Path::new("/a/b").display().to_string());
    }
}