use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use crate::{ChatRequest, Effort, Error, Event, Message, Model, Provider, Role, StopReason, Tokens, agent, read_json, stream_lines};

pub const ANTHROPIC_URL: &str = "https://api.anthropic.com";
const VERSION: &str = "2023-06-01";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const COMPACTION_BETA: &str = "compact-2026-01-12";
const MAX_TOKENS: u64 = 32_000;

pub struct Anthropic {
    base: String,
    key: String,
    fallback: bool,
    agent: ureq::Agent,
}

impl Anthropic {
    pub fn new(base_url: &str, key: &str) -> Anthropic {
        let base = base_url.trim().trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let base = if base.is_empty() { ANTHROPIC_URL } else { base };
        Anthropic { base: base.to_string(), key: key.trim().to_string(), fallback: true, agent: agent() }
    }

    pub fn with_fallback(self, fallback: bool) -> Anthropic {
        Anthropic { fallback, ..self }
    }
}

impl Provider for Anthropic {
    fn models(&self) -> Result<Vec<Model>, Error> {
        let response = self
            .agent
            .get(format!("{}/v1/models?limit=1000", self.base))
            .header("x-api-key", &self.key)
            .header("anthropic-version", VERSION)
            .call()?;
        Ok(parse_models(&read_json(response)?))
    }

    fn stream(&self, request: &ChatRequest, cancel: &AtomicBool, on_event: &mut dyn FnMut(Event)) -> Result<(), Error> {
        let fallback = self.fallback && supports_fallback(&request.model);
        let mut betas = Vec::new();
        if fallback {
            betas.push(FALLBACK_BETA);
        }
        if request.server_compaction {
            betas.push(COMPACTION_BETA);
        }
        let mut post =
            self.agent.post(format!("{}/v1/messages", self.base)).header("x-api-key", &self.key).header("anthropic-version", VERSION);
        if !betas.is_empty() {
            post = post.header("anthropic-beta", betas.join(","));
        }
        let response = post.send_json(body(request, fallback))?;
        let mut blocks = Blocks::default();
        stream_lines(response, cancel, |line| {
            let Some(data) = line.strip_prefix("data:") else { return Ok(true) };
            match serde_json::from_str::<Value>(data.trim()) {
                Ok(value) => blocks.apply(&value, on_event).map(|_| true),
                Err(_) => Ok(true),
            }
        })?;
        if let Some(native) = blocks.native() {
            on_event(Event::Native(native));
        }
        Ok(())
    }
}

fn supports_fallback(model: &str) -> bool {
    ["claude-fable-5-1", "claude-opus-5", "claude-sonnet-5-5"].iter().any(|family| model.starts_with(family))
}

pub fn parse_models(body: &Value) -> Vec<Model> {
    let Some(list) = body["data"].as_array() else { return Vec::new() };
    list.iter()
        .filter_map(|entry| {
            let id = entry["id"].as_str()?.to_string();
            let caps = &entry["capabilities"];
            let supported = |value: &Value| value["supported"].as_bool().unwrap_or(false);
            let adaptive = supported(&caps["thinking"]["types"]["adaptive"]);
            let mut efforts: Vec<Effort> = Effort::ALL.into_iter().filter(|effort| supported(&caps["effort"][effort.wire()])).collect();
            if adaptive && efforts.is_empty() {
                efforts = vec![Effort::Low, Effort::Medium, Effort::High];
            }
            if !adaptive {
                efforts.clear();
            }
            Some(Model {
                name: entry["display_name"].as_str().map(str::to_string),
                context: entry["max_input_tokens"].as_u64(),
                max_output: entry["max_tokens"].as_u64(),
                vision: supported(&caps["image_input"]),
                efforts,
                server_compaction: supported(&caps["context_management"]["compact_20260112"]),
                ..Model::new(id)
            })
        })
        .collect()
}

fn body(request: &ChatRequest, fallback: bool) -> Value {
    let system: Vec<&str> = request.messages.iter().filter(|m| m.role == Role::System).map(|m| m.content.as_str()).collect();
    let mut messages: Vec<Value> = Vec::new();
    for message in &request.messages {
        match message.role {
            Role::System => {}
            Role::User => push(&mut messages, "user", user_blocks(message)),
            Role::Assistant => push(&mut messages, "assistant", assistant_blocks(message)),
            Role::Tool => push(&mut messages, "user", vec![tool_result(message)]),
        }
    }
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens.unwrap_or(MAX_TOKENS),
        "messages": messages,
        "stream": true,
        "cache_control": { "type": "ephemeral" },
    });
    if !system.is_empty() {
        body["system"] = json!([{ "type": "text", "text": system.join("\n\n"), "cache_control": { "type": "ephemeral" } }]);
    }
    if !request.tools.is_empty() {
        let mut tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({ "name": tool.name, "description": tool.description, "input_schema": tool.parameters, "eager_input_streaming": true })
            })
            .collect();
        if let Some(last) = tools.last_mut() {
            last["cache_control"] = json!({ "type": "ephemeral" });
        }
        body["tools"] = json!(tools);
    }
    if let Some(effort) = request.effort {
        let effort = if request.thinking == Some(false) { Effort::Low } else { effort };
        body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
        body["output_config"] = json!({ "effort": effort.wire() });
    }
    if fallback {
        body["fallbacks"] = json!("default");
    }
    if request.server_compaction {
        body["context_management"] = json!({ "edits": [{ "type": "compact_20260112" }] });
    }
    body
}

fn push(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    match messages.last_mut() {
        Some(last) if last["role"] == role => {
            if let Some(content) = last["content"].as_array_mut() {
                content.extend(blocks);
            }
        }
        _ => messages.push(json!({ "role": role, "content": blocks })),
    }
}

fn user_blocks(message: &Message) -> Vec<Value> {
    let mut blocks: Vec<Value> = message.images.iter().map(|url| image_block(url)).collect();
    if !message.content.trim().is_empty() {
        blocks.push(json!({ "type": "text", "text": message.content }));
    }
    blocks
}

pub fn image_block(url: &str) -> Value {
    let inline = url.strip_prefix("data:").and_then(|rest| rest.split_once(";base64,"));
    match inline {
        Some((media_type, data)) => json!({ "type": "image", "source": { "type": "base64", "media_type": media_type, "data": data } }),
        None => json!({ "type": "image", "source": { "type": "url", "url": url } }),
    }
}

fn spoken(block: &Value) -> bool {
    match block["type"].as_str() {
        Some("text") => block["text"].as_str().is_some_and(|text| !text.is_empty()),
        Some("tool_use") => true,
        _ => false,
    }
}

fn assistant_blocks(message: &Message) -> Vec<Value> {
    if let Some(native) = message.native.as_ref().and_then(Value::as_array).filter(|blocks| blocks.iter().any(spoken)) {
        return native.iter().filter(|block| block["type"] != "text" || spoken(block)).cloned().collect();
    }
    let mut blocks = Vec::new();
    if !message.content.trim().is_empty() {
        blocks.push(json!({ "type": "text", "text": message.content }));
    }
    for call in &message.tool_calls {
        let input = serde_json::from_str::<Value>(&call.function.arguments).ok().filter(Value::is_object).unwrap_or_else(|| json!({}));
        blocks.push(json!({ "type": "tool_use", "id": call.id, "name": call.function.name, "input": input }));
    }
    blocks
}

fn tool_result(message: &Message) -> Value {
    let content = if message.content.is_empty() { "(no output)" } else { message.content.as_str() };
    json!({
        "type": "tool_result",
        "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
        "content": content,
        "is_error": content.starts_with("Error:"),
    })
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" | "stop_sequence" => StopReason::End,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
        "refusal" => StopReason::Refusal,
        other => StopReason::Other(other.to_string()),
    }
}

#[derive(Default)]
struct Blocks {
    blocks: Vec<Value>,
    inputs: Vec<String>,
    tools: Vec<Option<usize>>,
    calls: usize,
    tokens: Tokens,
}

impl Blocks {
    fn slot(&mut self, index: usize) {
        if self.blocks.len() <= index {
            self.blocks.resize(index + 1, Value::Null);
            self.inputs.resize(index + 1, String::new());
            self.tools.resize(index + 1, None);
        }
    }

    fn apply(&mut self, value: &Value, emit: &mut dyn FnMut(Event)) -> Result<(), Error> {
        let index = value["index"].as_u64().unwrap_or(0) as usize;
        match value["type"].as_str().unwrap_or_default() {
            "message_start" => {
                let usage = &value["message"]["usage"];
                self.tokens = Tokens::from_anthropic(usage);
            }
            "content_block_start" => {
                self.slot(index);
                let block = value["content_block"].clone();
                if block["type"] == "tool_use" {
                    self.tools[index] = Some(self.calls);
                    emit(Event::ToolCall {
                        index: self.calls,
                        id: block["id"].as_str().map(str::to_string),
                        name: block["name"].as_str().map(str::to_string),
                        arguments: String::new(),
                    });
                    self.calls += 1;
                }
                self.blocks[index] = block;
            }
            "content_block_delta" => {
                self.slot(index);
                let delta = &value["delta"];
                let block = &mut self.blocks[index];
                match delta["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or_default();
                        block["text"] = json!(format!("{}{text}", block["text"].as_str().unwrap_or_default()));
                        if !text.is_empty() {
                            emit(Event::Content(text.to_string()));
                        }
                    }
                    "thinking_delta" => {
                        let text = delta["thinking"].as_str().unwrap_or_default();
                        block["thinking"] = json!(format!("{}{text}", block["thinking"].as_str().unwrap_or_default()));
                        if !text.is_empty() {
                            emit(Event::Reasoning(text.to_string()));
                        }
                    }
                    "signature_delta" => block["signature"] = delta["signature"].clone(),
                    "input_json_delta" => {
                        let partial = delta["partial_json"].as_str().unwrap_or_default();
                        self.inputs[index].push_str(partial);
                        if let Some(call) = self.tools[index] {
                            emit(Event::ToolCall { index: call, id: None, name: None, arguments: partial.to_string() });
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                self.slot(index);
                if self.tools[index].is_some() {
                    let input = serde_json::from_str::<Value>(&self.inputs[index]).ok().filter(Value::is_object);
                    self.blocks[index]["input"] = input.unwrap_or_else(|| json!({}));
                }
            }
            "message_delta" => {
                let usage = &value["usage"];
                let latest = Tokens::from_anthropic(usage);
                self.tokens.completion = latest.completion.max(self.tokens.completion);
                if latest.prompt > 0 {
                    self.tokens = Tokens { completion: self.tokens.completion, ..latest };
                }
                emit(Event::Usage(self.tokens));
                if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                    emit(Event::Stop(stop_reason(reason)));
                }
            }
            "error" => {
                let message = value["error"]["message"].as_str().unwrap_or("The model stream failed.");
                return Err(Error(message.to_string()));
            }
            _ => {}
        }
        Ok(())
    }

    fn native(self) -> Option<Value> {
        let blocks: Vec<Value> = self.blocks.into_iter().filter(|block| !block.is_null()).collect();
        (!blocks.is_empty()).then_some(Value::Array(blocks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolCall, ToolSpec};

    fn run(lines: &[Value]) -> (Vec<Event>, Option<Value>) {
        let mut blocks = Blocks::default();
        let mut events = Vec::new();
        for line in lines {
            blocks.apply(line, &mut |event| events.push(event)).unwrap();
        }
        (events, blocks.native())
    }

    #[test]
    fn models_report_window_vision_and_efforts() {
        let body = json!({ "data": [
            { "id": "claude-opus-5-5", "display_name": "Claude Opus 5.5", "max_input_tokens": 1000000, "max_tokens": 128000,
              "capabilities": { "image_input": { "supported": true },
                "thinking": { "supported": true, "types": { "adaptive": { "supported": true } } },
                "effort": { "supported": true, "low": { "supported": true }, "medium": { "supported": true }, "high": { "supported": true },
                            "xhigh": { "supported": true }, "max": { "supported": true } },
                "context_management": { "compact_20260112": { "supported": true } } } },
            { "id": "claude-haiku-4-5", "display_name": "Claude Haiku 4.5", "max_input_tokens": 200000,
              "capabilities": { "image_input": { "supported": true }, "thinking": { "types": { "adaptive": { "supported": false } } } } }
        ]});
        let models = parse_models(&body);
        assert_eq!(models[0].label(), "Claude Opus 5.5");
        assert_eq!(models[0].context, Some(1_000_000));
        assert_eq!(models[0].max_output, Some(128_000));
        assert_eq!(models[0].efforts, Effort::ALL.to_vec());
        assert!(models[0].vision && models[0].server_compaction);
        assert!(models[1].efforts.is_empty() && !models[1].server_compaction);
    }

    #[test]
    fn history_becomes_alternating_messages() {
        let call = ToolCall::new("toolu_1", "read_file", r#"{"path":"a.rs"}"#);
        let request = ChatRequest {
            model: "claude-opus-5-5".into(),
            messages: vec![
                Message::system("You are Quill."),
                Message::user_with_images("look", vec!["data:image/png;base64,AAAA".into()]),
                Message::assistant("Reading.", vec![call]),
                Message::tool("toolu_1", "Error: missing"),
                Message::user("try b.rs"),
                Message::assistant("", Vec::new()),
                Message::user("well?"),
            ],
            effort: Some(Effort::High),
            tools: vec![ToolSpec { name: "read_file".into(), description: "Read".into(), parameters: json!({ "type": "object" }) }],
            ..ChatRequest::default()
        };
        let body = body(&request, true);
        let messages = body["messages"].as_array().unwrap();
        let roles: Vec<&str> = messages.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, vec!["user", "assistant", "user"]);
        assert_eq!(messages[0]["content"][0]["source"]["media_type"], "image/png");
        assert_eq!(messages[1]["content"][1]["input"]["path"], "a.rs");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][0]["is_error"], true);
        assert_eq!(messages[2]["content"][2]["text"], "well?");
        assert_eq!(body["system"][0]["text"], "You are Quill.");
        assert_eq!(body["thinking"]["display"], "summarized");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["tools"][0]["eager_input_streaming"], true);
        assert!(body["tools"][0]["cache_control"].is_object());
    }

    #[test]
    fn native_blocks_are_replayed_unchanged() {
        let native = json!([
            { "type": "thinking", "thinking": "plan", "signature": "sig" },
            { "type": "text", "text": "" },
            { "type": "tool_use", "id": "t", "name": "grep", "input": { "pattern": "x" } }
        ]);
        let message = Message { native: Some(native), ..Message::assistant("", vec![ToolCall::new("t", "grep", "{}")]) };
        let blocks = assistant_blocks(&message);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["signature"], "sig");
        let only_thinking = Message {
            native: Some(json!([{ "type": "thinking", "thinking": "", "signature": "s" }])),
            ..Message::assistant("Done.", vec![])
        };
        assert_eq!(assistant_blocks(&only_thinking), vec![json!({ "type": "text", "text": "Done." })]);
    }

    #[test]
    fn the_stream_becomes_events_and_native_content() {
        let (events, native) = run(&[
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 10, "cache_read_input_tokens": 90 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "", "signature": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "Let me look." } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "abc" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Checking." } }),
            json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "tool_use", "id": "toolu_9", "name": "grep", "input": {} } }),
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "{\"pattern\":" } }),
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "\"x\"}" } }),
            json!({ "type": "content_block_stop", "index": 2 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 42 } }),
            json!({ "type": "message_stop" }),
        ]);
        assert_eq!(events[0], Event::Reasoning("Let me look.".into()));
        assert_eq!(events[1], Event::Content("Checking.".into()));
        assert_eq!(
            events[2],
            Event::ToolCall { index: 0, id: Some("toolu_9".into()), name: Some("grep".into()), arguments: String::new() }
        );
        assert!(events.contains(&Event::Usage(Tokens { prompt: 100, completion: 42, cache_read: 90, cache_write: 0 })));
        assert_eq!(events.last(), Some(&Event::Stop(StopReason::ToolUse)));
        let native = native.unwrap();
        assert_eq!(native[0]["signature"], "abc");
        assert_eq!(native[2]["input"]["pattern"], "x");
    }

    #[test]
    fn stream_errors_and_refusals_surface() {
        let mut blocks = Blocks::default();
        let error =
            blocks.apply(&json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }), &mut |_| {});
        assert_eq!(error, Err(Error("Overloaded".into())));
        assert_eq!(stop_reason("refusal"), StopReason::Refusal);
        assert!(supports_fallback("claude-opus-5-5") && !supports_fallback("claude-haiku-4-5"));
    }
}