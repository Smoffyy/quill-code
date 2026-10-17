use quill_providers::Profile;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    LlamaCpp,
    LmStudio,
    Ollama,
    Vllm,
    Jan,
    Gpt4All,
    #[default]
    #[serde(other)]
    Custom,
}

impl ServerKind {
    pub const ALL: [ServerKind; 7] = [
        ServerKind::LlamaCpp,
        ServerKind::LmStudio,
        ServerKind::Ollama,
        ServerKind::Vllm,
        ServerKind::Jan,
        ServerKind::Gpt4All,
        ServerKind::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ServerKind::LlamaCpp => "llama.cpp",
            ServerKind::LmStudio => "LM Studio",
            ServerKind::Ollama => "Ollama",
            ServerKind::Vllm => "vLLM",
            ServerKind::Jan => "Jan",
            ServerKind::Gpt4All => "GPT4All",
            ServerKind::Custom => "Another server",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ServerKind::LlamaCpp => "llama-server",
            ServerKind::LmStudio => "turn on the server in the Developer tab",
            ServerKind::Ollama => "ollama serve",
            ServerKind::Vllm => "vllm serve, with --enable-auto-tool-choice for tools",
            ServerKind::Jan => "turn on the Local API Server in Settings",
            ServerKind::Gpt4All => "turn on the API server in Settings",
            ServerKind::Custom => "anything with an OpenAI-compatible API",
        }
    }

    pub fn port(self) -> Option<u16> {
        match self {
            ServerKind::LlamaCpp => Some(9931),
            ServerKind::LmStudio => Some(1234),
            ServerKind::Ollama => Some(11434),
            ServerKind::Vllm => Some(8000),
            ServerKind::Jan => Some(1337),
            ServerKind::Gpt4All => Some(4891),
            ServerKind::Custom => None,
        }
    }

    pub fn default_url(self) -> String {
        self.port().map(|port| format!("http://127.0.0.1:{port}")).unwrap_or_default()
    }

    pub fn asks_key(self) -> bool {
        !matches!(self, ServerKind::Ollama | ServerKind::Gpt4All)
    }

    fn slug(self) -> &'static str {
        match self {
            ServerKind::LlamaCpp => "llama-cpp",
            ServerKind::LmStudio => "lm-studio",
            ServerKind::Ollama => "ollama",
            ServerKind::Vllm => "vllm",
            ServerKind::Jan => "jan",
            ServerKind::Gpt4All => "gpt4all",
            ServerKind::Custom => "server",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub kind: ServerKind,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub enabled: bool,
}

impl Provider {
    pub fn new(id: &str, kind: ServerKind, url: &str) -> Provider {
        Provider { id: id.into(), name: kind.label().into(), kind, url: url.into(), key: None, enabled: true }
    }
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Override {
    pub model: ModelRef,
    #[serde(default)]
    pub context: Option<u64>,
    #[serde(default)]
    pub vision: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Models {
    pub providers: Vec<Provider>,
    pub selected: Option<ModelRef>,
    pub pinned: Vec<ModelRef>,
    pub thinking: bool,
    pub effort: String,
    pub roles: Roles,
    pub overrides: Vec<Override>,
    pub profiles: Vec<Profile>,
}

impl Default for Models {
    fn default() -> Self {
        Models {
            providers: Vec::new(),
            selected: None,
            pinned: Vec::new(),
            thinking: true,
            effort: "medium".into(),
            roles: Roles::default(),
            overrides: Vec::new(),
            profiles: Vec::new(),
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

    pub fn fresh_id(&self, kind: ServerKind) -> String {
        (1..)
            .map(|n| if n == 1 { kind.slug().to_string() } else { format!("{}-{n}", kind.slug()) })
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
        let new: ModelRef = serde_json::from_str(r#"{ "provider": "local-2", "model": "gemma" }"#).unwrap();
        assert_eq!(new, ModelRef::new("local-2", "gemma"));
        assert_eq!(serde_json::to_value(&new).unwrap()["provider"], "local-2");
    }

    #[test]
    fn new_servers_get_the_next_free_id() {
        let mut models = Models::default();
        assert!(models.providers.is_empty());
        assert_eq!(models.fresh_id(ServerKind::Ollama), "ollama");
        models.providers.push(Provider::new("ollama", ServerKind::Ollama, "http://127.0.0.1:11434"));
        assert_eq!(models.fresh_id(ServerKind::Ollama), "ollama-2");
    }

    #[test]
    fn servers_know_their_kind_and_keys_stay_out_of_the_file_when_empty() {
        let provider = Provider::new("jan", ServerKind::Jan, &ServerKind::Jan.default_url());
        assert_eq!(provider.url, "http://127.0.0.1:1337");
        let saved = serde_json::to_value(&provider).unwrap();
        assert_eq!(saved["kind"], "jan");
        assert!(saved.get("key").is_none());
        let unknown: Provider = serde_json::from_str(r#"{ "kind": "something_new", "url": "http://box" }"#).unwrap();
        assert_eq!(unknown.kind, ServerKind::Custom);
    }

    #[test]
    fn overrides_are_found_by_model() {
        let mut models = Models::default();
        let model = ModelRef::new("local", "m");
        assert!(models.override_for(&model).is_none());
        models.overrides.push(Override { model: model.clone(), context: Some(8192), vision: None });
        assert_eq!(models.override_for(&model).unwrap().context, Some(8192));
    }
}