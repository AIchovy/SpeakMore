use crate::{
    config::{BudgetField, InstructionRole, PreparedProvider, Protocol},
    Failure, Result,
};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

pub(crate) fn request(
    provider: &PreparedProvider,
    raw: &str,
    instructions: &str,
    max_output: u32,
) -> Result<(HeaderMap, Vec<u8>)> {
    let config = &provider.config;
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    let body = match config.protocol {
        Protocol::Anthropic => {
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
            json!({
                "model": config.model, "system": instructions,
                "messages": [{ "role": "user", "content": raw }],
                "max_tokens": max_output, "stream": false
            })
        }
        Protocol::Ollama => json!({
            "model": config.model,
            "messages": [{ "role": "system", "content": instructions }, { "role": "user", "content": raw }],
            "options": { "num_predict": max_output }, "stream": false
        }),
        Protocol::Openai | Protocol::OpenaiCompatible { .. } => {
            let (budget, role) = match config.protocol {
                Protocol::OpenaiCompatible {
                    budget_field,
                    instruction_role,
                } => (budget_field, instruction_role),
                _ => (BudgetField::MaxCompletionTokens, InstructionRole::Developer),
            };
            let mut value = json!({
                "model": config.model,
                "messages": [
                    { "role": match role { InstructionRole::System => "system", InstructionRole::Developer => "developer" }, "content": instructions },
                    { "role": "user", "content": raw }
                ],
                "stream": false
            });
            let field = match budget {
                BudgetField::MaxCompletionTokens => "max_completion_tokens",
                BudgetField::MaxTokens => "max_tokens",
            };
            value[field] = json!(max_output);
            if config.protocol == Protocol::Openai {
                value["store"] = json!(false);
            }
            // DeepSeek defaults to reasoning; this text-only adapter requests a direct answer.
            if matches!(config.protocol, Protocol::OpenaiCompatible { .. })
                && config.endpoint.host_str() == Some("api.deepseek.com")
            {
                value["thinking"] = json!({"type":"disabled"});
            }
            value
        }
    };
    if let Some(key) = &provider.key {
        let (name, value) = if config.protocol == Protocol::Anthropic {
            ("x-api-key", HeaderValue::from_str(key.expose()))
        } else {
            let bearer = Zeroizing::new(format!("Bearer {}", key.expose()));
            (AUTHORIZATION.as_str(), HeaderValue::from_str(&bearer))
        };
        let mut value = value.map_err(|_| Failure::InvalidCredential)?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    // No credentials enter the serializable JSON value.
    let bytes = serde_json::to_vec(&body).map_err(|_| Failure::InvalidText)?;
    Ok((headers, bytes))
}

pub(crate) fn response(protocol: Protocol, body: &[u8]) -> Result<String> {
    // Typed deserialization rejects duplicate critical keys and invalid field types.
    // Unknown provider metadata is ignored; content must match a text-only contract.
    match protocol {
        Protocol::Openai | Protocol::OpenaiCompatible { .. } => {
            let data: ChatResponse = parse(body)?;
            if data.error.is_some() || data.choices.len() != 1 || data.choices[0].index != 0 {
                return Err(Failure::UnsupportedResponse);
            }
            let choice = data
                .choices
                .into_iter()
                .next()
                .ok_or(Failure::UnsupportedResponse)?;
            if choice.finish_reason == "content_filter" {
                return Err(Failure::RefusedResponse);
            }
            if choice.finish_reason != "stop" {
                return Err(Failure::IncompleteResponse);
            }
            choice.message.text()
        }
        Protocol::Anthropic => {
            let data: AnthropicResponse = parse(body)?;
            if data.error.is_some() {
                return Err(Failure::UnsupportedResponse);
            }
            if data.stop_reason == "refusal"
                || data
                    .stop_details
                    .as_ref()
                    .is_some_and(|details| details.kind == "refusal")
            {
                return Err(Failure::RefusedResponse);
            }
            if data.stop_details.is_some() || data.stop_sequence.is_some() {
                return Err(Failure::UnsupportedResponse);
            }
            // We never send stop_sequences, so stop_sequence is not complete.
            if data.stop_reason != "end_turn" {
                return Err(Failure::IncompleteResponse);
            }
            if data.role != "assistant" || data.kind != "message" {
                return Err(Failure::UnsupportedResponse);
            }
            let mut result = String::new();
            for block in data.content {
                if block.kind != "text" {
                    return Err(Failure::UnsupportedResponse);
                }
                result.push_str(block.text.as_deref().ok_or(Failure::UnsupportedResponse)?);
            }
            Ok(result)
        }
        Protocol::Ollama => {
            let data: OllamaResponse = parse(body)?;
            if data.error.is_some() {
                return Err(Failure::UnsupportedResponse);
            }
            if !data.done || data.done_reason != "stop" {
                return Err(Failure::IncompleteResponse);
            }
            data.message.text()
        }
    }
}

fn parse<T: for<'a> Deserialize<'a>>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|_| Failure::UnsupportedResponse)
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    error: Option<Value>,
}

#[derive(Deserialize)]
struct Choice {
    index: u32,
    finish_reason: String,
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    content: Option<String>,
    refusal: Option<Value>,
    tool_calls: Option<Value>,
    function_call: Option<Value>,
    audio: Option<Value>,
    images: Option<Value>,
}

impl Message {
    fn text(self) -> Result<String> {
        if self.refusal.is_some() {
            return Err(Failure::RefusedResponse);
        }
        if self.role != "assistant"
            || self.function_call.is_some()
            || self.audio.is_some()
            || self
                .images
                .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
            || self
                .tool_calls
                .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
        {
            return Err(Failure::UnsupportedResponse);
        }
        self.content.ok_or(Failure::UnsupportedResponse)
    }
}

#[derive(Deserialize)]
struct AnthropicResponse {
    #[serde(rename = "type")]
    kind: String,
    role: String,
    stop_reason: String,
    content: Vec<TextBlock>,
    error: Option<Value>,
    stop_details: Option<StopDetails>,
    stop_sequence: Option<String>,
}

#[derive(Deserialize)]
struct StopDetails {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct TextBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: Message,
    done: bool,
    done_reason: String,
    error: Option<Value>,
}
