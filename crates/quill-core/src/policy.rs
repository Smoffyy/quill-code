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

fn redirects_into_a_file(text: &str) -> bool {
    text.match_indices('>').any(|(at, _)| {
        let rest = text[at + 1..].trim_start_matches('>').trim_start();
        let target = rest.split_whitespace().next().unwrap_or_default();
        let to_stream = text[..at].ends_with(|c: char| c.is_ascii_digit()) && rest.starts_with('&');
        !to_stream && !target.is_empty() && !target.starts_with('&') && !["/dev/null", "$null", "nul", "null"].contains(&target)
    })
}

pub fn edits_files(command: &str) -> bool {
    let text = command.to_lowercase();
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '{' | '}' | '"' | '\''))
        .filter(|word| !word.is_empty())
        .collect();
    let has = |word: &str| words.contains(&word);
    let interpreter = ["python", "python3", "py", "node", "ruby", "perl", "php"].iter().any(|word| has(word));
    let inline = has("-c") || has("-e") || text.contains("<<");
    let writes = [".write(", "write_text", "write_bytes", "writefile", "appendfile"].iter().any(|marker| text.contains(marker));
    let in_place = (has("sed") || has("perl")) && (text.contains(" -i") || text.contains(" -pi") || text.contains("--in-place"));
    let cmdlets = ["set-content", "add-content", "out-file", "writealltext", "writealllines", "writeallbytes", "appendalltext"]
        .iter()
        .any(|cmdlet| text.contains(cmdlet))
        || (has("new-item") && text.contains("-value"));
    let printed = ["echo", "printf", "cat", "type", "write-output", "write-host"].iter().any(|word| has(word));
    (interpreter && inline && writes) || in_place || cmdlets || has("tee") || has("tee-object") || (printed && redirects_into_a_file(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_that_edit_files_are_spotted() {
        let edits = [
            "python -c \"open('a.py','w').write('x')\"",
            "python3 - <<'EOF'\nfrom pathlib import Path\nPath('a.txt').write_text('x')\nEOF",
            "node -e \"require('fs').writeFileSync('a.js','x')\"",
            "sed -i 's/a/b/' main.rs",
            "perl -pi -e 's/a/b/' main.rs",
            "Set-Content -Path a.txt -Value hi",
            "'hi' | Out-File notes.md",
            "[IO.File]::WriteAllText('a.txt', 'x')",
            "New-Item a.txt -Value hi",
            "echo hello > notes.txt",
            "cat <<EOF > config.toml\nx = 1\nEOF",
            "printf 'x' >> log.txt",
            "echo x | tee a.txt",
        ];
        for command in edits {
            assert!(edits_files(command), "{command}");
        }
        let runs = [
            "python main.py",
            "python -m pytest -q",
            "cargo test 2>&1",
            "npm run build > /dev/null",
            "echo done",
            "Get-Content a.txt | Select-String fn",
            "git commit -m \"open the write path\"",
            "node script.js",
            "sed -n '1,20p' main.rs",
        ];
        for command in runs {
            assert!(!edits_files(command), "{command}");
        }
    }

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