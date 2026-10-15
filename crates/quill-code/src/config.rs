use std::path::PathBuf;
use std::time::SystemTime;

use gpui_kit::{App, Global};
use quill_config::{Settings, settings_path};

pub use quill_config::{app_dir, personal_instructions_path, read_personal_instructions, write_personal_instructions};

pub struct Config {
    settings: Settings,
    error: Option<String>,
    read_at: Option<SystemTime>,
}

impl Global for Config {}

fn modified() -> Option<SystemTime> {
    std::fs::metadata(settings_path()).and_then(|m| m.modified()).ok()
}

impl Config {
    pub fn load() -> Config {
        let loaded = quill_config::load();
        Config { settings: loaded.settings, error: loaded.error, read_at: modified() }
    }

    pub fn get(cx: &App) -> &Settings {
        &cx.global::<Config>().settings
    }

    pub fn error(cx: &App) -> Option<String> {
        cx.global::<Config>().error.clone()
    }

    pub fn update(cx: &mut App, edit: impl FnOnce(&mut Settings)) {
        let config = cx.global_mut::<Config>();
        edit(&mut config.settings);
        if config.error.is_none() && quill_config::save(&config.settings).is_ok() {
            config.read_at = modified();
        }
    }

    pub fn reload(cx: &mut App) -> bool {
        if cx.global::<Config>().read_at == modified() {
            return false;
        }
        cx.set_global(Config::load());
        true
    }

    pub fn replace(cx: &mut App, settings: Settings) {
        let config = cx.global_mut::<Config>();
        config.error = None;
        config.settings = settings;
        if quill_config::save(&config.settings).is_ok() {
            config.read_at = modified();
        }
    }

    pub fn add_project(cx: &mut App, path: PathBuf) {
        Config::update(cx, |settings| settings.add_project(path));
    }
}