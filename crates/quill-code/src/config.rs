use std::fs;
use std::path::PathBuf;

use gpui_kit::{App, Global};
use quill_agent::Access;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server_url: String,
    pub model: Option<String>,
    pub thinking: bool,
    #[serde(alias = "mode")]
    pub access: Access,
    pub projects: Vec<PathBuf>,
    pub pinned_models: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server_url: "http://127.0.0.1:9931".into(),
            model: None,
            thinking: true,
            access: Access::Full,
            projects: Vec::new(),
            pinned_models: Vec::new(),
        }
    }
}

impl Global for Config {}

pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

pub fn app_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home_dir).join("quill-code")
}

pub fn projects_dir() -> PathBuf {
    home_dir().join(".quill-code").join("projects")
}

pub fn worktrees_dir() -> PathBuf {
    home_dir().join(".quill-code").join("worktrees")
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

impl Config {
    pub fn load() -> Config {
        fs::read_to_string(app_dir().join("config.json")).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn get(cx: &App) -> &Config {
        cx.global::<Config>()
    }

    pub fn update(cx: &mut App, edit: impl FnOnce(&mut Config)) {
        let config = cx.global_mut::<Config>();
        edit(config);
        let dir = app_dir();
        if let Ok(text) = serde_json::to_string_pretty(config) {
            let _ = fs::create_dir_all(&dir).and_then(|_| fs::write(dir.join("config.json"), text));
        }
    }

    pub fn add_project(cx: &mut App, path: PathBuf) {
        Config::update(cx, |config| {
            config.projects.retain(|p| p != &path);
            config.projects.insert(0, path);
        });
    }
}