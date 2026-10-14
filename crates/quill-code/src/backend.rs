use gpui_kit::{App, Context, SharedString};
use quill_llama::{Model, Props, Server, Thinking, guess_thinking, normalize_url};

use crate::config::Config;

#[derive(Clone, PartialEq)]
pub enum Connection {
    Checking,
    Online,
    Offline(SharedString),
}

pub struct Backend {
    pub connection: Connection,
    pub models: Vec<Model>,
    props: Option<(String, Props)>,
}

impl Backend {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut backend = Backend { connection: Connection::Checking, models: Vec::new(), props: None };
        backend.refresh(cx);
        backend
    }

    pub fn url(cx: &App) -> String {
        Config::get(cx).server_url.clone()
    }

    pub fn selected_id(cx: &App) -> Option<String> {
        Config::get(cx).model.clone()
    }

    pub fn selected<'a>(&'a self, cx: &App) -> Option<&'a Model> {
        let id = Self::selected_id(cx)?;
        self.models.iter().find(|m| m.id == id)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.connection = Connection::Checking;
        cx.notify();
        let url = Self::url(cx);
        let task = cx.background_executor().spawn(async move { Server::new(&url).models() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok(models) => {
                        this.models = models;
                        this.connection = Connection::Online;
                        if let Some(model) = this.selected(cx).filter(|m| m.status == quill_llama::Status::Loaded) {
                            let id = model.id.clone();
                            this.fetch_props(id, cx);
                        }
                    }
                    Err(error) => {
                        this.models.clear();
                        this.connection = Connection::Offline(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn set_url(&mut self, url: &str, cx: &mut Context<Self>) {
        let url = normalize_url(url);
        Config::update(cx, |config| config.server_url = url);
        self.props = None;
        self.refresh(cx);
    }

    pub fn select(&mut self, id: String, cx: &mut Context<Self>) {
        Config::update(cx, |config| config.model = Some(id.clone()));
        self.props = None;
        self.fetch_props(id, cx);
        cx.notify();
    }

    pub fn set_thinking(&mut self, on: bool, cx: &mut Context<Self>) {
        Config::update(cx, |config| config.thinking = on);
        cx.notify();
    }

    pub fn thinking(&self, cx: &App) -> Thinking {
        let Some(id) = Self::selected_id(cx) else { return Thinking::None };
        match &self.props {
            Some((props_id, props)) if *props_id == id => props.thinking,
            _ => guess_thinking(&id),
        }
    }

    pub fn request_thinking(&self, cx: &App) -> Option<bool> {
        (self.thinking(cx) == Thinking::Toggle).then(|| Config::get(cx).thinking)
    }

    pub fn mark_used(&mut self, id: &str, cx: &mut Context<Self>) {
        let needs_props = !matches!(&self.props, Some((props_id, _)) if props_id == id);
        if let Some(model) = self.models.iter_mut().find(|m| m.id == id) {
            model.status = quill_llama::Status::Loaded;
        }
        if needs_props {
            self.fetch_props(id.to_string(), cx);
        }
        cx.notify();
    }

    fn fetch_props(&mut self, id: String, cx: &mut Context<Self>) {
        let url = Self::url(cx);
        let wanted = id.clone();
        let task = cx.background_executor().spawn(async move { Server::new(&url).props(&wanted) });
        cx.spawn(async move |this, cx| {
            if let Ok(props) = task.await {
                _ = this.update(cx, |this, cx| {
                    if let Some(model) = this.models.iter_mut().find(|m| m.id == id) {
                        model.status = quill_llama::Status::Loaded;
                        model.context = props.context.or(model.context);
                    }
                    this.props = Some((id, props));
                    cx.notify();
                });
            }
        })
        .detach();
    }
}