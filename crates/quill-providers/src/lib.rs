mod anthropic;
mod message;
mod openai;

use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use anthropic::{ANTHROPIC_URL, Anthropic, image_block};
pub use message::{FunctionCall, Message, Role, ToolCall};
pub use openai::{OPENAI_URL, OpenAi, guess_thinking, normalize_url, parse_models, parse_props};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 5] = [Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max];

    pub fn wire(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Effort::Low => "Low",
            Effort::Medium => "Medium",
            Effort::High => "High",
            Effort::XHigh => "Extra high",
            Effort::Max => "Max",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Effort::Low => "Quick answers, least thinking",
            Effort::Medium => "Balanced speed and depth",
            Effort::High => "Thinks things through",
            Effort::XHigh => "Deeper reasoning for hard problems",
            Effort::Max => "Everything it has, slowest",
        }
    }

    pub fn nearest(self, offered: &[Effort]) -> Option<Effort> {
        offered.iter().copied().min_by_key(|effort| (*effort as i32 - self as i32).abs() * 2 + i32::from(*effort > self))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: Option<String>,
    pub status: Status,
    pub context: Option<u64>,
    pub max_output: Option<u64>,
    pub vision: bool,
    pub efforts: Vec<Effort>,
    pub server_compaction: bool,
}

impl Model {
    pub fn new(id: impl Into<String>) -> Model {
        Model {
            id: id.into(),
            name: None,
            status: Status::Loaded,
            context: None,
            max_output: None,
            vision: false,
            efforts: Vec::new(),
            server_compaction: false,
        }
    }

    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    None,
    Toggle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Props {
    pub context: Option<u64>,
    pub slots: u32,
    pub vision: bool,
    pub tools: bool,
    pub thinking: Thinking,
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
    pub thinking: Option<bool>,
    pub effort: Option<Effort>,
    pub max_tokens: Option<u64>,
    pub server_compaction: bool,
    pub tools: Vec<ToolSpec>,
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
    pub cache_write: u64,
}

impl Tokens {
    pub fn from_anthropic(usage: &Value) -> Tokens {
        let count = |key: &str| usage[key].as_u64().unwrap_or(0);
        let (cache_read, cache_write) = (count("cache_read_input_tokens"), count("cache_creation_input_tokens"));
        Tokens { prompt: count("input_tokens") + cache_read + cache_write, completion: count("output_tokens"), cache_read, cache_write }
    }

    pub fn total(self) -> u64 {
        self.prompt + self.completion
    }

    pub fn add(&mut self, other: Tokens) {
        self.prompt += other.prompt;
        self.completion += other.completion;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Reasoning(String),
    Content(String),
    ToolCall { index: usize, id: Option<String>, name: Option<String>, arguments: String },
    Usage(Tokens),
    Native(Value),
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
    let tls = TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(8)))
        .http_status_as_error(false)
        .tls_config(tls)
        .build()
        .into()
}

fn error_message(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
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

    #[test]
    fn efforts_fall_back_to_the_nearest_offered() {
        let offered = [Effort::Low, Effort::Medium, Effort::High];
        assert_eq!(Effort::Max.nearest(&offered), Some(Effort::High));
        assert_eq!(Effort::Medium.nearest(&offered), Some(Effort::Medium));
        assert_eq!(Effort::XHigh.nearest(&[Effort::Low, Effort::Max]), Some(Effort::Max));
        assert_eq!(Effort::High.nearest(&[]), None);
        assert_eq!(serde_json::to_string(&Effort::XHigh).unwrap(), "\"xhigh\"");
    }
}