use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use crate::{
    ChatRequest, Control, Error, Event, Model, Props, Provider, Reasoning, Status, StopReason, Tokens, agent, authorized, read_json,
    stream_lines,
};

pub struct OpenAi {
    base: String,
    key: Option<String>,
    agent: ureq::Agent,
}

impl OpenAi {
    pub fn new(base_url: &str) -> OpenAi {
        OpenAi { base: normalize_url(base_url), key: None, agent: agent() }
    }

    pub fn with_key(self, key: Option<&str>) -> OpenAi {
        OpenAi { key: key.map(str::trim).filter(|key| !key.is_empty()).map(str::to_string), ..self }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn reachable(&self) -> bool {
        self.agent.get(format!("{}/v1/models", self.base)).call().is_ok()
    }

    pub fn props(&self, model: &str) -> Result<Props, Error> {
        let body = self.get_json(&format!("/props?model={}", encode(model)))?;
        Ok(parse_props(&body))
    }

    pub(crate) fn get_json(&self, path: &str) -> Result<Value, Error> {
        read_json(authorized(self.agent.get(format!("{}{path}", self.base)), self.key.as_deref()).call()?)
    }
}

impl Provider for OpenAi {
    fn models(&self) -> Result<Vec<Model>, Error> {
        Ok(parse_models(&self.get_json("/v1/models")?))
    }

    fn stream(&self, request: &ChatRequest, cancel: &AtomicBool, on_event: &mut dyn FnMut(Event)) -> Result<(), Error> {
        let post = authorized(self.agent.post(format!("{}/v1/chat/completions", self.base)), self.key.as_deref());
        let response = post.send_json(chat_body(request))?;
        let whole = response.headers().get("content-type").and_then(|kind| kind.to_str().ok()).is_some_and(|kind| kind.contains("json"));
        if whole {
            events(&read_json(response)?, "message").into_iter().for_each(on_event);
            return Ok(());
        }
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
    let control = if template.contains("reasoning_effort") {
        Control::effort()
    } else if template.contains("enable_thinking") {
        Control::toggle()
    } else {
        Control::None
    };
    Props {
        context: body["default_generation_settings"]["n_ctx"].as_u64().filter(|n| *n > 0),
        slots: body["total_slots"].as_u64().unwrap_or(1).max(1) as u32,
        vision: body["modalities"]["vision"].as_bool().unwrap_or(false),
        tools: caps["supports_tool_calls"].as_bool().unwrap_or(template.contains("tools")),
        control,
    }
}

fn chat_body(request: &ChatRequest) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(|message| message.wire()).collect();
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    let mut kwargs = request.kwargs.clone();
    match &request.reasoning {
        Some(Reasoning::Toggle { kwarg, on }) => {
            kwargs.insert(kwarg.clone(), json!(on));
        }
        Some(Reasoning::Effort(level)) => {
            kwargs.insert("reasoning_effort".into(), json!(level));
            body["reasoning_effort"] = json!(level);
        }
        None => {}
    }
    if !kwargs.is_empty() {
        body["chat_template_kwargs"] = Value::Object(kwargs);
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
    Some(Sse::Events(events(&value, "delta")))
}

fn events(value: &Value, part: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let choice = &value["choices"][0];
    let delta = &choice[part];
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
        events.push(Event::Usage(Tokens { prompt, completion, cache_read }));
    }
    events
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
    fn props_detect_thinking_and_tools() {
        let body = json!({
            "chat_template": "{%- if enable_thinking is defined and enable_thinking is false %}<think>\n\n</think>{%- endif %}",
            "chat_template_caps": { "supports_tool_calls": true },
            "total_slots": 4,
            "modalities": { "vision": true },
            "default_generation_settings": { "n_ctx": 131072 }
        });
        let props = parse_props(&body);
        assert_eq!(props.control, Control::toggle());
        assert!(props.tools && props.vision);
        assert_eq!(props.slots, 4);
        assert_eq!(props.context, Some(131072));
        assert_eq!(parse_props(&json!({})).control, Control::None);
        assert_eq!(parse_props(&json!({ "chat_template": "Reasoning: {{ reasoning_effort }}" })).control, Control::effort());
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
    fn a_whole_reply_reads_like_a_stream() {
        let reply = json!({
            "choices": [{ "message": { "content": "Hi", "tool_calls": [{ "id": "a", "function": { "name": "shell", "arguments": "{}" } }] }, "finish_reason": "tool_calls" }],
            "usage": { "prompt_tokens": 3, "completion_tokens": 1 }
        });
        assert_eq!(
            events(&reply, "message"),
            vec![
                Event::Content("Hi".into()),
                Event::ToolCall { index: 0, id: Some("a".into()), name: Some("shell".into()), arguments: "{}".into() },
                Event::Stop(StopReason::ToolUse),
                Event::Usage(Tokens { prompt: 3, completion: 1, cache_read: 0 }),
            ]
        );
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
    fn requests_carry_reasoning_kwargs_and_tools() {
        let tool = ToolSpec { name: "read_file".into(), description: "Read".into(), parameters: json!({ "type": "object" }) };
        let request = ChatRequest {
            model: "m".into(),
            messages: vec![Message::user("hi")],
            reasoning: Some(Reasoning::Toggle { kwarg: "enable_thinking".into(), on: false }),
            tools: vec![tool],
            ..ChatRequest::default()
        };
        let body = chat_body(&request);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        let effort = chat_body(&ChatRequest { reasoning: Some(Reasoning::Effort("high".into())), ..request.clone() });
        assert_eq!(
            (effort["reasoning_effort"].clone(), effort["chat_template_kwargs"]["reasoning_effort"].clone()),
            (json!("high"), json!("high"))
        );
        let mut kwargs = serde_json::Map::new();
        kwargs.insert("top_k".into(), json!(20));
        let extra = chat_body(&ChatRequest { reasoning: None, kwargs, ..request.clone() });
        assert_eq!(extra["chat_template_kwargs"], json!({ "top_k": 20 }));
        let plain = chat_body(&ChatRequest { reasoning: None, tools: Vec::new(), ..request });
        assert!(plain.get("chat_template_kwargs").is_none() && plain.get("tools").is_none() && plain.get("reasoning_effort").is_none());
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
    fn model_names_are_encoded() {
        assert_eq!(encode("qwen3.5 4b/q4"), "qwen3.5%204b%2Fq4");
    }
}