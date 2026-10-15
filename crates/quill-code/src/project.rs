use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gpui_kit::{Hsla, hsla};

use quill_git::github::{self, Visibility};
use quill_git::{self as git, hidden};

pub fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let letters: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [first, second, ..] => first.chars().take(1).chain(second.chars().take(1)).collect(),
    };
    letters.to_lowercase()
}

pub fn color(name: &str) -> Hsla {
    hsla((crate::ui::name_hash(name) % 360) as f32 / 360., 0.55, 0.42, 1.)
}

pub fn git_available() -> bool {
    hidden("git").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub fn editor_available() -> bool {
    hidden("cmd").args(["/C", "where", "code"]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub fn open_in_editor(path: &Path) {
    let _ = hidden("cmd").args(["/C", "code"]).arg(path).spawn();
}

pub fn open_folder(path: &Path) {
    let _ = Command::new("explorer").arg(path).spawn();
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
    format!("{}\\…\\{}", parts[0], parts[parts.len() - 2..].join("\\"))
}

pub fn open_file(path: &Path) {
    let _ = hidden("cmd").args(["/C", "start", ""]).arg(path).spawn();
}

pub fn clean_name(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect::<String>()
        .trim_matches(['-', '.'])
        .to_string()
}

pub struct Created {
    pub path: PathBuf,
    pub note: Option<String>,
}

pub fn create(base: &Path, raw: &str, git_branch: Option<&str>, publish: Option<Visibility>) -> Result<Created, String> {
    let name = clean_name(raw);
    if name.is_empty() {
        return Err("Give the project a name.".into());
    }
    let dir = base.join(&name);
    if dir.exists() {
        return Err(format!("{} already exists.", dir.display()));
    }
    let repo = publish.map(|visibility| github::create_repo(&name, visibility)).transpose()?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join("README.md"), format!("# {name}")).map_err(|e| e.to_string())?;
    let Some(branch) = git_branch else { return Ok(Created { path: dir, note: None }) };
    git::init(&dir, branch, &format!("Start {name}"))?;
    let note = repo.map(|repo| match git::add_remote(&dir, &repo.clone_url).and_then(|()| git::push(&dir, branch)) {
        Ok(()) => format!("Published to {}", repo.html_url),
        Err(error) => format!("Created on GitHub, but the first push failed: {error}"),
    });
    Ok(Created { path: dir, note })
}

pub fn repo_name(url: &str) -> String {
    let last = url.trim().trim_end_matches('/').rsplit(['/', ':']).next().unwrap_or_default();
    clean_name(last.trim_end_matches(".git"))
}

pub fn clone(base: &Path, url: &str) -> Result<PathBuf, String> {
    let name = repo_name(url);
    if name.is_empty() {
        return Err("That does not look like a Git URL.".into());
    }
    fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let mut dir = base.join(&name);
    let mut n = 2;
    while dir.exists() {
        dir = base.join(format!("{name}-{n}"));
        n += 1;
    }
    let output = hidden("git")
        .args(["clone", "--", url.trim()])
        .arg(&dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("Git could not start: {e}"))?;
    if output.status.success() {
        Ok(dir)
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        Err(error.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("The clone failed.").trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_come_from_words() {
        assert_eq!(initials("quill-probe"), "qp");
        assert_eq!(initials("website"), "we");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn repo_names_come_from_urls() {
        assert_eq!(repo_name("https://github.com/ggml-org/llama.cpp.git"), "llama.cpp");
        assert_eq!(repo_name("git@github.com:me/tool.git"), "tool");
        assert_eq!(repo_name("https://example.com/a/b/"), "b");
    }

    #[test]
    fn names_are_made_safe_for_folders() {
        assert_eq!(clean_name("  My App! "), "My-App");
        assert_eq!(clean_name("../x"), "x");
    }
}