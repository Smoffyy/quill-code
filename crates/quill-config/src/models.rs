use quill_providers::{ANTHROPIC_URL, Effort, OPENAI_URL, Tokens, normalize_url};
use serde::{Deserialize, Serialize};

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:9931";
pub const LOCAL_PORTS: [u16; 6] = [9931, 8080, 1234, 11434, 8000, 5000];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    Local,
    OpenAi,
    Anthropic,
    ClaudeCode,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 4] = [ProviderKind::Local, ProviderKind::OpenAi, ProviderKind::Anthropic, ProviderKind::ClaudeCode];

    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Local => "Local server",
            ProviderKind::OpenAi => "OpenAI-compatible",
            ProviderKind::Anthropic => "Anthropic",
            ProviderKind::ClaudeCode => "Claude Code",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ProviderKind::Local => "llama.cpp, LM Studio, Ollama, vLLM or anything with an OpenAI-style API on your network",
            ProviderKind::OpenAi => "OpenAI or a hosted OpenAI-style service such as OpenRouter, with an API key",
            ProviderKind::Anthropic => "Claude models through the Anthropic API, with an API key",
            ProviderKind::ClaudeCode => "Claude models through your installed Claude Code and its sign-in, no key needed",
        }
    }

    pub fn needs_key(self) -> bool {
        matches!(self, ProviderKind::OpenAi | ProviderKind::Anthropic)
    }

    pub fn default_url(self) -> &'static str {
        match self {
            ProviderKind::Local => DEFAULT_SERVER,
            ProviderKind::OpenAi => OPENAI_URL,
            ProviderKind::Anthropic => ANTHROPIC_URL,
            ProviderKind::ClaudeCode => "",
        }
    }

    pub fn env_key(self) -> Option<&'static str> {
        match self {
            ProviderKind::OpenAi => Some("OPENAI_API_KEY"),
            ProviderKind::Anthropic => Some("ANTHROPIC_API_KEY"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub url: String,
    pub enabled: bool,
    pub models: Vec<String>,
    pub fallback: bool,
}

impl Default for Provider {
    fn default() -> Self {
        Provider::new("local", "Local server", ProviderKind::Local)
    }
}

impl Provider {
    pub fn new(id: &str, name: &str, kind: ProviderKind) -> Provider {
        let models =
            if kind == ProviderKind::ClaudeCode { ["fable", "opus", "sonnet", "haiku"].map(String::from).to_vec() } else { Vec::new() };
        Provider { id: id.into(), name: name.into(), kind, url: kind.default_url().into(), enabled: true, models, fallback: true }
    }

    pub fn official(&self) -> bool {
        self.kind.needs_key() && normalize_url(&self.url) == normalize_url(self.kind.default_url())
    }
}

pub fn default_providers() -> Vec<Provider> {
    vec![
        Provider::new("local", "Local server", ProviderKind::Local),
        Provider::new("claude-code", "Claude Code", ProviderKind::ClaudeCode),
        Provider::new("anthropic", "Anthropic", ProviderKind::Anthropic),
        Provider::new("openai", "OpenAI", ProviderKind::OpenAi),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "RawRef")]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawRef {
    Legacy(String),
    Full { provider: String, model: String },
}

impl From<RawRef> for ModelRef {
    fn from(raw: RawRef) -> ModelRef {
        match raw {
            RawRef::Legacy(model) => ModelRef::new("local", model),
            RawRef::Full { provider, model } => ModelRef { provider, model },
        }
    }
}

impl ModelRef {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> ModelRef {
        ModelRef { provider: provider.into(), model: model.into() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Roles {
    pub compaction: Option<ModelRef>,
    pub commit: Option<ModelRef>,
    pub subagent: Option<ModelRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Price {
    pub fn new(input: f64, output: f64) -> Price {
        Price { input, output, cache_read: input * 0.1, cache_write: input * 1.25 }
    }

    pub fn cost(&self, tokens: Tokens) -> f64 {
        let fresh = tokens.prompt.saturating_sub(tokens.cache_read + tokens.cache_write) as f64;
        let total = fresh * self.input
            + tokens.cache_read as f64 * self.cache_read
            + tokens.cache_write as f64 * self.cache_write
            + tokens.completion as f64 * self.output;
        total / 1_000_000.0
    }
}

const ANTHROPIC_PRICES: [(&str, f64, f64, Option<f64>); 11] = [
    ("claude-fable-5-1", 10.0, 50.0, Some(0.25)),
    ("claude-mythos-5-1", 10.0, 50.0, Some(0.25)),
    ("claude-fable-5", 10.0, 50.0, Some(1.0)),
    ("claude-mythos-5", 10.0, 50.0, Some(1.0)),
    ("claude-opus-5-5", 4.0, 20.0, Some(0.2)),
    ("claude-opus-5", 5.0, 25.0, None),
    ("claude-opus-4", 5.0, 25.0, None),
    ("claude-sonnet-5", 2.0, 10.0, Some(0.2)),
    ("claude-sonnet-4", 3.0, 15.0, None),
    ("claude-haiku-5", 0.1, 0.5, None),
    ("claude-haiku-4", 1.0, 5.0, None),
];

pub fn default_price(model: &str) -> Option<Price> {
    let (_, input, output, read) = ANTHROPIC_PRICES.iter().find(|(family, ..)| model.starts_with(family))?;
    let price = Price::new(*input, *output);
    Some(Price { cache_read: read.unwrap_or(price.cache_read), ..price })
}

const CLAUDE_LATEST: [(&str, &str); 4] =
    [("fable", "claude-fable-5-1"), ("opus", "claude-opus-5-5"), ("sonnet", "claude-sonnet-5-5"), ("haiku", "claude-haiku-5-5")];

pub fn claude_latest(alias: &str) -> Option<&'static str> {
    CLAUDE_LATEST.iter().find(|(name, _)| *name == alias).map(|(_, id)| *id)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Override {
    pub model: ModelRef,
    #[serde(default)]
    pub context: Option<u64>,
    #[serde(default)]
    pub max_output: Option<u64>,
    #[serde(default)]
    pub vision: Option<bool>,
    #[serde(default)]
    pub price: Option<Price>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Models {
    pub providers: Vec<Provider>,
    pub selected: Option<ModelRef>,
    pub pinned: Vec<ModelRef>,
    pub thinking: bool,
    pub effort: Effort,
    pub roles: Roles,
    pub overrides: Vec<Override>,
}

impl Default for Models {
    fn default() -> Self {
        Models {
            providers: default_providers(),
            selected: None,
            pinned: Vec::new(),
            thinking: true,
            effort: Effort::High,
            roles: Roles::default(),
            overrides: Vec::new(),
        }
    }
}

impl Models {
    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|provider| provider.id == id)
    }

    pub fn override_for(&self, model: &ModelRef) -> Option<&Override> {
        self.overrides.iter().find(|entry| entry.model == *model)
    }

    pub fn override_mut(&mut self, model: &ModelRef) -> &mut Override {
        let index = match self.overrides.iter().position(|entry| entry.model == *model) {
            Some(index) => index,
            None => {
                self.overrides.push(Override { model: model.clone(), context: None, max_output: None, vision: None, price: None });
                self.overrides.len() - 1
            }
        };
        &mut self.overrides[index]
    }

    pub fn fresh_id(&self, kind: ProviderKind) -> String {
        let base = match kind {
            ProviderKind::Local => "local",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::ClaudeCode => "claude-code",
        };
        (1..)
            .map(|n| if n == 1 { base.to_string() } else { format!("{base}-{n}") })
            .find(|id| self.provider(id).is_none())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_refs_read_old_plain_names() {
        let old: ModelRef = serde_json::from_str("\"qwen3.5-4b\"").unwrap();
        assert_eq!(old, ModelRef::new("local", "qwen3.5-4b"));
        let new: ModelRef = serde_json::from_str(r#"{ "provider": "openai", "model": "gpt-5" }"#).unwrap();
        assert_eq!(new, ModelRef::new("openai", "gpt-5"));
        assert_eq!(serde_json::to_value(&new).unwrap()["provider"], "openai");
    }

    #[test]
    fn providers_know_their_defaults() {
        let models = Models::default();
        assert_eq!(models.providers.len(), 4);
        assert_eq!(models.provider("claude-code").unwrap().models.len(), 4);
        assert!(models.provider("anthropic").unwrap().official());
        assert!(!models.provider("local").unwrap().official());
        assert_eq!(models.fresh_id(ProviderKind::Local), "local-2");
        assert_eq!(models.fresh_id(ProviderKind::ClaudeCode), "claude-code-2");
    }

    #[test]
    fn prices_follow_the_model_family_and_the_cache() {
        let opus = default_price("claude-opus-5-5").unwrap();
        assert_eq!((opus.input, opus.output, opus.cache_read), (4.0, 20.0, 0.2));
        assert_eq!(default_price("claude-opus-5").unwrap().input, 5.0);
        assert!(default_price("gpt-5").is_none());
        let tokens = Tokens { prompt: 1_000_000, completion: 100_000, cache_read: 800_000, cache_write: 0 };
        assert!((opus.cost(tokens) - (0.2 * 4.0 + 0.8 * 0.2 + 0.1 * 20.0)).abs() < 1e-9);
        assert_eq!(claude_latest("haiku"), Some("claude-haiku-5-5"));
    }

    #[test]
    fn overrides_are_created_on_demand() {
        let mut models = Models::default();
        let model = ModelRef::new("local", "m");
        assert!(models.override_for(&model).is_none());
        models.override_mut(&model).context = Some(8192);
        models.override_mut(&model).vision = Some(true);
        assert_eq!(models.overrides.len(), 1);
        assert_eq!(models.override_for(&model).unwrap().context, Some(8192));
    }
}