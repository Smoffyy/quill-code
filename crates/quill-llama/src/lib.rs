use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
        let refused = "Nothing is answering at that address. Is llama-server running?";
        match error {
            ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => Error(refused.into()),
            ureq::Error::ConnectionFailed => Error(refused.into()),
            ureq::Error::HostNotFound => Error("That host name could not be found.".into()),
            ureq::Error::Timeout(_) => Error("The server took too long to answer.".into()),
            ureq::Error::TlsRequired => Error("That address needs https, which quill-code cannot open yet.".into()),
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
    pub status: Status,
    pub context: Option<u64>,
    pub vision: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: impl Into<String>) -> ToolCall {
        ToolCall { id: id.into(), kind: "function".into(), function: FunctionCall { name: name.into(), arguments: arguments.into() } }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(from = "RawMessage")]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub images: Vec<String>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
}

#[derive(Deserialize)]
struct RawMessage {
    role: Role,
    #[serde(default)]
    content: Value,
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
    #[serde(default)]
    tool_call_id: Option<String>,
}

impl From<RawMessage> for Message {
    fn from(raw: RawMessage) -> Message {
        let (content, images) = match raw.content {
            Value::String(text) => (text, Vec::new()),
            Value::Array(parts) => {
                let text = parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n");
                let images = parts.iter().filter_map(|p| p["image_url"]["url"].as_str().map(str::to_string)).collect();
                (text, images)
            }
            _ => (String::new(), Vec::new()),
        };
        Message { role: raw.role, content, images, tool_calls: raw.tool_calls, tool_call_id: raw.tool_call_id }
    }
}

impl Serialize for Message {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("role", &self.role)?;
        if self.images.is_empty() {
            map.serialize_entry("content", &self.content)?;
        } else {
            let mut parts = vec![json!({ "type": "text", "text": self.content })];
            parts.extend(self.images.iter().map(|url| json!({ "type": "image_url", "image_url": { "url": url } })));
            map.serialize_entry("content", &parts)?;
        }
        if !self.tool_calls.is_empty() {
            map.serialize_entry("tool_calls", &self.tool_calls)?;
        }
        if let Some(id) = &self.tool_call_id {
            map.serialize_entry("tool_call_id", id)?;
        }
        map.end()
    }
}

impl Message {
    fn new(role: Role, content: impl Into<String>) -> Message {
        Message { role, content: content.into(), images: Vec::new(), tool_calls: Vec::new(), tool_call_id: None }
    }

    pub fn system(content: impl Into<String>) -> Message {
        Message::new(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Message {
        Message::new(Role::User, content)
    }

    pub fn user_with_images(content: impl Into<String>, images: Vec<String>) -> Message {
        Message { images, ..Message::new(Role::User, content) }
    }

    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Message {
        Message { tool_calls, ..Message::new(Role::Assistant, content) }
    }

    pub fn tool(id: impl Into<String>, content: impl Into<String>) -> Message {
        Message { tool_call_id: Some(id.into()), ..Message::new(Role::Tool, content) }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub thinking: Option<bool>,
    pub tools: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Reasoning(String),
    Content(String),
    ToolCall { index: usize, id: Option<String>, name: Option<String>, arguments: String },
    Usage { prompt: u64, completion: u64 },
}

pub struct Server {
    base: String,
    agent: ureq::Agent,
}

impl Server {
    pub fn new(base_url: &str) -> Server {
        let agent = ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(5))).http_status_as_error(false).build().into();
        Server { base: normalize_url(base_url), agent }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn models(&self) -> Result<Vec<Model>, Error> {
        let body = self.get_json("/v1/models")?;
        Ok(parse_models(&body))
    }

    pub fn props(&self, model: &str) -> Result<Props, Error> {
        let body = self.get_json(&format!("/props?model={}", encode(model)))?;
        Ok(parse_props(&body))
    }

    pub fn stream_chat(&self, request: &ChatRequest, cancel: &AtomicBool, mut on_event: impl FnMut(Event)) -> Result<(), Error> {
        let response = self.agent.post(format!("{}/v1/chat/completions", self.base)).send_json(chat_body(request))?;
        let status = response.status().as_u16();
        let mut body = response.into_body();
        if status >= 400 {
            let text = body.read_to_string().unwrap_or_default();
            return Err(Error(error_message(status, &text)));
        }
        let reader = BufReader::new(body.into_reader());
        for line in reader.lines() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let line = line.map_err(|e| Error(e.to_string()))?;
            match parse_sse_line(&line) {
                Some(Sse::Done) => break,
                Some(Sse::Error(message)) => return Err(Error(message)),
                Some(Sse::Events(events)) => events.into_iter().for_each(&mut on_event),
                None => {}
            }
        }
        Ok(())
    }

    fn get_json(&self, path: &str) -> Result<Value, Error> {
        let response = self.agent.get(format!("{}{path}", self.base)).call()?;
        let status = response.status().as_u16();
        let text = response.into_body().read_to_string()?;
        if status >= 400 {
            return Err(Error(error_message(status, &text)));
        }
        serde_json::from_str(&text).map_err(|e| Error(format!("The server sent something that is not JSON: {e}")))
    }
}

pub fn normalize_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let trimmed = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    if trimmed.contains("://") { trimmed.to_string() } else { format!("http://{trimmed}") }
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn error_message(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.trim().chars().take(300).collect());
    if detail.is_empty() { format!("The server answered {status}.") } else { format!("The server answered {status}: {detail}") }
}

fn ctx_size_arg(args: &[Value]) -> Option<u64> {
    args.windows(2).find(|pair| matches!(pair[0].as_str(), Some("--ctx-size") | Some("-c"))).and_then(|pair| pair[1].as_str()?.parse().ok())
}

pub fn parse_models(body: &Value) -> Vec<Model> {
    let Some(list) = body["data"].as_array() else { return Vec::new() };
    list.iter()
        .filter_map(|entry| {
            let id = entry["id"].as_str()?.to_string();
            let status = entry["status"]["value"].as_str().map_or(Status::Loaded, Status::parse);
            let args = entry["status"]["args"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            let context = entry["meta"]["n_ctx"]
                .as_u64()
                .or_else(|| ctx_size_arg(args))
                .or_else(|| {
                    ["max_model_len", "loaded_context_length", "context_length", "max_context_length"]
                        .iter()
                        .find_map(|k| entry[k].as_u64())
                })
                .or_else(|| entry["meta"]["n_ctx_train"].as_u64());
            let vision = entry["architecture"]["input_modalities"].as_array().is_some_and(|m| m.iter().any(|v| v == "image"));
            Some(Model { id, status, context, vision })
        })
        .collect()
}

pub fn parse_props(body: &Value) -> Props {
    let template = body["chat_template"].as_str().unwrap_or_default();
    let caps = &body["chat_template_caps"];
    let thinking = if template.contains("enable_thinking") { Thinking::Toggle } else { Thinking::None };
    Props {
        context: body["default_generation_settings"]["n_ctx"].as_u64().filter(|n| *n > 0),
        slots: body["total_slots"].as_u64().unwrap_or(1).max(1) as u32,
        vision: body["modalities"]["vision"].as_bool().unwrap_or(false),
        tools: caps["supports_tool_calls"].as_bool().unwrap_or(template.contains("tools")),
        thinking,
    }
}

pub fn guess_thinking(model: &str) -> Thinking {
    let id = model.to_ascii_lowercase();
    let toggles = ["qwen3", "glm-4.5", "glm-4.6", "glm-4.7", "hunyuan"];
    if toggles.iter().any(|family| id.contains(family)) && !id.contains("instruct") { Thinking::Toggle } else { Thinking::None }
}

pub fn context_label(tokens: u64) -> String {
    if tokens >= 1024 && tokens.is_multiple_of(1024) {
        format!("{}K", tokens / 1024)
    } else if tokens >= 1000 {
        format!("{}K", tokens / 1000)
    } else {
        tokens.to_string()
    }
}

fn chat_body(request: &ChatRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "messages": request.messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(thinking) = request.thinking {
        body["chat_template_kwargs"] = json!({ "enable_thinking": thinking });
    }
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(request.tools.clone());
    }
    body
}

#[derive(Debug, PartialEq)]
enum Sse {
    Events(Vec<Event>),
    Error(String),
    Done,
}

fn parse_sse_line(line: &str) -> Option<Sse> {
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" {
        return Some(Sse::Done);
    }
    let value: Value = serde_json::from_str(data).ok()?;
    if let Some(message) = value["error"]["message"].as_str() {
        return Some(Sse::Error(message.to_string()));
    }
    let mut events = Vec::new();
    let delta = &value["choices"][0]["delta"];
    if let Some(text) = delta["reasoning_content"].as_str().filter(|t| !t.is_empty()) {
        events.push(Event::Reasoning(text.to_string()));
    }
    if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
        events.push(Event::Content(text.to_string()));
    }
    for (position, call) in delta["tool_calls"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().enumerate() {
        events.push(Event::ToolCall {
            index: call["index"].as_u64().map_or(position, |i| i as usize),
            id: call["id"].as_str().map(str::to_string),
            name: call["function"]["name"].as_str().map(str::to_string),
            arguments: call["function"]["arguments"].as_str().unwrap_or_default().to_string(),
        });
    }
    if let (Some(prompt), Some(completion)) = (value["usage"]["prompt_tokens"].as_u64(), value["usage"]["completion_tokens"].as_u64()) {
        events.push(Event::Usage { prompt, completion });
    }
    Some(Sse::Events(events))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_normalized() {
        assert_eq!(normalize_url("127.0.0.1:9931/"), "http://127.0.0.1:9931");
        assert_eq!(normalize_url("http://localhost:8080/v1"), "http://localhost:8080");
        assert_eq!(normalize_url(" https://box.lan "), "https://box.lan");
    }

    #[test]
    fn router_models_are_parsed() {
        let body = json!({ "data": [
            { "id": "qwen3.5-35b-a3b", "status": { "value": "unloaded", "args": ["llama-server.exe", "--ctx-size", "65536"] },
              "architecture": { "input_modalities": ["text", "image"] }, "context_length": 262144 },
            { "id": "qwen3.5-4b", "status": { "value": "sleeping", "args": ["--ctx-size", "131072"] },
              "architecture": { "input_modalities": ["text"] }, "meta": { "n_ctx": 131072, "n_ctx_train": 262144 } },
            { "id": "single" }
        ]});
        let models = parse_models(&body);
        assert_eq!(models.len(), 3);
        assert_eq!(models[0], Model { id: "qwen3.5-35b-a3b".into(), status: Status::Unloaded, context: Some(65536), vision: true });
        assert_eq!(models[1].status, Status::Sleeping);
        assert_eq!(models[1].context, Some(131072));
        assert!(!models[1].vision);
        assert_eq!(models[2].status, Status::Loaded);
    }

    #[test]
    fn other_servers_report_their_window() {
        let body = json!({ "data": [
            { "id": "vllm", "max_model_len": 32768 },
            { "id": "lmstudio", "loaded_context_length": 8192, "max_context_length": 131072 },
            { "id": "router", "context_length": 200000 }
        ]});
        let windows: Vec<Option<u64>> = parse_models(&body).into_iter().map(|m| m.context).collect();
        assert_eq!(windows, vec![Some(32768), Some(8192), Some(200000)]);
    }

    #[test]
    fn props_detect_thinking_and_tools() {
        let body = json!({
            "chat_template": "{%- if enable_thinking is defined and enable_thinking is false %}<think>\n\n</think>{%- endif %}",
            "chat_template_caps": { "supports_tool_calls": true },
            "total_slots": 4,
            "modalities": { "vision": true },
            "default_generation_settings": { "n_ctx": 131072 }
        });
        let props = parse_props(&body);
        assert_eq!(props.thinking, Thinking::Toggle);
        assert!(props.tools && props.vision);
        assert_eq!(props.slots, 4);
        assert_eq!(props.context, Some(131072));
        assert_eq!(parse_props(&json!({})).thinking, Thinking::None);
    }

    #[test]
    fn sse_lines_become_events() {
        assert_eq!(parse_sse_line("data: [DONE]"), Some(Sse::Done));
        assert_eq!(parse_sse_line(": keep-alive"), None);
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{"reasoning_content":"hm"}}]}"#),
            Some(Sse::Events(vec![Event::Reasoning("hm".into())]))
        );
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{"content":"Hi"}}],"usage":{"prompt_tokens":5,"completion_tokens":1}}"#),
            Some(Sse::Events(vec![Event::Content("Hi".into()), Event::Usage { prompt: 5, completion: 1 }]))
        );
        assert_eq!(parse_sse_line(r#"data: {"error":{"message":"boom"}}"#), Some(Sse::Error("boom".into())));
    }

    #[test]
    fn thinking_goes_through_template_kwargs() {
        let request = ChatRequest { model: "m".into(), messages: vec![Message::user("hi")], thinking: Some(false), tools: Vec::new() };
        let body = chat_body(&request);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(body["messages"][0]["role"], "user");
        assert!(body.get("tools").is_none());
        let plain = chat_body(&ChatRequest { thinking: None, ..request });
        assert!(plain.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn tool_calls_stream_in_pieces() {
        let first = parse_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"abc","type":"function","function":{"name":"read_file","arguments":"{"}}]}}]}"#,
        );
        assert_eq!(
            first,
            Some(Sse::Events(vec![Event::ToolCall {
                index: 0,
                id: Some("abc".into()),
                name: Some("read_file".into()),
                arguments: "{".into()
            }]))
        );
        let next = parse_sse_line(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"path\""}}]}}]}"#);
        assert_eq!(next, Some(Sse::Events(vec![Event::ToolCall { index: 0, id: None, name: None, arguments: "\"path\"".into() }])));
    }

    #[test]
    fn images_become_content_parts_and_back() {
        let message = Message::user_with_images("look", vec!["data:image/png;base64,AAAA".into()]);
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["content"][0]["text"], "look");
        assert_eq!(value["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");
        let back: Message = serde_json::from_value(value).unwrap();
        assert_eq!(back, message);
        let plain: Message = serde_json::from_value(json!({ "role": "user", "content": "hi" })).unwrap();
        assert_eq!(plain, Message::user("hi"));
    }

    #[test]
    fn tool_messages_serialize_like_openai() {
        let call = ToolCall::new("c1", "read_file", r#"{"path":"a"}"#);
        let assistant = serde_json::to_value(Message::assistant("", vec![call])).unwrap();
        assert_eq!(assistant["tool_calls"][0]["type"], "function");
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "read_file");
        let result = serde_json::to_value(Message::tool("c1", "text")).unwrap();
        assert_eq!(result["role"], "tool");
        assert_eq!(result["tool_call_id"], "c1");
        assert!(serde_json::to_value(Message::user("x")).unwrap().get("tool_calls").is_none());
    }

    #[test]
    fn thinking_is_guessed_from_the_name() {
        assert_eq!(guess_thinking("qwen3.5-4b"), Thinking::Toggle);
        assert_eq!(guess_thinking("Qwen3-30B-A3B-Instruct-2507"), Thinking::None);
        assert_eq!(guess_thinking("llama-3.1-8b"), Thinking::None);
    }

    #[test]
    fn context_sizes_read_naturally() {
        assert_eq!(context_label(131072), "128K");
        assert_eq!(context_label(65536), "64K");
        assert_eq!(context_label(32000), "32K");
        assert_eq!(context_label(512), "512");
    }

    #[test]
    fn model_names_are_encoded() {
        assert_eq!(encode("qwen3.5 4b/q4"), "qwen3.5%204b%2Fq4");
    }
}