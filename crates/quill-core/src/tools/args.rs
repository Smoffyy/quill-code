use serde_json::{Value, json};

pub const PATH: [&str; 6] = ["path", "file_path", "file", "filename", "filepath", "dir"];
pub const CONTENT: [&str; 6] = ["content", "contents", "text", "file_text", "code", "body"];
pub const OLD: [&str; 6] = ["old_text", "old_str", "old_string", "old", "search", "find"];
pub const NEW: [&str; 6] = ["new_text", "new_str", "new_string", "new", "replace", "replacement"];
pub const COMMAND: [&str; 3] = ["command", "cmd", "script"];
pub const PATTERN: [&str; 5] = ["pattern", "query", "regex", "text", "q"];
pub const GLOB: [&str; 3] = ["glob", "include", "file_pattern"];
pub const MESSAGE: [&str; 3] = ["message", "commit_message", "msg"];
pub const BRANCH: [&str; 3] = ["branch", "branch_name", "new_branch"];
pub const TITLE: [&str; 2] = ["title", "pr_title"];
pub const BODY: [&str; 3] = ["body", "description", "pr_body"];

const WRAPPERS: [&str; 4] = ["arguments", "args", "parameters", "input"];

fn first_value(text: &str) -> Option<Value> {
    serde_json::Deserializer::from_str(text).into_iter::<Value>().next()?.ok()
}

pub fn parse(arguments: &str) -> Value {
    let text = arguments.trim();
    if text.is_empty() {
        return json!({});
    }
    let mut value = match first_value(text) {
        Some(value) => value,
        None => return json!({ "__invalid": text }),
    };
    if let Some(inner) = value.as_str().and_then(first_value).filter(Value::is_object) {
        value = inner;
    }
    if let Some(object) = value.as_object()
        && object.len() == 1
        && let Some(inner) = WRAPPERS.iter().find_map(|key| object.get(*key)).filter(|inner| inner.is_object())
    {
        return inner.clone();
    }
    value
}

pub fn list(args: &Value, keys: &[&str]) -> Option<Vec<Value>> {
    keys.iter().find_map(|key| match args.get(*key)? {
        Value::Array(items) => Some(items.clone()),
        Value::String(text) => first_value(text.trim()).and_then(|value| value.as_array().cloned()),
        _ => None,
    })
}

pub fn invalid(args: &Value) -> Option<&str> {
    args.get("__invalid").and_then(Value::as_str)
}

pub fn text<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| args.get(*key).and_then(Value::as_str))
}

pub fn trimmed<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    text(args, keys).map(str::trim).filter(|value| !value.is_empty())
}

pub fn number(args: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        let value = args.get(*key)?;
        value.as_u64().or_else(|| value.as_str()?.trim().parse().ok())
    })
}

pub fn flag(args: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|key| args.get(*key).is_some_and(|v| v.as_bool().unwrap_or_else(|| v.as_str() == Some("true"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_arguments_are_kept_for_the_error() {
        assert_eq!(parse("{\"path\": \"a\"}")["path"], "a");
        assert!(invalid(&parse("{oops")).is_some());
        assert_eq!(parse(""), json!({}));
    }

    #[test]
    fn wrapped_and_encoded_arguments_are_unwrapped() {
        assert_eq!(parse("{\"path\": \"a\"}}")["path"], "a");
        assert_eq!(parse("{\"path\": \"a\"}</tool_call>")["path"], "a");
        assert_eq!(parse("\"{\\\"path\\\": \\\"a\\\"}\"")["path"], "a");
        assert_eq!(parse("{\"arguments\": {\"path\": \"a\"}}")["path"], "a");
        assert_eq!(parse("{\"input\": \"text\"}")["input"], "text");
        let args = json!({ "plan": "[{\"step\": \"a\"}]", "edits": [1], "content": "[1, 2]" });
        assert_eq!(list(&args, &["plan"]).unwrap().len(), 1);
        assert_eq!(list(&args, &["edits"]).unwrap(), vec![json!(1)]);
        assert!(list(&args, &["missing"]).is_none());
        assert_eq!(text(&args, &CONTENT), Some("[1, 2]"));
    }

    #[test]
    fn lenient_keys_and_values_are_read() {
        let args = json!({ "file_path": "a.rs", "start": "4", "replace_all": "true" });
        assert_eq!(text(&args, &PATH), Some("a.rs"));
        assert_eq!(number(&args, &["start_line", "start"]), Some(4));
        assert!(flag(&args, &["replace_all"]));
        assert!(!flag(&args, &["missing"]));
    }
}