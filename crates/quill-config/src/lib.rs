mod models;
mod paths;

use std::fs;
use std::path::{Path, PathBuf};

use quill_core::{Access, FinalNewline, Limits, Rule, Shell};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use models::{ModelRef, Models, Override, Provider, Roles, ServerKind};
pub use paths::{app_dir, home_dir, personal_instructions_path, read_personal_instructions};

pub const INSTRUCTION_FILES: [&str; 4] = ["AGENTS.md", "CLAUDE.md", "GEMINI.md", ".github/copilot-instructions.md"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub worktrees_dir: Option<PathBuf>,
    pub trusted: Vec<PathBuf>,
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
    pub reasoning: Reasoning,
    pub group_exploration: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance { reasoning: Reasoning::Collapsed, group_exploration: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Agent {
    pub access: Access,
    pub max_steps: usize,
    pub auto_compact: bool,
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
    pub ignored: Vec<String>,
    pub disabled: Vec<String>,
    pub final_newline: FinalNewline,
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
            ignored: limits.ignored,
            disabled: Vec::new(),
            final_newline: limits.final_newline,
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
    serde_json::from_str(text).map_err(|e| format!("settings.json could not be read: {e}"))
}

pub fn load() -> Loaded {
    match fs::read_to_string(settings_path()) {
        Ok(text) if !text.trim().is_empty() => match parse(&text) {
            Ok(settings) => Loaded { settings, error: None },
            Err(error) => Loaded { settings: Settings::default(), error: Some(error) },
        },
        _ => {
            let settings = Settings::default();
            let _ = save(&settings);
            Loaded { settings, error: None }
        }
    }
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    let path = settings_path();
    let value = keep_unknown(&path, serde_json::to_value(settings).map_err(std::io::Error::other)?);
    write_atomic(&path, &serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?)
}

pub fn keep_unknown(path: &Path, value: Value) -> Value {
    let mut saved = fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null);
    merge(&mut saved, value);
    saved
}

pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temp, text)?;
    fs::rename(&temp, path)
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
            auto_compact: self.agent.auto_compact,
            parallel_tools: self.agent.parallel_tools,
            shell: self.tools.shell,
            ignored: self.tools.ignored.clone(),
            safe_commands: self.permissions.safe_commands.clone(),
            disabled_tools: self.tools.disabled.clone(),
            final_newline: self.tools.final_newline,
        }
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.general.worktrees_dir.clone().unwrap_or_else(|| home_dir().join(".quill-code").join("worktrees"))
    }

    pub fn reset(&self) -> Settings {
        let mut fresh = Settings::default();
        fresh.models.providers = self.models.providers.clone();
        fresh.models.selected = self.models.selected.clone();
        fresh.models.pinned = self.models.pinned.clone();
        fresh.models.profiles = self.models.profiles.clone();
        fresh.general.trusted = self.general.trusted.clone();
        fresh
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
        assert!(parse("{ broken").is_err());
    }

    #[test]
    fn settings_from_the_desktop_app_still_load() {
        let old = r#"{
            "general": { "projects": ["C:\\work\\a"], "projects_dir": null, "worktrees_dir": "D:\\trees", "remember_project": true },
            "appearance": { "theme": "dark", "motion": "off", "reasoning": "hidden", "transcript_width": 820, "sidebar": false }
        }"#;
        let settings = parse(old).unwrap();
        assert_eq!(settings.general.worktrees_dir, Some(PathBuf::from("D:\\trees")));
        assert_eq!(settings.appearance.reasoning, Reasoning::Hidden);
        assert!(settings.appearance.group_exploration);
        let saved = serde_json::to_string(&settings).unwrap();
        assert!(!saved.contains("theme") && !saved.contains("projects"));
    }

    #[test]
    fn a_reset_keeps_the_models() {
        let mut settings = Settings::default();
        settings.models.selected = Some(ModelRef::new("local", "qwen"));
        settings.models.pinned = vec![ModelRef::new("local", "qwen")];
        settings.models.providers.push(Provider::new("ollama", ServerKind::Ollama, "http://box:11434"));
        settings.agent.max_steps = 3;
        settings.appearance.reasoning = Reasoning::Expanded;
        let fresh = settings.reset();
        assert_eq!(fresh.models.selected, settings.models.selected);
        assert_eq!(fresh.models.pinned, settings.models.pinned);
        assert_eq!(fresh.models.providers, settings.models.providers);
        assert_eq!(fresh.agent, Agent::default());
        assert_eq!(fresh.appearance, Appearance::default());
    }

    #[test]
    fn objects_merge_and_values_replace() {
        let mut base = json!({ "agent": { "max_steps": 100, "access": "full" }, "tools": { "ignored": ["a", "b"] } });
        merge(&mut base, json!({ "agent": { "access": "supervised" }, "tools": { "ignored": ["c"] } }));
        assert_eq!(base, json!({ "agent": { "max_steps": 100, "access": "supervised" }, "tools": { "ignored": ["c"] } }));
    }

    #[test]
    fn saving_keeps_what_a_newer_version_wrote() {
        let path = std::env::temp_dir().join(format!("quill-keep-{}.json", std::process::id()));
        fs::write(&path, r#"{ "agent": { "max_steps": 5, "later": true }, "newer": [1] }"#).unwrap();
        let value = keep_unknown(&path, json!({ "agent": { "max_steps": 9 }, "tools": {} }));
        assert_eq!(value, json!({ "agent": { "max_steps": 9, "later": true }, "newer": [1], "tools": {} }));
        fs::write(&path, "{ broken").unwrap();
        assert_eq!(keep_unknown(&path, json!({ "a": 1 })), json!({ "a": 1 }));
        fs::remove_file(&path).unwrap();
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
        let limits = settings.limits();
        assert_eq!(limits.shell, Shell::Cmd);
        assert_eq!(limits.disabled_tools, vec!["shell".to_string()]);
        settings.agent.auto_compact = false;
        assert!(!settings.limits().auto_compact);
        assert_eq!(Settings::default().limits(), Limits::default());
    }
}