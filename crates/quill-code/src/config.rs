use std::time::SystemTime;

use quill_config::{Settings, settings_path};

pub use quill_config::{app_dir, personal_instructions_path, read_personal_instructions};

pub struct Config {
    pub settings: Settings,
    pub error: Option<String>,
    read_at: Option<SystemTime>,
}

fn modified() -> Option<SystemTime> {
    std::fs::metadata(settings_path()).and_then(|m| m.modified()).ok()
}

impl Config {
    pub fn load() -> Config {
        let loaded = quill_config::load();
        Config { settings: loaded.settings, error: loaded.error, read_at: modified() }
    }

    pub fn update(&mut self, edit: impl FnOnce(&mut Settings)) {
        self.reload();
        edit(&mut self.settings);
        if self.error.is_none() && quill_config::save(&self.settings).is_ok() {
            self.read_at = modified();
        }
    }

    pub fn reload(&mut self) -> bool {
        if self.read_at == modified() {
            return false;
        }
        *self = Config::load();
        true
    }

    pub fn replace(&mut self, settings: Settings) -> std::io::Result<()> {
        self.settings = settings;
        quill_config::save(&self.settings)?;
        self.error = None;
        self.read_at = modified();
        Ok(())
    }
}