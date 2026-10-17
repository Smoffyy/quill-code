use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::{ChatRequest, Error, Event, Model, OpenAi, Provider, Status};

pub struct LmStudio {
    server: OpenAi,
}

impl LmStudio {
    pub fn new(base_url: &str, key: Option<&str>) -> LmStudio {
        LmStudio { server: OpenAi::new(base_url).with_key(key) }
    }
}

impl Provider for LmStudio {
    fn models(&self) -> Result<Vec<Model>, Error> {
        match self.server.get_json("/api/v1/models") {
            Ok(body) if body["models"].is_array() => Ok(parse_catalog(&body)),
            _ => self.server.models(),
        }
    }

    fn stream(&self, request: &ChatRequest, cancel: &AtomicBool, on_event: &mut dyn FnMut(Event)) -> Result<(), Error> {
        self.server.stream(request, cancel, on_event)
    }
}

fn parse_catalog(body: &Value) -> Vec<Model> {
    let Some(list) = body["models"].as_array() else { return Vec::new() };
    list.iter()
        .filter(|entry| entry["type"] == "llm")
        .filter_map(|entry| {
            let id = entry["key"].as_str()?.to_string();
            let loaded = entry["loaded_instances"].as_array().and_then(|instances| instances.first());
            Some(Model {
                name: entry["display_name"].as_str().map(str::to_string),
                status: if loaded.is_some() { Status::Loaded } else { Status::Unloaded },
                context: loaded.and_then(|instance| instance["config"]["context_length"].as_u64()).or(entry["max_context_length"].as_u64()),
                vision: entry["capabilities"]["vision"].as_bool().unwrap_or(false),
                ..Model::new(id)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_catalog_keeps_language_models_with_their_loaded_window() {
        let body = json!({ "models": [
            { "type": "llm", "key": "google/gemma-4-26b-a4b", "display_name": "Gemma 4 26B A4B",
              "loaded_instances": [{ "id": "google/gemma-4-26b-a4b", "config": { "context_length": 32768 } }],
              "max_context_length": 262144, "capabilities": { "vision": true, "trained_for_tool_use": true } },
            { "type": "llm", "key": "qwen/qwen3-8b", "loaded_instances": [], "max_context_length": 40960 },
            { "type": "embedding", "key": "nomic-embed", "loaded_instances": [], "max_context_length": 2048 }
        ]});
        let models = parse_catalog(&body);
        assert_eq!(models.len(), 2);
        assert_eq!((models[0].label(), models[0].context, models[0].vision), ("Gemma 4 26B A4B", Some(32768), true));
        assert_eq!((models[1].status, models[1].context), (Status::Unloaded, Some(40960)));
    }
}