mod contracts;
mod network;

use crate::*;

fn input(protocol: Protocol, endpoint: &str) -> ConfigurationInput {
    ConfigurationInput {
        protocol,
        endpoint: endpoint.into(),
        model: "fixture-model".into(),
    }
}

fn configured(protocol: Protocol) -> PreparedProvider {
    let endpoint = protocol
        .default_endpoint()
        .unwrap_or("https://fixture.invalid/v1/chat/completions");
    let config = Configuration::validate(input(protocol, endpoint)).unwrap();
    let key = matches!(protocol, Protocol::Openai | Protocol::Anthropic)
        .then(|| SecretKey::new("fixture-secret-never-real".into()).unwrap());
    PreparedProvider::new(config, key).unwrap()
}

fn chat(text: &str) -> serde_json::Value {
    serde_json::json!({ "choices": [{ "index": 0, "message": { "role": "assistant", "content": text, "refusal": null }, "finish_reason": "stop" }] })
}

fn ollama(text: &str) -> serde_json::Value {
    serde_json::json!({ "message": { "role": "assistant", "content": text }, "done": true, "done_reason": "stop" })
}

fn anthropic(text: &str) -> serde_json::Value {
    serde_json::json!({ "type": "message", "role": "assistant", "content": [{ "type": "text", "text": text }], "stop_reason": "end_turn" })
}
