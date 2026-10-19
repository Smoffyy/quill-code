use std::path::{Path, PathBuf};
use std::process::Command;

use quill_git::github::{self, Release};

use crate::picker::Choice;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn slug() -> &'static str {
    REPOSITORY.trim_start_matches("https://github.com/")
}

fn parts(version: &str) -> Vec<u64> {
    version.split(['.', '-', '+']).take(3).map(|part| part.parse().unwrap_or(0)).collect()
}

fn installer(name: &str) -> String {
    format!("{REPOSITORY}/releases/latest/download/{name}")
}

fn built_here(exe: &Path) -> Option<&'static str> {
    let path = exe.to_string_lossy().replace('\\', "/").to_lowercase();
    if path.contains("/.cargo/bin/") {
        return Some("This copy of Quill Code was installed with cargo. Update it with: cargo install --path crates/quill-code --force");
    }
    if path.contains("/target/debug/") || path.contains("/target/release/") {
        return Some("This copy of Quill Code was built from source. Pull the latest code and build it again.");
    }
    None
}

fn old_copy(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

pub fn tidy() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_copy(&exe));
    }
}

pub fn source_note() -> Option<&'static str> {
    std::env::current_exe().ok().and_then(|exe| built_here(&exe))
}

pub fn releases() -> Result<Vec<Release>, String> {
    let mut releases = github::releases(slug())?;
    releases.sort_by_key(|release| std::cmp::Reverse(parts(&release.version)));
    Ok(releases)
}

pub fn newest(releases: &[Release]) -> Option<&Release> {
    releases.iter().find(|release| !release.prerelease)
}

pub fn tags(release: &Release, newest: Option<&str>) -> Vec<&'static str> {
    let mut tags = Vec::new();
    if release.version == VERSION {
        tags.push("installed");
    }
    if newest == Some(release.version.as_str()) {
        tags.push("latest");
    }
    if release.prerelease {
        tags.push("preview");
    }
    tags
}

pub fn choices(releases: &[Release]) -> Vec<Choice<String>> {
    let newest = newest(releases).map(|release| release.version.clone());
    releases
        .iter()
        .map(|release| {
            let mut detail = vec![release.date.clone()];
            detail.extend(tags(release, newest.as_deref()).into_iter().map(str::to_string));
            Choice::new(release.version.clone(), detail.join(" · "), release.version.clone()).current(release.version == VERSION)
        })
        .collect()
}

pub fn versions() -> u8 {
    let releases = match releases() {
        Ok(releases) => releases,
        Err(error) => {
            eprintln!("Could not list the releases. {error}");
            return 1;
        }
    };
    if releases.is_empty() {
        println!("There is no published release yet.");
        return 0;
    }
    let newest = newest(&releases).map(|release| release.version.clone());
    for release in &releases {
        let tags = tags(release, newest.as_deref());
        let tags = if tags.is_empty() { String::new() } else { format!("  ({})", tags.join(", ")) };
        println!("{:<10} {:<10}  {}{tags}", release.version, release.date, release.title);
    }
    println!("\nSwitch to one with: quill update <version>");
    0
}

pub fn run(version: Option<String>) -> u8 {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("Could not find where Quill Code is installed: {error}");
            return 1;
        }
    };
    if let Some(note) = built_here(&exe) {
        eprintln!("{note}");
        return 1;
    }
    eprintln!("Checking the releases…");
    let releases = match releases() {
        Ok(releases) => releases,
        Err(error) => {
            eprintln!("Could not check for updates. {error}");
            return 1;
        }
    };
    let target = match &version {
        Some(wanted) => releases.iter().find(|release| release.version == wanted.trim_start_matches('v')),
        None => newest(&releases),
    };
    let Some(target) = target else {
        match version {
            Some(wanted) => eprintln!("There is no release {wanted}. See them with: quill versions"),
            None => eprintln!("There is no published release yet."),
        }
        return 1;
    };
    if target.version == VERSION {
        let state = if version.is_some() { "already installed" } else { "the latest version" };
        println!("Quill Code {VERSION} is {state}.");
        return 0;
    }
    match install(&exe, &target.version) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

pub fn switch(version: &str, root: &Path, resume: bool) -> u8 {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("Could not find where Quill Code is installed: {error}");
            return 1;
        }
    };
    if let Err(error) = install(&exe, version) {
        eprintln!("{error}");
        return 1;
    }
    let mut command = Command::new(&exe);
    command.arg("-C").arg(root);
    if resume {
        command.arg("-c");
    }
    match command.status() {
        Ok(status) => status.code().unwrap_or(1) as u8,
        Err(error) => {
            eprintln!("Quill Code {version} is installed, but it could not start: {error}");
            1
        }
    }
}

fn install(exe: &Path, version: &str) -> Result<(), String> {
    eprintln!("Switching Quill Code {VERSION} to {version}…");
    let mut command = if cfg!(windows) {
        let mut command = Command::new("powershell");
        command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &format!("irm {} | iex", installer("install.ps1"))]);
        command
    } else {
        let url = installer("install.sh");
        let mut command = Command::new("sh");
        command.args(["-c", &format!("(curl -fsSL '{url}' || wget -qO- '{url}') | sh")]);
        command
    };
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let status = command.env("QUILL_VERSION", version).env("QUILL_INSTALL_DIR", &dir).env("QUILL_NO_MODIFY_PATH", "1").status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(format!("The switch did not finish. Quill Code {VERSION} is still installed.")),
        Err(error) => Err(format!("The installer could not start: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str, prerelease: bool) -> Release {
        Release { version: version.into(), title: String::new(), date: String::new(), prerelease }
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(parts("0.10.0") > parts("0.9.3"));
        assert!(parts("1.0.0") > parts("0.99.0"));
        assert!(parts("0.2.1") > parts("0.2.0-dev"));
        assert_eq!(parts("0.2.0"), parts("0.2.0"));
    }

    #[test]
    fn the_newest_release_skips_previews() {
        let releases = vec![release("2.0.0", true), release("1.1.0", false), release("1.0.0", false)];
        assert_eq!(newest(&releases).map(|release| release.version.as_str()), Some("1.1.0"));
        assert_eq!(tags(&releases[0], Some("1.1.0")), vec!["preview"]);
        assert_eq!(tags(&releases[1], Some("1.1.0")), vec!["latest"]);
    }

    #[test]
    fn the_picker_marks_the_running_release() {
        let mut releases = vec![release("9.0.0", false), release(VERSION, false)];
        releases[1].date = "2026-10-16".into();
        let choices = choices(&releases);
        assert_eq!(choices[0].detail, " · latest");
        assert!(!choices[0].current && choices[1].current);
        assert_eq!(choices[1].detail, "2026-10-16 · installed");
        assert_eq!(choices[1].value, VERSION);
    }

    #[test]
    fn source_builds_are_told_how_to_update() {
        assert!(built_here(Path::new("/home/me/.cargo/bin/quill")).is_some());
        assert!(built_here(Path::new("D:\\repos\\quill-code\\target\\release\\quill.exe")).is_some());
        assert!(built_here(Path::new("/home/me/.local/bin/quill")).is_none());
        assert_eq!(old_copy(Path::new("/opt/quill.exe")), Path::new("/opt/quill.exe.old"));
        assert_eq!(slug(), "Smoffyy/quill-code");
    }
}