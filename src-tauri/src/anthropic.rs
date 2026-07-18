//! Anthropic (Claude) provider — the Messages API.

use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::Channel;

use crate::ollama::ChatChunk;
use crate::sse;

const API: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2023-06-01";

pub fn fallback() -> Vec<String> {
    vec![
        "claude-sonnet-4-6".into(),
        "claude-opus-4-8".into(),
        "claude-haiku-4-5-20251001".into(),
    ]
}

pub async fn list_models(key: &str) -> Result<Vec<String>, String> {
    let response = reqwest::Client::new()
        .get(format!("{API}/models"))
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Anthropic models list returned {}", response.status()));
    }
    let data: Value = response.json().await.map_err(|e| e.to_string())?;
    let ids = data
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok(ids)
}

pub async fn chat_stream(
    key: &str,
    model: &str,
    system: &str,
    messages: &[Value],
    channel: &Channel<ChatChunk>,
) -> Result<String, String> {
    // Anthropic requires max_tokens (a generous cap here so long, detailed
    // answers aren't truncated). The system prompt is a top-level field, not a
    // message. Our tree already yields strictly alternating user/assistant turns
    // ending on the new user question, which is what the Messages API expects.
    let mut body = json!({
        "model": model,
        "max_tokens": 8192,
        "messages": messages,
        "stream": true,
    });
    if !system.trim().is_empty() {
        body["system"] = json!(system);
    }
    let response = reqwest::Client::new()
        .post(format!("{API}/messages"))
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Anthropic error ({status}): {text}"));
    }
    sse::pipe_sse(response, delta, channel).await
}

fn delta(value: &Value) -> Option<String> {
    if value.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
        return value
            .get("delta")?
            .get("text")?
            .as_str()
            .map(String::from);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_text_delta() {
        let v = json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": "Hi" } });
        assert_eq!(delta(&v).as_deref(), Some("Hi"));
    }

    #[test]
    fn ignores_non_content_events() {
        assert!(delta(&json!({ "type": "message_start" })).is_none());
        assert!(delta(&json!({ "type": "message_stop" })).is_none());
    }
}
