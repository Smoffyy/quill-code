use std::fs;
use std::path::PathBuf;

pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

pub fn app_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home_dir).join("quill-code")
}

pub fn personal_instructions_path() -> PathBuf {
    home_dir().join(".quill-code").join("AGENTS.md")
}

pub fn read_personal_instructions() -> String {
    fs::read_to_string(personal_instructions_path()).unwrap_or_default()
}

pub fn write_personal_instructions(text: &str) -> std::io::Result<()> {
    let path = personal_instructions_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, text)
}