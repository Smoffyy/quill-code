use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;

use quill_config::{ModelRef, Provider as ProviderConfig, ServerKind, Settings};
use quill_providers::{
    ChatRequest, Control, Event, LmStudio, Message, Model, Ollama, OpenAi, Props, Provider, Status, Tuning, normalize_url, profile_for,
};

use crate::Msg;

#[derive(Clone, Debug, PartialEq)]
pub enum Connection {
    Checking,
    Online,
    Offline(String),
    Off,
}

#[derive(Clone)]
pub struct Source {
    pub config: ProviderConfig,
    pub connection: Connection,
    pub models: Vec<Model>,
}

impl Source {
    pub fn status(&self) -> String {
        match &self.connection {
            Connection::Checking => "Connecting".into(),
            Connection::Online => {
                let count = self.models.len();
                format!("{count} {}", if count == 1 { "model" } else { "models" })
            }
            Connection::Offline(error) => error.clone(),
            Connection::Off => "Turned off".into(),
        }
    }
}

#[derive(Clone)]
pub struct Writer {
    provider: Arc<dyn Provider>,
    model: String,
    context: Option<u64>,
    tuning: Tuning,
}

impl Writer {
    pub fn write(&self, system: &str, user: &str) -> Result<String, String> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![Message::system(system), Message::user(user)],
            reasoning: self.tuning.quiet.clone(),
            kwargs: self.tuning.kwargs.clone(),
            context: self.context,
            ..ChatRequest::default()
        };
        let mut text = String::new();
        self.provider
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
}

pub struct Found {
    connection: Connection,
    models: Vec<Model>,
}

pub fn connect(config: &ProviderConfig) -> Arc<dyn Provider> {
    let key = config.key.as_deref();
    match config.kind {
        ServerKind::Ollama => Arc::new(Ollama::new(&config.url, key)),
        ServerKind::LmStudio => Arc::new(LmStudio::new(&config.url, key)),
        _ => Arc::new(OpenAi::new(&config.url).with_key(key)),
    }
}

pub fn list_models(config: &ProviderConfig) -> Result<Vec<Model>, String> {
    connect(config).models().map_err(|error| error.to_string())
}

fn probe(config: &ProviderConfig) -> Found {
    match list_models(config) {
        Ok(models) => Found { connection: Connection::Online, models },
        Err(error) => Found { connection: Connection::Offline(error), models: Vec::new() },
    }
}

pub fn scan(settings: &Settings) -> Vec<(ServerKind, String)> {
    let known: Vec<String> = settings.models.providers.iter().map(|p| normalize_url(&p.url)).collect();
    let candidates: Vec<(ServerKind, String)> =
        ServerKind::ALL.iter().map(|kind| (*kind, kind.default_url())).filter(|(_, url)| !url.is_empty() && !known.contains(url)).collect();
    std::thread::scope(|scope| {
        let checks: Vec<_> = candidates.iter().map(|(_, url)| scope.spawn(move || OpenAi::new(url).reachable())).collect();
        candidates.iter().zip(checks).filter_map(|(found, check)| check.join().unwrap_or(false).then(|| found.clone())).collect()
    })
}

pub fn add_servers(settings: &mut Settings, found: &[(ServerKind, String)]) {
    for (kind, url) in found {
        let id = settings.models.fresh_id(*kind);
        settings.models.providers.push(ProviderConfig::new(&id, *kind, url));
    }
}

pub struct Backend {
    pub sources: Vec<Source>,
    props: Option<(ModelRef, Props)>,
    generation: usize,
    tx: Sender<Msg>,
}

impl Backend {
    pub fn new(tx: Sender<Msg>) -> Backend {
        Backend { sources: Vec::new(), props: None, generation: 0, tx }
    }

    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|source| source.config.id == id)
    }

    pub fn find(&self, model: &ModelRef) -> Option<(&Source, &Model)> {
        let source = self.source(&model.provider)?;
        source.models.iter().find(|m| m.id == model.model).map(|m| (source, m))
    }

    pub fn selected(&self, settings: &Settings) -> Option<(&Source, &Model)> {
        self.find(settings.models.selected.as_ref()?)
    }

    pub fn effective(&self, model: &ModelRef, settings: &Settings) -> Option<Model> {
        let (_, found) = self.find(model)?;
        let mut found = found.clone();
        if let Some(props) = self.props.as_ref().filter(|(r, _)| r == model).map(|(_, props)| props) {
            found.context = props.context.or(found.context);
            found.vision |= props.vision;
        }
        if let Some(over) = settings.models.override_for(model) {
            found.context = over.context.or(found.context);
            found.vision = over.vision.unwrap_or(found.vision);
        }
        Some(found)
    }

    pub fn checking(&self, provider: &str) -> bool {
        self.source(provider).is_some_and(|source| source.connection == Connection::Checking)
    }

    pub fn refresh(&mut self, settings: &Settings) {
        self.generation += 1;
        let generation = self.generation;
        let previous = std::mem::take(&mut self.sources);
        self.sources = settings
            .models
            .providers
            .iter()
            .cloned()
            .map(|config| {
                let old = previous.iter().find(|s| s.config.id == config.id && s.config.url == config.url);
                Source {
                    connection: if config.enabled { Connection::Checking } else { Connection::Off },
                    models: old.filter(|_| config.enabled).map(|s| s.models.clone()).unwrap_or_default(),
                    config,
                }
            })
            .collect();
        for source in self.sources.iter().filter(|s| s.config.enabled) {
            let config = source.config.clone();
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let found = probe(&config);
                let _ = tx.send(Msg::Probed { generation, id: config.id, found });
            });
        }
    }

    pub fn probed(&mut self, generation: usize, id: &str, found: Found, settings: &Settings) {
        if generation != self.generation {
            return;
        }
        let Some(source) = self.sources.iter_mut().find(|s| s.config.id == id) else { return };
        source.connection = found.connection;
        source.models = found.models;
        if let Some(selected) = settings.models.selected.clone().filter(|r| r.provider == id)
            && self.find(&selected).is_some_and(|(_, m)| m.status == Status::Loaded)
        {
            self.fetch_props(selected);
        }
    }

    pub fn select(&mut self, model: ModelRef) {
        self.props = None;
        self.fetch_props(model);
    }

    pub fn control(&self, model: &ModelRef, settings: &Settings) -> Control {
        if let Some(profile) = profile_for(&model.model, &settings.models.profiles) {
            return profile.control;
        }
        match &self.props {
            Some((r, props)) if r == model => props.control.clone(),
            _ => self.find(model).and_then(|(_, found)| found.control.clone()).unwrap_or(Control::None),
        }
    }

    pub fn selected_control(&self, settings: &Settings) -> Control {
        settings.models.selected.as_ref().map_or(Control::None, |model| self.control(model, settings))
    }

    pub fn tuning(&self, model: &ModelRef, settings: &Settings) -> Tuning {
        let kwargs = profile_for(&model.model, &settings.models.profiles).map(|profile| profile.kwargs).unwrap_or_default();
        Tuning::new(&self.control(model, settings), settings.models.thinking, &settings.models.effort, kwargs)
    }

    pub fn provider(&self, model: &ModelRef) -> Result<Arc<dyn Provider>, String> {
        let source = self.source(&model.provider).ok_or("That model's server was removed. Pick another model with /model.")?;
        Ok(connect(&source.config))
    }

    pub fn writer(&self, model: &ModelRef, settings: &Settings) -> Result<Writer, String> {
        let context = self.effective(model, settings).and_then(|found| found.context);
        Ok(Writer { provider: self.provider(model)?, model: model.model.clone(), context, tuning: self.tuning(model, settings) })
    }

    pub fn mark_used(&mut self, model: &ModelRef) {
        if let Some(found) = self.model_mut(model) {
            found.status = Status::Loaded;
        }
        if !matches!(&self.props, Some((r, _)) if r == model) {
            self.fetch_props(model.clone());
        }
    }

    fn model_mut(&mut self, model: &ModelRef) -> Option<&mut Model> {
        self.sources.iter_mut().find(|s| s.config.id == model.provider).and_then(|s| s.models.iter_mut().find(|m| m.id == model.model))
    }

    fn fetch_props(&mut self, model: ModelRef) {
        let Some(config) = self.source(&model.provider).map(|s| s.config.clone()).filter(|c| c.kind == ServerKind::LlamaCpp) else {
            return;
        };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            if let Ok(props) = OpenAi::new(&config.url).with_key(config.key.as_deref()).props(&model.model) {
                let _ = tx.send(Msg::Props { model, props });
            }
        });
    }

    pub fn got_props(&mut self, model: ModelRef, props: Props) {
        if let Some(found) = self.model_mut(&model) {
            found.status = Status::Loaded;
        }
        self.props = Some((model, props));
    }
}