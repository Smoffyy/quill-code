use std::fs;
use std::path::PathBuf;

pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

fn config_dir() -> PathBuf {
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home_dir);
    }
    if cfg!(target_os = "macos") {
        return home_dir().join("Library").join("Application Support");
    }
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|path| path.is_absolute()).unwrap_or_else(|| home_dir().join(".config"))
}

pub fn app_dir() -> PathBuf {
    config_dir().join("quill-code")
}

pub fn personal_instructions_path() -> PathBuf {
    home_dir().join(".quill-code").join("AGENTS.md")
}

pub fn read_personal_instructions() -> String {
    fs::read_to_string(personal_instructions_path()).unwrap_or_default()
}