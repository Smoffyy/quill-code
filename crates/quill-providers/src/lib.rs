mod lmstudio;
mod message;
mod ollama;
mod openai;
mod profiles;

use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use lmstudio::LmStudio;
pub use message::{FunctionCall, Message, Role, ToolCall};
pub use ollama::{OLLAMA_CONTEXT, Ollama};
pub use openai::{OpenAi, normalize_url, parse_models, parse_props};
pub use profiles::{Control, Profile, Reasoning, Tuning, builtin as builtin_profiles, profile_for};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        let refused = "Nothing is answering at that address. Is the model server running?";
        match error {
            ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => Error(refused.into()),
            ureq::Error::ConnectionFailed => Error(refused.into()),
            ureq::Error::HostNotFound => Error("That host name could not be found.".into()),
            ureq::Error::Timeout(_) => Error("The server took too long to answer.".into()),
            ureq::Error::TlsRequired => Error("That address needs https, which this connection cannot open.".into()),
            ureq::Error::Io(io) => Error(io.to_string()),
            other => Error(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Loaded,
    Loading,
    Sleeping,
    Unloaded,
}

impl Status {
    fn parse(value: &str) -> Status {
        match value {
            "loaded" => Status::Loaded,
            "loading" => Status::Loading,
            "sleeping" => Status::Sleeping,
            _ => Status::Unloaded,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Loaded => "Loaded",
            Status::Loading => "Loading",
            Status::Sleeping => "Sleeping",
            Status::Unloaded => "Not loaded",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: Option<String>,
    pub status: Status,
    pub context: Option<u64>,
    pub vision: bool,
    pub control: Option<Control>,
}

impl Model {
    pub fn new(id: impl Into<String>) -> Model {
        Model { id: id.into(), name: None, status: Status::Loaded, context: None, vision: false, control: None }
    }

    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Props {
    pub context: Option<u64>,
    pub slots: u32,
    pub vision: bool,
    pub tools: bool,
    pub control: Control,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub reasoning: Option<Reasoning>,
    pub kwargs: Map<String, Value>,
    pub tools: Vec<ToolSpec>,
    pub context: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    End,
    ToolUse,
    MaxTokens,
    Refusal,
    Other(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tokens {
    pub prompt: u64,
    pub completion: u64,
    pub cache_read: u64,
}

impl Tokens {
    pub fn total(self) -> u64 {
        self.prompt + self.completion
    }

    pub fn add(&mut self, other: Tokens) {
        self.prompt += other.prompt;
        self.completion += other.completion;
        self.cache_read += other.cache_read;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Reasoning(String),
    Content(String),
    ToolCall { index: usize, id: Option<String>, name: Option<String>, arguments: String },
    Usage(Tokens),
    Stop(StopReason),
}

pub trait Provider: Send + Sync {
    fn models(&self) -> Result<Vec<Model>, Error>;

    fn stream(&self, request: &ChatRequest, cancel: &AtomicBool, on_event: &mut dyn FnMut(Event)) -> Result<(), Error>;
}

pub fn context_label(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1024 && tokens.is_multiple_of(1024) {
        format!("{}K", tokens / 1024)
    } else if tokens >= 1000 {
        format!("{}K", tokens / 1000)
    } else {
        tokens.to_string()
    }
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = TlsConfig::builder().provider(TlsProvider::Rustls).root_certs(RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(8)))
        .http_status_as_error(false)
        .tls_config(tls)
        .build()
        .into()
}

fn authorized<B>(request: ureq::RequestBuilder<B>, key: Option<&str>) -> ureq::RequestBuilder<B> {
    match key {
        Some(key) => request.header("Authorization", format!("Bearer {key}")),
        None => request,
    }
}

fn error_message(status: u16, body: &str) -> String {
    if matches!(status, 401 | 403) {
        return format!("The server turned the request down ({status}). Check the API key.");
    }
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().or(v["error"].as_str()).map(str::to_string))
        .unwrap_or_else(|| body.trim().chars().take(300).collect());
    if detail.is_empty() { format!("The server answered {status}.") } else { format!("The server answered {status}: {detail}") }
}

fn read_json(response: ureq::http::Response<ureq::Body>) -> Result<Value, Error> {
    let status = response.status().as_u16();
    let text = response.into_body().read_to_string()?;
    if status >= 400 {
        return Err(Error(error_message(status, &text)));
    }
    serde_json::from_str(&text).map_err(|e| Error(format!("The server sent something that is not JSON: {e}")))
}

fn stream_lines(
    response: ureq::http::Response<ureq::Body>,
    cancel: &AtomicBool,
    mut each: impl FnMut(&str) -> Result<bool, Error>,
) -> Result<(), Error> {
    use std::io::BufRead;
    let status = response.status().as_u16();
    let mut body = response.into_body();
    if status >= 400 {
        let text = body.read_to_string().unwrap_or_default();
        return Err(Error(error_message(status, &text)));
    }
    for line in std::io::BufReader::new(body.into_reader()).lines() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        let line = line.map_err(|e| Error(e.to_string()))?;
        if !each(&line)? {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_sizes_read_naturally() {
        assert_eq!(context_label(131072), "128K");
        assert_eq!(context_label(65536), "64K");
        assert_eq!(context_label(32000), "32K");
        assert_eq!(context_label(1_000_000), "1M");
        assert_eq!(context_label(512), "512");
    }
}