use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
        self.wire().serialize(serializer)
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

    pub fn wire(&self) -> Value {
        let mut value = json!({ "role": self.role });
        value["content"] = if self.images.is_empty() {
            json!(self.content)
        } else {
            let mut parts = vec![json!({ "type": "text", "text": self.content })];
            parts.extend(self.images.iter().map(|url| json!({ "type": "image_url", "image_url": { "url": url } })));
            json!(parts)
        };
        if !self.tool_calls.is_empty() {
            value["tool_calls"] = json!(self.tool_calls);
        }
        if let Some(id) = &self.tool_call_id {
            value["tool_call_id"] = json!(id);
        }
        value
    }
}
#[cfg(test)]
mod tests {
    use super::*;

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
}