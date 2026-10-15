use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::{App, Context, SharedString};
use quill_config::{LOCAL_PORTS, ModelRef, Price, Provider as ProviderConfig, ProviderKind, api_key, claude_latest, default_price};
use quill_core::{ClaudeCode, Engine, claude_complete, claude_version, find_claude};
use quill_providers::{
    Anthropic, ChatRequest, Effort, Event, Message, Model, OpenAi, Props, Provider, Status, Thinking, guess_thinking, normalize_url,
};

use crate::config::Config;

#[derive(Clone, PartialEq)]
pub enum Connection {
    Checking,
    Online,
    Offline(SharedString),
    NeedsKey,
    Off,
}

#[derive(Clone)]
pub struct Source {
    pub config: ProviderConfig,
    pub connection: Connection,
    pub models: Vec<Model>,
    pub detail: Option<String>,
    key: Option<String>,
    binary: Option<PathBuf>,
}

impl Source {
    pub fn ready(&self) -> bool {
        self.connection == Connection::Online
    }

    pub fn status(&self) -> SharedString {
        match &self.connection {
            Connection::Checking => "Connecting".into(),
            Connection::Online if self.config.kind == ProviderKind::ClaudeCode => {
                format!("Claude Code {} · your Claude plan", self.detail.clone().unwrap_or_default()).into()
            }
            Connection::Online => {
                let count = self.models.len();
                format!("{count} {}", if count == 1 { "model" } else { "models" }).into()
            }
            Connection::Offline(error) => error.clone(),
            Connection::NeedsKey => "Add an API key to use it".into(),
            Connection::Off => "Turned off".into(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum Control {
    None,
    Toggle,
    Effort(Vec<Effort>),
}

#[derive(Clone)]
pub enum Writer {
    Model { provider: Arc<dyn Provider>, model: String, effort: Option<Effort> },
    Claude { binary: PathBuf, model: String },
}

impl Writer {
    pub fn write(&self, system: &str, user: &str) -> Result<String, String> {
        match self {
            Writer::Model { provider, model, effort } => {
                let request = ChatRequest {
                    model: model.clone(),
                    messages: vec![Message::system(system), Message::user(user)],
                    thinking: Some(false),
                    effort: *effort,
                    ..ChatRequest::default()
                };
                let mut text = String::new();
                provider
                    .stream(&request, &AtomicBool::new(false), &mut |event| {
                        if let Event::Content(chunk) = event {
                            text.push_str(&chunk);
                        }
                    })
                    .map_err(|e| e.to_string())?;
                Ok(match text.rfind("</think>") {
                    Some(end) => text[end + 8..].trim().to_string(),
                    None => text.trim().to_string(),
                })
            }
            Writer::Claude { binary, model } => claude_complete(binary, model, &format!("{system}\n\n{user}")),
        }
    }
}

struct Found {
    connection: Connection,
    models: Vec<Model>,
    detail: Option<String>,
    key: Option<String>,
    binary: Option<PathBuf>,
}

impl Found {
    fn offline(error: impl Into<SharedString>) -> Found {
        Found { connection: Connection::Offline(error.into()), models: Vec::new(), detail: None, key: None, binary: None }
    }

    fn listed(result: Result<Vec<Model>, quill_providers::Error>, key: Option<String>) -> Found {
        match result {
            Ok(models) => Found { connection: Connection::Online, models, detail: None, key, binary: None },
            Err(error) => Found::offline(error.to_string()),
        }
    }
}

pub fn claude_label(id: &str) -> String {
    let mut words = vec!["Claude".to_string()];
    let mut version = Vec::new();
    for part in id.trim_start_matches("claude-").split('-') {
        if part.chars().all(|c| c.is_ascii_digit()) {
            if part.len() <= 2 {
                version.push(part);
            }
        } else {
            let mut chars = part.chars();
            words.extend(chars.next().map(|first| format!("{}{}", first.to_uppercase(), chars.as_str())));
        }
    }
    if !version.is_empty() {
        words.push(version.join("."));
    }
    words.join(" ")
}

fn names_path() -> std::path::PathBuf {
    quill_config::app_dir().join("claude-models.json")
}

fn load_names() -> HashMap<String, String> {
    std::fs::read_to_string(names_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn probe(config: &ProviderConfig, names: &HashMap<String, String>) -> Found {
    match config.kind {
        ProviderKind::Local => Found::listed(OpenAi::new(&config.url).models(), None),
        ProviderKind::OpenAi | ProviderKind::Anthropic => {
            let Some(key) = api_key(config) else {
                return Found { connection: Connection::NeedsKey, ..Found::offline("") };
            };
            let result = match config.kind {
                ProviderKind::OpenAi => OpenAi::cloud(&config.url, Some(key.clone())).models(),
                _ => Anthropic::new(&config.url, &key).models(),
            };
            Found::listed(result, Some(key))
        }
        ProviderKind::ClaudeCode => {
            let configured = Some(Path::new(config.url.trim())).filter(|path| !path.as_os_str().is_empty());
            let Some(binary) = find_claude(configured) else {
                return Found::offline("Claude Code is not installed. Install it, sign in once in a terminal, then refresh.");
            };
            let Some(version) = claude_version(&binary) else {
                return Found::offline(format!("{} did not answer.", binary.display()));
            };
            let models = config
                .models
                .iter()
                .map(|alias| Model {
                    name: Some(claude_label(names.get(alias).map(String::as_str).or(claude_latest(alias)).unwrap_or(alias))),
                    vision: true,
                    efforts: Effort::ALL.to_vec(),
                    ..Model::new(alias.clone())
                })
                .collect();
            Found { connection: Connection::Online, models, detail: Some(version), key: None, binary: Some(binary) }
        }
    }
}

pub struct Backend {
    pub sources: Vec<Source>,
    props: Option<(ModelRef, Props)>,
    windows: HashMap<ModelRef, u64>,
    names: HashMap<String, String>,
    generation: usize,
}

impl Backend {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut backend = Backend { sources: Vec::new(), props: None, windows: HashMap::new(), names: load_names(), generation: 0 };
        backend.refresh(cx);
        backend
    }

    pub fn selected_ref(cx: &App) -> Option<ModelRef> {
        Config::get(cx).models.selected.clone()
    }

    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|source| source.config.id == id)
    }

    pub fn find(&self, model: &ModelRef) -> Option<(&Source, &Model)> {
        let source = self.source(&model.provider)?;
        source.models.iter().find(|m| m.id == model.model).map(|m| (source, m))
    }

    pub fn selected(&self, cx: &App) -> Option<(&Source, &Model)> {
        self.find(&Self::selected_ref(cx)?)
    }

    pub fn effective(&self, model: &ModelRef, cx: &App) -> Option<Model> {
        let (_, found) = self.find(model)?;
        let mut found = found.clone();
        if let Some(window) = self.windows.get(model) {
            found.context = Some(*window);
        }
        if let Some(props) = self.props.as_ref().filter(|(r, _)| r == model).map(|(_, props)| props) {
            found.context = props.context.or(found.context);
            found.vision |= props.vision;
        }
        if let Some(over) = Config::get(cx).models.override_for(model) {
            found.context = over.context.or(found.context);
            found.max_output = over.max_output.or(found.max_output);
            found.vision = over.vision.unwrap_or(found.vision);
        }
        Some(found)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let providers = Config::get(cx).models.providers.clone();
        self.generation += 1;
        let generation = self.generation;
        let previous = std::mem::take(&mut self.sources);
        self.sources = providers
            .into_iter()
            .map(|config| {
                let old = previous.iter().find(|s| s.config.id == config.id && s.config.kind == config.kind && s.config.url == config.url);
                Source {
                    connection: if config.enabled { Connection::Checking } else { Connection::Off },
                    models: old.filter(|_| config.enabled).map(|s| s.models.clone()).unwrap_or_default(),
                    detail: old.and_then(|s| s.detail.clone()),
                    key: old.and_then(|s| s.key.clone()),
                    binary: old.and_then(|s| s.binary.clone()),
                    config,
                }
            })
            .collect();
        cx.notify();
        for source in self.sources.iter().filter(|s| s.config.enabled) {
            let config = source.config.clone();
            let id = config.id.clone();
            let names = self.names.clone();
            let task = cx.background_executor().spawn(async move { probe(&config, &names) });
            cx.spawn(async move |this, cx| {
                let found = task.await;
                _ = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    let Some(source) = this.sources.iter_mut().find(|s| s.config.id == id) else { return };
                    source.connection = found.connection;
                    source.models = found.models;
                    source.detail = found.detail;
                    source.key = found.key;
                    source.binary = found.binary;
                    let kind = source.config.kind;
                    if let Some(selected) = Self::selected_ref(cx).filter(|r| r.provider == id && kind == ProviderKind::Local) {
                        let loaded = this.find(&selected).is_some_and(|(_, m)| m.status == Status::Loaded);
                        if loaded {
                            this.fetch_props(selected, cx);
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub fn select(&mut self, model: ModelRef, cx: &mut Context<Self>) {
        Config::update(cx, |config| config.models.selected = Some(model.clone()));
        self.props = None;
        if self.source(&model.provider).is_some_and(|s| s.config.kind == ProviderKind::Local) {
            self.fetch_props(model, cx);
        }
        cx.notify();
    }

    pub fn set_thinking(&mut self, on: bool, cx: &mut Context<Self>) {
        Config::update(cx, |config| config.models.thinking = on);
        cx.notify();
    }

    pub fn set_effort(&mut self, effort: Effort, cx: &mut Context<Self>) {
        Config::update(cx, |config| config.models.effort = effort);
        cx.notify();
    }

    pub fn control(&self, cx: &App) -> Control {
        let Some(selected) = Self::selected_ref(cx) else { return Control::None };
        let Some((source, model)) = self.find(&selected) else { return Control::None };
        if !model.efforts.is_empty() {
            return Control::Effort(model.efforts.clone());
        }
        if source.config.kind != ProviderKind::Local {
            return Control::None;
        }
        let thinking = match &self.props {
            Some((r, props)) if *r == selected => props.thinking,
            _ => guess_thinking(&model.id),
        };
        if thinking == Thinking::Toggle { Control::Toggle } else { Control::None }
    }

    pub fn request_thinking(&self, cx: &App) -> Option<bool> {
        (self.control(cx) == Control::Toggle).then(|| Config::get(cx).models.thinking)
    }

    pub fn request_effort(&self, cx: &App) -> Option<Effort> {
        match self.control(cx) {
            Control::Effort(levels) => Config::get(cx).models.effort.nearest(&levels),
            _ => None,
        }
    }

    pub fn engine(&self, model: &ModelRef) -> Result<Engine, String> {
        let source = self.source(&model.provider).ok_or("That model's provider was removed. Choose another model.")?;
        let config = &source.config;
        let key = || source.key.clone().ok_or_else(|| format!("{} needs an API key. Add one in Settings, Models.", config.name));
        Ok(match config.kind {
            ProviderKind::Local => Engine::Model(Arc::new(OpenAi::new(&config.url))),
            ProviderKind::OpenAi => Engine::Model(Arc::new(OpenAi::cloud(&config.url, Some(key()?)))),
            ProviderKind::Anthropic => Engine::Model(Arc::new(Anthropic::new(&config.url, &key()?).with_fallback(config.fallback))),
            ProviderKind::ClaudeCode => {
                let binary = source.binary.clone().ok_or("Claude Code was not found. Install it, then refresh the model list.")?;
                Engine::ClaudeCode(ClaudeCode { binary, resume: None })
            }
        })
    }

    pub fn writer(&self, model: &ModelRef, cx: &App) -> Result<Writer, String> {
        let effort = self.effective(model, cx).and_then(|m| Effort::Low.nearest(&m.efforts));
        Ok(match self.engine(model)? {
            Engine::Model(provider) => Writer::Model { provider, model: model.model.clone(), effort },
            Engine::ClaudeCode(claude) => Writer::Claude { binary: claude.binary, model: model.model.clone() },
        })
    }

    pub fn price(&self, model: &ModelRef, cx: &App) -> Option<Price> {
        if let Some(price) = Config::get(cx).models.override_for(model).and_then(|o| o.price) {
            return Some(price);
        }
        let kind = self.source(&model.provider)?.config.kind;
        if kind == ProviderKind::Anthropic { default_price(&model.model) } else { None }
    }

    pub fn learn_model(&mut self, model: &ModelRef, resolved: &str, cx: &mut Context<Self>) {
        let claude = self.source(&model.provider).is_some_and(|s| s.config.kind == ProviderKind::ClaudeCode);
        if !claude || self.names.get(&model.model).map(String::as_str) == Some(resolved) {
            return;
        }
        self.names.insert(model.model.clone(), resolved.to_string());
        if let Ok(text) = serde_json::to_string_pretty(&self.names) {
            let _ = std::fs::write(names_path(), text);
        }
        let found =
            self.sources.iter_mut().find(|s| s.config.id == model.provider).and_then(|s| s.models.iter_mut().find(|m| m.id == model.model));
        if let Some(found) = found {
            found.name = Some(claude_label(resolved));
        }
        cx.notify();
    }

    pub fn learn_window(&mut self, model: &ModelRef, tokens: u64, cx: &mut Context<Self>) {
        if self.windows.insert(model.clone(), tokens) != Some(tokens) {
            cx.notify();
        }
    }

    pub fn mark_used(&mut self, model: &ModelRef, cx: &mut Context<Self>) {
        let local = self.source(&model.provider).is_some_and(|s| s.config.kind == ProviderKind::Local);
        if !local {
            return;
        }
        if let Some(found) =
            self.sources.iter_mut().find(|s| s.config.id == model.provider).and_then(|s| s.models.iter_mut().find(|m| m.id == model.model))
        {
            found.status = Status::Loaded;
        }
        if !matches!(&self.props, Some((r, _)) if r == model) {
            self.fetch_props(model.clone(), cx);
        }
        cx.notify();
    }

    fn fetch_props(&mut self, model: ModelRef, cx: &mut Context<Self>) {
        let Some(url) = self.source(&model.provider).map(|s| s.config.url.clone()) else { return };
        let wanted = model.model.clone();
        let task = cx.background_executor().spawn(async move { OpenAi::new(&url).props(&wanted) });
        cx.spawn(async move |this, cx| {
            if let Ok(props) = task.await {
                _ = this.update(cx, |this, cx| {
                    let found = this
                        .sources
                        .iter_mut()
                        .find(|s| s.config.id == model.provider)
                        .and_then(|s| s.models.iter_mut().find(|m| m.id == model.model));
                    if let Some(found) = found {
                        found.status = Status::Loaded;
                    }
                    this.props = Some((model, props));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn scan(&mut self, cx: &mut Context<Self>) -> gpui_kit::Task<usize> {
        let known: Vec<String> = Config::get(cx).models.providers.iter().map(|p| normalize_url(&p.url)).collect();
        let task = cx.background_executor().spawn(async move {
            let urls: Vec<String> =
                LOCAL_PORTS.iter().map(|port| format!("http://127.0.0.1:{port}")).filter(|url| !known.contains(url)).collect();
            std::thread::scope(|scope| {
                let checks: Vec<_> = urls.iter().map(|url| scope.spawn(move || OpenAi::new(url).models().is_ok())).collect();
                urls.iter().zip(checks).filter_map(|(url, check)| check.join().unwrap_or(false).then(|| url.clone())).collect::<Vec<_>>()
            })
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let count = found.len();
            _ = this.update(cx, |this, cx| {
                if count == 0 {
                    return;
                }
                Config::update(cx, |config| {
                    for url in found {
                        let id = config.models.fresh_id(ProviderKind::Local);
                        let port = url.rsplit(':').next().unwrap_or_default().to_string();
                        let mut provider = ProviderConfig::new(&id, &format!("Local server on {port}"), ProviderKind::Local);
                        provider.url = url;
                        config.models.providers.push(provider);
                    }
                });
                this.refresh(cx);
            });
            count
        })
    }
}
pub fn provider_icon(kind: ProviderKind) -> crate::icons::Icon {
    use crate::icons::Icon;
    match kind {
        ProviderKind::Local => Icon::Server,
        ProviderKind::OpenAi => Icon::OpenAi,
        ProviderKind::Anthropic | ProviderKind::ClaudeCode => Icon::Claude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_models_read_with_their_version() {
        assert_eq!(claude_label("claude-haiku-5-5"), "Claude Haiku 5.5");
        assert_eq!(claude_label("claude-haiku-4-5-20251001"), "Claude Haiku 4.5");
        assert_eq!(claude_label("claude-fable-5-1"), "Claude Fable 5.1");
        assert_eq!(claude_label("opus"), "Claude Opus");
    }
}