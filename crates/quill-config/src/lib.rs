mod models;
mod paths;
mod secrets;

use std::fs;
use std::path::{Path, PathBuf};

use quill_core::{Access, Limits, Rule, Shell};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use models::{
    DEFAULT_SERVER, LOCAL_PORTS, ModelRef, Models, Override, Price, Provider, ProviderKind, Roles, claude_latest, default_price,
    default_providers,
};
pub use paths::{app_dir, home_dir, personal_instructions_path, read_personal_instructions, write_personal_instructions};
pub use secrets::{KeySource, api_key, forget_key, key_source, store_key};

pub const INSTRUCTION_FILES: [&str; 4] = ["AGENTS.md", "CLAUDE.md", "GEMINI.md", ".github/copilot-instructions.md"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub projects: Vec<PathBuf>,
    pub projects_dir: Option<PathBuf>,
    pub worktrees_dir: Option<PathBuf>,
    pub remember_project: bool,
    pub last_project: Option<PathBuf>,
}

impl Default for General {
    fn default() -> Self {
        General { projects: Vec::new(), projects_dir: None, worktrees_dir: None, remember_project: true, last_project: None }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    #[default]
    Full,
    Reduced,
    Off,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reasoning {
    Expanded,
    #[default]
    Collapsed,
    Hidden,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    pub motion: Motion,
    pub reasoning: Reasoning,
    pub transcript_width: u16,
    pub sidebar: bool,
    pub group_exploration: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            theme: Theme::System,
            motion: Motion::Full,
            reasoning: Reasoning::Collapsed,
            transcript_width: 820,
            sidebar: true,
            group_exploration: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Agent {
    pub access: Access,
    pub max_steps: usize,
    pub auto_compact: bool,
    pub compact_at: u8,
    pub parallel_tools: bool,
    pub instruction_files: Vec<String>,
}

impl Default for Agent {
    fn default() -> Self {
        let limits = Limits::default();
        Agent {
            access: Access::Full,
            max_steps: limits.max_steps,
            auto_compact: true,
            compact_at: (limits.compact_at * 100.0).round() as u8,
            parallel_tools: limits.parallel_tools,
            instruction_files: INSTRUCTION_FILES.iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tools {
    pub shell: Shell,
    pub command_timeout: u64,
    pub max_command_timeout: u64,
    pub max_output: usize,
    pub max_read_lines: usize,
    pub max_matches: usize,
    pub max_list: usize,
    pub ignored: Vec<String>,
    pub disabled: Vec<String>,
    pub git: bool,
}

impl Default for Tools {
    fn default() -> Self {
        let limits = Limits::default();
        Tools {
            shell: limits.shell,
            command_timeout: limits.command_timeout,
            max_command_timeout: limits.max_command_timeout,
            max_output: limits.max_output,
            max_read_lines: limits.max_read_lines,
            max_matches: limits.max_matches,
            max_list: limits.max_list,
            ignored: limits.ignored,
            disabled: Vec::new(),
            git: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    pub rules: Vec<Rule>,
    pub safe_commands: Vec<String>,
}

impl Default for Permissions {
    fn default() -> Self {
        Permissions { rules: Vec::new(), safe_commands: Limits::default().safe_commands }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub general: General,
    pub appearance: Appearance,
    pub models: Models,
    pub agent: Agent,
    pub tools: Tools,
    pub permissions: Permissions,
}

#[derive(Deserialize)]
#[serde(default)]
struct Legacy {
    server_url: String,
    model: Option<String>,
    thinking: bool,
    #[serde(alias = "mode")]
    access: Access,
    projects: Vec<PathBuf>,
    pinned_models: Vec<String>,
}

impl Default for Legacy {
    fn default() -> Self {
        Legacy {
            server_url: DEFAULT_SERVER.into(),
            model: None,
            thinking: true,
            access: Access::Full,
            projects: Vec::new(),
            pinned_models: Vec::new(),
        }
    }
}

impl From<Legacy> for Settings {
    fn from(old: Legacy) -> Settings {
        let mut settings = Settings::default();
        settings.models.providers[0].url = old.server_url;
        settings.models.selected = old.model.map(|model| ModelRef::new("local", model));
        settings.models.thinking = old.thinking;
        settings.models.pinned = old.pinned_models.into_iter().map(|model| ModelRef::new("local", model)).collect();
        settings.agent.access = old.access;
        settings.general.projects = old.projects;
        settings
    }
}

pub fn settings_path() -> PathBuf {
    app_dir().join("settings.json")
}

pub fn project_settings_path(root: &Path) -> PathBuf {
    root.join(".quill").join("settings.json")
}

pub fn merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(base), Value::Object(over)) => {
            for (key, value) in over {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (slot, value) => *slot = value,
    }
}

pub struct Loaded {
    pub settings: Settings,
    pub error: Option<String>,
}

pub fn parse(text: &str) -> Result<Settings, String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| format!("settings.json could not be read: {e}"))?;
    migrate(&mut value);
    serde_json::from_value(value).map_err(|e| format!("settings.json could not be read: {e}"))
}

fn migrate(value: &mut Value) {
    let Some(models) = value.get_mut("models").and_then(Value::as_object_mut) else { return };
    let Some(url) = models.remove("server_url") else { return };
    if !models.contains_key("providers") {
        let mut providers = default_providers();
        if let Some(url) = url.as_str() {
            providers[0].url = url.to_string();
        }
        models.insert("providers".into(), serde_json::to_value(providers).unwrap_or_default());
    }
}

pub fn load() -> Loaded {
    match fs::read_to_string(settings_path()) {
        Ok(text) => match parse(&text) {
            Ok(settings) => Loaded { settings, error: None },
            Err(error) => Loaded { settings: Settings::default(), error: Some(error) },
        },
        Err(_) => {
            let settings = fs::read_to_string(app_dir().join("config.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<Legacy>(&text).ok())
                .map(Settings::from)
                .unwrap_or_default();
            let _ = save(&settings);
            Loaded { settings, error: None }
        }
    }
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
    fs::create_dir_all(app_dir())?;
    fs::write(settings_path(), text)
}

impl Settings {
    pub fn for_project(&self, root: Option<&Path>) -> Settings {
        let Some(text) = root.and_then(|root| fs::read_to_string(project_settings_path(root)).ok()) else { return self.clone() };
        let Ok(over) = serde_json::from_str::<Value>(&text) else { return self.clone() };
        let Ok(mut base) = serde_json::to_value(self) else { return self.clone() };
        merge(&mut base, over);
        serde_json::from_value(base).unwrap_or_else(|_| self.clone())
    }

    pub fn limits(&self) -> Limits {
        Limits {
            max_steps: self.agent.max_steps.max(1),
            command_timeout: self.tools.command_timeout.max(1),
            max_command_timeout: self.tools.max_command_timeout.max(self.tools.command_timeout).max(1),
            max_output: self.tools.max_output.max(1000),
            max_read_lines: self.tools.max_read_lines.max(10),
            max_matches: self.tools.max_matches.max(1),
            max_list: self.tools.max_list.max(10),
            compact_at: if self.agent.auto_compact { f32::from(self.agent.compact_at.clamp(30, 98)) / 100.0 } else { f32::MAX },
            parallel_tools: self.agent.parallel_tools,
            shell: self.tools.shell,
            ignored: self.tools.ignored.clone(),
            safe_commands: self.permissions.safe_commands.clone(),
            disabled_tools: self.tools.disabled.clone(),
        }
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.general.projects_dir.clone().unwrap_or_else(|| home_dir().join(".quill-code").join("projects"))
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.general.worktrees_dir.clone().unwrap_or_else(|| home_dir().join(".quill-code").join("worktrees"))
    }

    pub fn add_project(&mut self, path: PathBuf) {
        self.general.projects.retain(|p| p != &path);
        self.general.projects.insert(0, path);
    }

    pub fn value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn defaults() -> Value {
        Settings::default().value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core::RuleAction;
    use serde_json::json;

    #[test]
    fn missing_fields_take_defaults() {
        let settings = parse(r#"{ "agent": { "max_steps": 12 } }"#).unwrap();
        assert_eq!(settings.agent.max_steps, 12);
        assert_eq!(settings.tools, Tools::default());
        assert_eq!(settings.models.providers[0].url, DEFAULT_SERVER);
        assert!(parse("{ broken").is_err());
    }

    #[test]
    fn a_single_server_becomes_the_local_provider() {
        let settings = parse(r#"{ "models": { "server_url": "http://box:8080", "selected": "qwen", "pinned": ["qwen"] } }"#).unwrap();
        assert_eq!(settings.models.providers.len(), 4);
        assert_eq!(settings.models.provider("local").unwrap().url, "http://box:8080");
        assert_eq!(settings.models.selected, Some(ModelRef::new("local", "qwen")));
        assert_eq!(settings.models.pinned, vec![ModelRef::new("local", "qwen")]);
    }

    #[test]
    fn old_config_files_migrate() {
        let old: Legacy = serde_json::from_value(json!({
            "server_url": "http://box:8080", "model": "qwen", "thinking": false, "mode": "manual",
            "projects": ["C:\\work\\a"], "pinned_models": ["qwen"]
        }))
        .unwrap();
        let settings = Settings::from(old);
        assert_eq!(settings.models.providers[0].url, "http://box:8080");
        assert_eq!(settings.models.selected, Some(ModelRef::new("local", "qwen")));
        assert!(!settings.models.thinking);
        assert_eq!(settings.agent.access, Access::Supervised);
        assert_eq!(settings.general.projects, vec![PathBuf::from("C:\\work\\a")]);
        assert_eq!(settings.models.pinned, vec![ModelRef::new("local", "qwen")]);
    }

    #[test]
    fn objects_merge_and_values_replace() {
        let mut base = json!({ "agent": { "max_steps": 100, "access": "full" }, "tools": { "ignored": ["a", "b"] } });
        merge(&mut base, json!({ "agent": { "access": "supervised" }, "tools": { "ignored": ["c"] } }));
        assert_eq!(base, json!({ "agent": { "max_steps": 100, "access": "supervised" }, "tools": { "ignored": ["c"] } }));
    }

    #[test]
    fn projects_can_override_their_own_settings() {
        let root = std::env::temp_dir().join(format!("quill-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".quill")).unwrap();
        fs::write(
            project_settings_path(&root),
            r#"{ "agent": { "access": "supervised" }, "permissions": { "rules": [{ "tool": "shell", "pattern": "npm test", "action": "allow" }] } }"#,
        )
        .unwrap();
        let user = Settings { agent: Agent { max_steps: 7, ..Agent::default() }, ..Settings::default() };
        let effective = user.for_project(Some(&root));
        assert_eq!(effective.agent.access, Access::Supervised);
        assert_eq!(effective.agent.max_steps, 7);
        assert_eq!(effective.permissions.rules[0].action, RuleAction::Allow);
        assert_eq!(user.for_project(None), user);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn limits_follow_the_settings() {
        let mut settings = Settings::default();
        settings.tools.shell = Shell::Cmd;
        settings.tools.disabled = vec!["shell".into()];
        settings.agent.compact_at = 70;
        let limits = settings.limits();
        assert_eq!(limits.shell, Shell::Cmd);
        assert_eq!(limits.disabled_tools, vec!["shell".to_string()]);
        assert!((limits.compact_at - 0.7).abs() < 0.001);
        settings.agent.auto_compact = false;
        assert_eq!(settings.limits().compact_at, f32::MAX);
        assert_eq!(Settings::default().limits(), Limits::default());
    }
}