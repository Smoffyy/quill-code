use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use crate::openai::normalize_url;
use crate::{
    ChatRequest, Control, Error, Event, Message, Model, Provider, Reasoning, Role, Status, StopReason, Tokens, agent, authorized,
    read_json, stream_lines,
};

pub const OLLAMA_CONTEXT: u64 = 32768;

pub struct Ollama {
    base: String,
    key: Option<String>,
    agent: ureq::Agent,
}

impl Ollama {
    pub fn new(base_url: &str, key: Option<&str>) -> Ollama {
        let key = key.map(str::trim).filter(|key| !key.is_empty()).map(str::to_string);
        Ollama { base: normalize_url(base_url), key, agent: agent() }
    }

    fn get_json(&self, path: &str) -> Result<Value, Error> {
        read_json(authorized(self.agent.get(format!("{}{path}", self.base)), self.key.as_deref()).call()?)
    }

    fn show(&self, name: &str) -> Option<Value> {
        let post = authorized(self.agent.post(format!("{}/api/show", self.base)), self.key.as_deref());
        read_json(post.send_json(json!({ "model": name })).ok()?).ok()
    }
}

impl Provider for Ollama {
    fn models(&self) -> Result<Vec<Model>, Error> {
        let tags = self.get_json("/api/tags")?;
        let running = self.get_json("/api/ps").unwrap_or_default();
        let names: Vec<String> = tags["models"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(|m| m["name"].as_str().map(str::to_string))
            .collect();
        let details: Vec<Option<Value>> = std::thread::scope(|scope| {
            let jobs: Vec<_> = names.iter().map(|name| scope.spawn(move || self.show(name))).collect();
            jobs.into_iter().map(|job| job.join().ok().flatten()).collect()
        });
        Ok(names.into_iter().zip(details).filter_map(|(name, details)| model(name, details.as_ref(), &running)).collect())
    }

    fn stream(&self, request: &ChatRequest, cancel: &AtomicBool, on_event: &mut dyn FnMut(Event)) -> Result<(), Error> {
        let post = authorized(self.agent.post(format!("{}/api/chat", self.base)), self.key.as_deref());
        let response = post.send_json(chat_body(request))?;
        let mut calls = 0;
        stream_lines(response, cancel, |line| {
            let (events, done) = parse_line(line, &mut calls)?;
            events.into_iter().for_each(&mut *on_event);
            Ok(!done)
        })
    }
}

fn capabilities(details: Option<&Value>) -> Vec<String> {
    let list = details.and_then(|d| d["capabilities"].as_array()).map(Vec::as_slice).unwrap_or(&[]);
    list.iter().filter_map(|c| c.as_str().map(str::to_string)).collect()
}

fn window(details: Option<&Value>) -> Option<u64> {
    let details = details?;
    let parameters = details["parameters"].as_str().unwrap_or_default();
    let chosen = parameters.lines().find_map(|line| line.trim().strip_prefix("num_ctx")?.trim().parse().ok());
    let trained = details["model_info"]
        .as_object()
        .and_then(|info| info.iter().find(|(key, _)| key.ends_with(".context_length")).and_then(|(_, value)| value.as_u64()));
    chosen.or(trained.map(|trained| trained.min(OLLAMA_CONTEXT)))
}

fn model(name: String, details: Option<&Value>, running: &Value) -> Option<Model> {
    let capabilities = capabilities(details);
    if !capabilities.is_empty() && !capabilities.iter().any(|c| c == "completion") {
        return None;
    }
    let loaded = running["models"].as_array().is_some_and(|list| list.iter().any(|m| m["name"] == name.as_str()));
    let has = |capability: &str| capabilities.iter().any(|c| c == capability);
    Some(Model {
        status: if loaded { Status::Loaded } else { Status::Unloaded },
        context: window(details).or(Some(OLLAMA_CONTEXT)),
        vision: has("vision"),
        control: Some(if has("thinking") { Control::toggle() } else { Control::None }),
        ..Model::new(name)
    })
}

fn wire(message: &Message, names: &HashMap<&str, &str>) -> Value {
    let mut value = json!({ "role": message.role, "content": message.content });
    if !message.images.is_empty() {
        let images: Vec<&str> = message.images.iter().map(|url| url.split_once("base64,").map_or(url.as_str(), |(_, data)| data)).collect();
        value["images"] = json!(images);
    }
    if !message.tool_calls.is_empty() {
        let calls: Vec<Value> = message
            .tool_calls
            .iter()
            .map(|call| {
                let arguments: Value = serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
                json!({ "function": { "name": call.function.name, "arguments": arguments } })
            })
            .collect();
        value["tool_calls"] = json!(calls);
    }
    if message.role == Role::Tool
        && let Some(name) = message.tool_call_id.as_deref().and_then(|id| names.get(id))
    {
        value["tool_name"] = json!(name);
    }
    value
}

fn chat_body(request: &ChatRequest) -> Value {
    let names: HashMap<&str, &str> = request
        .messages
        .iter()
        .flat_map(|message| message.tool_calls.iter().map(|call| (call.id.as_str(), call.function.name.as_str())))
        .collect();
    let messages: Vec<Value> = request.messages.iter().map(|message| wire(message, &names)).collect();
    let mut body = json!({ "model": request.model, "messages": messages, "stream": true });
    if let Some(context) = request.context {
        body["options"] = json!({ "num_ctx": context });
    }
    match &request.reasoning {
        Some(Reasoning::Toggle { on, .. }) => body["think"] = json!(on),
        Some(Reasoning::Effort(level)) => body["think"] = json!(level),
        None => {}
    }
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| json!({ "type": "function", "function": { "name": tool.name, "description": tool.description, "parameters": tool.parameters } }))
            .collect();
        body["tools"] = json!(tools);
    }
    body
}

fn parse_line(line: &str, calls: &mut usize) -> Result<(Vec<Event>, bool), Error> {
    let Ok(value) = serde_json::from_str::<Value>(line) else { return Ok((Vec::new(), false)) };
    if let Some(error) = value["error"].as_str() {
        return Err(Error(error.to_string()));
    }
    let mut events = Vec::new();
    let message = &value["message"];
    if let Some(text) = message["thinking"].as_str().filter(|t| !t.is_empty()) {
        events.push(Event::Reasoning(text.to_string()));
    }
    if let Some(text) = message["content"].as_str().filter(|t| !t.is_empty()) {
        events.push(Event::Content(text.to_string()));
    }
    for call in message["tool_calls"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        events.push(Event::ToolCall {
            index: *calls,
            id: None,
            name: call["function"]["name"].as_str().map(str::to_string),
            arguments: call["function"]["arguments"].to_string(),
        });
        *calls += 1;
    }
    let done = value["done"].as_bool().unwrap_or(false);
    if done {
        let count = |key: &str| value[key].as_u64().unwrap_or(0);
        events.push(Event::Usage(Tokens { prompt: count("prompt_eval_count"), completion: count("eval_count"), cache_read: 0 }));
        let stop = match value["done_reason"].as_str() {
            _ if *calls > 0 => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            _ => StopReason::End,
        };
        events.push(Event::Stop(stop));
    }
    Ok((events, done))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolCall, ToolSpec};

    #[test]
    fn models_report_their_window_and_abilities() {
        let details = json!({
            "capabilities": ["completion", "tools", "thinking"],
            "model_info": { "general.architecture": "qwen3", "qwen3.context_length": 40960 },
            "parameters": "temperature 0.6"
        });
        let running = json!({ "models": [{ "name": "qwen3:8b" }] });
        let qwen = model("qwen3:8b".into(), Some(&details), &running).unwrap();
        assert_eq!((qwen.status, qwen.context, qwen.control), (Status::Loaded, Some(OLLAMA_CONTEXT), Some(Control::toggle())));
        let custom = json!({ "capabilities": ["completion", "vision"], "parameters": "num_ctx                        8192" });
        let gemma = model("gemma".into(), Some(&custom), &json!({})).unwrap();
        assert_eq!((gemma.status, gemma.context, gemma.vision), (Status::Unloaded, Some(8192), true));
        assert!(model("nomic-embed-text".into(), Some(&json!({ "capabilities": ["embedding"] })), &json!({})).is_none());
    }

    #[test]
    fn requests_carry_the_window_thinking_and_tool_names() {
        let request = ChatRequest {
            model: "qwen3:8b".into(),
            messages: vec![
                Message::user_with_images("look", vec!["data:image/png;base64,AAAA".into()]),
                Message::assistant("", vec![ToolCall::new("c1", "shell", r#"{"command":"ls"}"#)]),
                Message::tool("c1", "a.txt"),
            ],
            reasoning: Some(Reasoning::Toggle { kwarg: "enable_thinking".into(), on: false }),
            tools: vec![ToolSpec { name: "shell".into(), description: "Run".into(), parameters: json!({ "type": "object" }) }],
            context: Some(32768),
            ..ChatRequest::default()
        };
        let body = chat_body(&request);
        assert_eq!(body["options"]["num_ctx"], 32768);
        assert_eq!(body["think"], false);
        let effort = ChatRequest { reasoning: Some(Reasoning::Effort("high".into())), ..request.clone() };
        assert_eq!(chat_body(&effort)["think"], "high");
        assert_eq!(body["messages"][0]["images"][0], "AAAA");
        assert_eq!(body["messages"][1]["tool_calls"][0]["function"]["arguments"]["command"], "ls");
        assert_eq!(body["messages"][2]["tool_name"], "shell");
        assert_eq!(body["tools"][0]["function"]["name"], "shell");
    }

    #[test]
    fn stream_lines_become_events() {
        let mut calls = 0;
        let (events, done) =
            parse_line(r#"{"message":{"role":"assistant","content":"","thinking":"hm"},"done":false}"#, &mut calls).unwrap();
        assert_eq!((events, done), (vec![Event::Reasoning("hm".into())], false));
        let call = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"shell","arguments":{"command":"ls"}}}]},"done":false}"#;
        let (events, _) = parse_line(call, &mut calls).unwrap();
        assert_eq!(
            events,
            vec![Event::ToolCall { index: 0, id: None, name: Some("shell".into()), arguments: r#"{"command":"ls"}"#.into() }]
        );
        let (events, done) = parse_line(r#"{"done":true,"done_reason":"stop","prompt_eval_count":12,"eval_count":3}"#, &mut calls).unwrap();
        assert!(done);
        assert_eq!(events[1], Event::Stop(StopReason::ToolUse));
        assert!(parse_line(r#"{"error":"model not found"}"#, &mut calls).is_err());
    }
}