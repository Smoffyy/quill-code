use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reasoning", rename_all = "snake_case")]
pub enum Control {
    None,
    Toggle {
        #[serde(default = "thinking_kwarg")]
        kwarg: String,
    },
    Effort {
        #[serde(default = "effort_levels")]
        levels: Vec<String>,
    },
}

fn thinking_kwarg() -> String {
    "enable_thinking".into()
}

fn effort_levels() -> Vec<String> {
    ["low", "medium", "high"].map(String::from).to_vec()
}

impl Control {
    pub fn toggle() -> Control {
        Control::Toggle { kwarg: thinking_kwarg() }
    }

    pub fn effort() -> Control {
        Control::Effort { levels: effort_levels() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(rename = "match")]
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<String>,
    #[serde(flatten)]
    pub control: Control,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub kwargs: Map<String, Value>,
}

impl Profile {
    fn new(pattern: &str, except: &[&str], control: Control) -> Profile {
        Profile { pattern: pattern.into(), except: except.iter().map(|s| s.to_string()).collect(), control, kwargs: Map::new() }
    }

    pub fn fits(&self, model: &str) -> bool {
        let model = model.to_lowercase();
        model.contains(&self.pattern.to_lowercase()) && !self.except.iter().any(|word| model.contains(&word.to_lowercase()))
    }
}

pub fn builtin() -> Vec<Profile> {
    let kwarg = |name: &str| Control::Toggle { kwarg: name.into() };
    vec![
        Profile::new("gpt-oss", &[], Control::effort()),
        Profile::new("qwen3", &["instruct", "coder"], Control::toggle()),
        Profile::new("glm-4.5", &[], Control::toggle()),
        Profile::new("glm-4.6", &[], Control::toggle()),
        Profile::new("glm-4.7", &[], Control::toggle()),
        Profile::new("hunyuan", &[], Control::toggle()),
        Profile::new("deepseek-v3.1", &[], kwarg("thinking")),
        Profile::new("deepseek-v3.2", &[], kwarg("thinking")),
    ]
}

pub fn profile_for(model: &str, extra: &[Profile]) -> Option<Profile> {
    extra.iter().find(|profile| profile.fits(model)).cloned().or_else(|| builtin().into_iter().find(|profile| profile.fits(model)))
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reasoning {
    Toggle { kwarg: String, on: bool },
    Effort(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tuning {
    pub reasoning: Option<Reasoning>,
    pub quiet: Option<Reasoning>,
    pub kwargs: Map<String, Value>,
}

impl Tuning {
    pub fn new(control: &Control, thinking: bool, effort: &str, kwargs: Map<String, Value>) -> Tuning {
        let (reasoning, quiet) = match control {
            Control::None => (None, None),
            Control::Toggle { kwarg } => (
                Some(Reasoning::Toggle { kwarg: kwarg.clone(), on: thinking }),
                Some(Reasoning::Toggle { kwarg: kwarg.clone(), on: false }),
            ),
            Control::Effort { levels } => {
                let chosen = levels.iter().find(|level| level.eq_ignore_ascii_case(effort)).or(levels.get(levels.len() / 2));
                (chosen.map(|level| Reasoning::Effort(level.clone())), levels.first().map(|level| Reasoning::Effort(level.clone())))
            }
        };
        Tuning { reasoning, quiet, kwargs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn families_are_matched_by_name() {
        assert_eq!(profile_for("gpt-oss-20b", &[]).unwrap().control, Control::effort());
        assert_eq!(profile_for("Qwen3.5-4B", &[]).unwrap().control, Control::toggle());
        assert!(profile_for("Qwen3-30B-A3B-Instruct-2507", &[]).is_none());
        assert!(profile_for("llama-3.1-8b", &[]).is_none());
        assert_eq!(profile_for("deepseek-v3.1-terminus", &[]).unwrap().control, Control::Toggle { kwarg: "thinking".into() });
    }

    #[test]
    fn profiles_from_settings_come_first_and_read_plainly() {
        let mine: Vec<Profile> = serde_json::from_value(json!([
            { "match": "qwen3.5-4b", "reasoning": "none", "kwargs": { "top_k": 20 } },
            { "match": "my-model", "reasoning": "effort", "levels": ["minimal", "high"] }
        ]))
        .unwrap();
        let qwen = profile_for("qwen3.5-4b", &mine).unwrap();
        assert_eq!((qwen.control, qwen.kwargs["top_k"].clone()), (Control::None, json!(20)));
        assert_eq!(profile_for("my-model", &mine).unwrap().control, Control::Effort { levels: vec!["minimal".into(), "high".into()] });
        let toggle: Profile = serde_json::from_value(json!({ "match": "x", "reasoning": "toggle" })).unwrap();
        assert_eq!(toggle.control, Control::toggle());
    }

    #[test]
    fn tuning_picks_the_chosen_level_and_a_quiet_one() {
        let tuning = Tuning::new(&Control::effort(), true, "high", Map::new());
        assert_eq!((tuning.reasoning, tuning.quiet), (Some(Reasoning::Effort("high".into())), Some(Reasoning::Effort("low".into()))));
        assert_eq!(Tuning::new(&Control::effort(), true, "turbo", Map::new()).reasoning, Some(Reasoning::Effort("medium".into())));
        let toggle = Tuning::new(&Control::toggle(), true, "", Map::new());
        assert_eq!(toggle.quiet, Some(Reasoning::Toggle { kwarg: "enable_thinking".into(), on: false }));
        assert_eq!(Tuning::new(&Control::None, true, "high", Map::new()), Tuning::default());
    }
}