use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use crate::{
    ChatRequest, Effort, Error, Event, Model, Props, Provider, Status, StopReason, Thinking, Tokens, agent, read_json, stream_lines,
};

pub const OPENAI_URL: &str = "https://api.openai.com";

pub struct OpenAi {
    base: String,
    key: Option<String>,
    local: bool,
    agent: ureq::Agent,
}

impl OpenAi {
    pub fn new(base_url: &str) -> OpenAi {
        OpenAi { base: normalize_url(base_url), key: None, local: true, agent: agent() }
    }

    pub fn cloud(base_url: &str, key: Option<String>) -> OpenAi {
        OpenAi { key: key.filter(|k| !k.trim().is_empty()), local: false, ..OpenAi::new(base_url) }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn props(&self, model: &str) -> Result<Props, Error> {
        let body = self.get_json(&format!("/props?model={}", encode(model)))?;
        Ok(parse_props(&body))
    }

    fn get_json(&self, path: &str) -> Result<Value, Error> {
        let mut get = self.agent.get(format!("{}{path}", self.base));
        if let Some(key) = &self.key {
            get = get.header("Authorization", format!("Bearer {key}"));
        }
        read_json(get.call()?)
    }
}

impl Provider for OpenAi {
    fn models(&self) -> Result<Vec<Model>, Error> {
        let models = parse_models(&self.get_json("/v1/models")?);
        if self.local {
            return Ok(models);
        }
        Ok(models
            .into_iter()
            .filter(|model| chat_model(&model.id))
            .map(|model| Model { efforts: reasoning_efforts(&model.id), ..model })
            .collect())
    }

    fn stream(&self, request: &ChatRequest, cancel: &AtomicBool, on_event: &mut dyn FnMut(Event)) -> Result<(), Error> {
        let mut post = self.agent.post(format!("{}/v1/chat/completions", self.base));
        if let Some(key) = &self.key {
            post = post.header("Authorization", format!("Bearer {key}"));
        }
        let response = post.send_json(chat_body(request, self.local))?;
        stream_lines(response, cancel, |line| match parse_sse_line(line) {
            Some(Sse::Done) => Ok(false),
            Some(Sse::Error(message)) => Err(Error(message)),
            Some(Sse::Events(events)) => {
                events.into_iter().for_each(&mut *on_event);
                Ok(true)
            }
            None => Ok(true),
        })
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

fn chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    let other = ["embedding", "whisper", "tts", "dall-e", "moderation", "image", "audio", "realtime", "transcribe", "search", "sora"];
    !other.iter().any(|word| id.contains(word))
}

fn reasoning_efforts(id: &str) -> Vec<Effort> {
    let id = id.to_ascii_lowercase();
    let o_series = id.strip_prefix('o').is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    if (id.starts_with("gpt-5") || o_series) && !id.contains("chat") { vec![Effort::Low, Effort::Medium, Effort::High] } else { Vec::new() }
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
            let name = entry["name"].as_str().filter(|name| *name != id).map(str::to_string);
            Some(Model { name, status, context, vision, ..Model::new(id) })
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

fn chat_body(request: &ChatRequest, local: bool) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(|message| message.wire()).collect();
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if local {
        if let Some(thinking) = request.thinking {
            body["chat_template_kwargs"] = json!({ "enable_thinking": thinking });
        }
    } else {
        if let Some(effort) = request.effort {
            let effort = if request.thinking == Some(false) { Effort::Low } else { effort.min(Effort::High) };
            body["reasoning_effort"] = json!(effort.wire());
        }
        if let Some(max) = request.max_tokens {
            body["max_completion_tokens"] = json!(max);
        }
    }
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": { "name": tool.name, "description": tool.description, "parameters": tool.parameters }
                })
            })
            .collect();
    }
    body
}

#[derive(Debug, PartialEq)]
enum Sse {
    Events(Vec<Event>),
    Error(String),
    Done,
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::End,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        "content_filter" => StopReason::Refusal,
        other => StopReason::Other(other.to_string()),
    }
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
    let choice = &value["choices"][0];
    let delta = &choice["delta"];
    let reasoning = delta["reasoning_content"].as_str().or_else(|| delta["reasoning"].as_str());
    if let Some(text) = reasoning.filter(|t| !t.is_empty()) {
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
    if let Some(reason) = choice["finish_reason"].as_str() {
        events.push(Event::Stop(stop_reason(reason)));
    }
    let usage = &value["usage"];
    if let (Some(prompt), Some(completion)) = (usage["prompt_tokens"].as_u64(), usage["completion_tokens"].as_u64()) {
        let cache_read = usage["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
        events.push(Event::Usage(Tokens { prompt, completion, cache_read, cache_write: 0 }));
    }
    Some(Sse::Events(events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, ToolSpec};

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
        assert_eq!(models[0], Model { status: Status::Unloaded, context: Some(65536), vision: true, ..Model::new("qwen3.5-35b-a3b") });
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
            { "id": "router", "context_length": 200000, "name": "Router Model" }
        ]});
        let models = parse_models(&body);
        let windows: Vec<Option<u64>> = models.iter().map(|m| m.context).collect();
        assert_eq!(windows, vec![Some(32768), Some(8192), Some(200000)]);
        assert_eq!(models[2].label(), "Router Model");
    }

    #[test]
    fn cloud_lists_keep_chat_models_with_their_efforts() {
        assert!(chat_model("gpt-5.2") && chat_model("o4-mini"));
        assert!(!chat_model("text-embedding-3-large") && !chat_model("whisper-1"));
        assert_eq!(reasoning_efforts("o3"), vec![Effort::Low, Effort::Medium, Effort::High]);
        assert!(reasoning_efforts("gpt-4.1").is_empty());
        assert!(reasoning_efforts("omni-moderation").is_empty());
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
            Some(Sse::Events(vec![Event::Content("Hi".into()), Event::Usage(Tokens { prompt: 5, completion: 1, ..Tokens::default() })]))
        );
        assert_eq!(parse_sse_line(r#"data: {"error":{"message":"boom"}}"#), Some(Sse::Error("boom".into())));
    }

    #[test]
    fn finish_reasons_become_stops() {
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"finish_reason":"tool_calls","index":0,"delta":{}}]}"#),
            Some(Sse::Events(vec![Event::Stop(StopReason::ToolUse)]))
        );
        assert_eq!(stop_reason("length"), StopReason::MaxTokens);
    }

    #[test]
    fn local_requests_carry_thinking_and_tools() {
        let tool = ToolSpec { name: "read_file".into(), description: "Read".into(), parameters: json!({ "type": "object" }) };
        let request = ChatRequest {
            model: "m".into(),
            messages: vec![Message::user("hi")],
            thinking: Some(false),
            tools: vec![tool],
            ..ChatRequest::default()
        };
        let body = chat_body(&request, true);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        let plain = chat_body(&ChatRequest { thinking: None, tools: Vec::new(), ..request }, true);
        assert!(plain.get("chat_template_kwargs").is_none());
        assert!(plain.get("tools").is_none());
    }

    #[test]
    fn cloud_requests_carry_effort_but_never_template_flags() {
        let request = ChatRequest {
            model: "o3".into(),
            messages: vec![Message::user("hi")],
            thinking: Some(true),
            effort: Some(Effort::Max),
            max_tokens: Some(4000),
            ..ChatRequest::default()
        };
        let body = chat_body(&request, false);
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["max_completion_tokens"], 4000);
        assert!(body.get("chat_template_kwargs").is_none());
        let quick = chat_body(&ChatRequest { thinking: Some(false), ..request }, false);
        assert_eq!(quick["reasoning_effort"], "low");
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
    fn thinking_is_guessed_from_the_name() {
        assert_eq!(guess_thinking("qwen3.5-4b"), Thinking::Toggle);
        assert_eq!(guess_thinking("Qwen3-30B-A3B-Instruct-2507"), Thinking::None);
        assert_eq!(guess_thinking("llama-3.1-8b"), Thinking::None);
    }

    #[test]
    fn model_names_are_encoded() {
        assert_eq!(encode("qwen3.5 4b/q4"), "qwen3.5%204b%2Fq4");
    }
}