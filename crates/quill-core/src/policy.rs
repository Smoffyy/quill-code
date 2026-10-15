use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    #[serde(alias = "manual")]
    Supervised,
    AutoEdits,
    #[default]
    #[serde(alias = "auto")]
    Full,
}

impl Access {
    pub const ALL: [Access; 3] = [Access::Supervised, Access::AutoEdits, Access::Full];

    pub fn label(self) -> &'static str {
        match self {
            Access::Supervised => "Ask first",
            Access::AutoEdits => "Trust edits",
            Access::Full => "Autopilot",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Access::Supervised => "Checks with you before every change and command",
            Access::AutoEdits => "Makes edits and read-only commands on its own, checks before the rest",
            Access::Full => "Works without stopping to ask, except before fetching from the web",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Read,
    ReadOutside,
    Write,
    Execute,
    Publish,
    Network,
    Desktop,
    Delegate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub tool: String,
    #[serde(default)]
    pub pattern: String,
    pub action: RuleAction,
}

impl Rule {
    pub fn allow(tool: &str, pattern: &str) -> Rule {
        Rule { tool: tool.to_string(), pattern: pattern.to_string(), action: RuleAction::Allow }
    }

    pub fn matches(&self, tool: &str, subject: &str) -> bool {
        (self.tool == "*" || self.tool.eq_ignore_ascii_case(tool)) && pattern_matches(&self.pattern, subject)
    }
}

fn pattern_matches(pattern: &str, subject: &str) -> bool {
    let pattern = pattern.trim().to_lowercase();
    let subject = subject.trim().to_lowercase();
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    match pattern.strip_suffix('*') {
        Some(prefix) => subject.starts_with(prefix),
        None => subject == pattern || subject.starts_with(&format!("{pattern} ")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Ask,
    Deny(String),
}

pub fn verdict(access: Access, effect: Effect, tool: &str, subject: &str, rules: &[Rule], safe_commands: &[String]) -> Verdict {
    if let Some(rule) = rules.iter().rev().find(|rule| rule.matches(tool, subject)) {
        return match rule.action {
            RuleAction::Allow => Verdict::Allow,
            RuleAction::Ask => Verdict::Ask,
            RuleAction::Deny => Verdict::Deny(format!("A rule in the user's settings blocks {tool} here.")),
        };
    }
    match effect {
        Effect::Read => Verdict::Allow,
        Effect::Network => Verdict::Ask,
        _ if access == Access::Full => Verdict::Allow,
        Effect::Write if access == Access::AutoEdits => Verdict::Allow,
        Effect::Execute if access == Access::AutoEdits && read_only(subject, safe_commands) => Verdict::Allow,
        _ => Verdict::Ask,
    }
}

pub fn read_only(command: &str, safe_commands: &[String]) -> bool {
    if command.contains('>') || command.contains('`') || command.contains("$(") {
        return false;
    }
    let mut parts = command.split(['|', ';', '&', '\n']).map(str::trim).filter(|part| !part.is_empty()).peekable();
    parts.peek().is_some() && parts.all(|part| safe_commands.iter().any(|safe| pattern_matches(safe, part)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe() -> Vec<String> {
        ["git status", "Get-ChildItem", "Select-String"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn access_reads_old_settings() {
        let auto: Access = serde_json::from_str("\"auto\"").unwrap();
        let manual: Access = serde_json::from_str("\"manual\"").unwrap();
        assert_eq!((auto, manual), (Access::Full, Access::Supervised));
        assert_eq!(serde_json::to_string(&Access::AutoEdits).unwrap(), "\"auto_edits\"");
    }

    #[test]
    fn presets_decide_by_effect() {
        let ask = |access, effect| verdict(access, effect, "t", "x", &[], &safe());
        assert_eq!(ask(Access::Supervised, Effect::Read), Verdict::Allow);
        assert_eq!(ask(Access::Supervised, Effect::Write), Verdict::Ask);
        assert_eq!(ask(Access::AutoEdits, Effect::Write), Verdict::Allow);
        assert_eq!(ask(Access::AutoEdits, Effect::Execute), Verdict::Ask);
        assert_eq!(ask(Access::Full, Effect::Execute), Verdict::Allow);
        assert_eq!(ask(Access::Full, Effect::ReadOutside), Verdict::Allow);
        assert_eq!(ask(Access::Full, Effect::Network), Verdict::Ask);
        assert_eq!(verdict(Access::AutoEdits, Effect::Execute, "shell", "git status", &[], &safe()), Verdict::Allow);
    }

    #[test]
    fn rules_win_over_presets() {
        let rules = vec![Rule::allow("shell", "npm test"), Rule { tool: "*".into(), pattern: "rm*".into(), action: RuleAction::Deny }];
        assert_eq!(verdict(Access::Supervised, Effect::Execute, "shell", "npm test -- --watch", &rules, &[]), Verdict::Allow);
        assert_eq!(verdict(Access::Supervised, Effect::Execute, "shell", "npm testing", &rules, &[]), Verdict::Ask);
        assert!(matches!(verdict(Access::Full, Effect::Execute, "shell", "rm -rf x", &rules, &[]), Verdict::Deny(_)));
    }

    #[test]
    fn read_only_commands_are_recognised() {
        assert!(read_only("git status", &safe()));
        assert!(read_only("Get-ChildItem src | Select-String foo", &safe()));
        assert!(!read_only("Get-ChildItem > out.txt", &safe()));
        assert!(!read_only("git status; npm install", &safe()));
        assert!(!read_only("", &safe()));
    }
}