//! OpenAI-compatible provider.
//!
//! Covers OpenAI itself and any API that speaks the same `/chat/completions`
//! dialect (OpenRouter, Groq, Together, DeepSeek, LM Studio, …) via a
//! configurable base URL.

use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::Channel;

use crate::ollama::ChatChunk;
use crate::sse;

fn base(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        "https://api.openai.com/v1".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Minimal fallback if `/models` can't be listed (e.g. a provider that doesn't
/// implement it). Users can still chat with these.
pub fn fallback() -> Vec<String> {
    vec!["gpt-4o".into(), "gpt-4o-mini".into()]
}

/// Drop obvious non-chat endpoints from OpenAI's large model list.
fn is_chat_model(id: &str) -> bool {
    const DENY: [&str; 9] = [
        "embed", "whisper", "tts", "dall-e", "moderation", "audio", "image", "transcribe", "realtime",
    ];
    let lower = id.to_lowercase();
    !DENY.iter().any(|d| lower.contains(d))
}

pub async fn list_models(base_url: &str, key: &str) -> Result<Vec<String>, String> {
    let url = format!("{}/models", base(base_url));
    let response = reqwest::Client::new()
        .get(&url)
        .bearer_auth(key)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("OpenAI models list returned {}", response.status()));
    }
    let data: Value = response.json().await.map_err(|e| e.to_string())?;
    let mut ids: Vec<String> = data
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                .filter(|id| is_chat_model(id))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    Ok(ids)
}

pub async fn chat_stream(
    base_url: &str,
    key: &str,
    model: &str,
    system: &str,
    messages: &[Value],
    channel: &Channel<ChatChunk>,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", base(base_url));
    let body = json!({
        "model": model,
        "messages": crate::sse::with_system(system, messages),
        "stream": true,
    });
    let response = reqwest::Client::new()
        .post(&url)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("OpenAI error ({status}): {text}"));
    }
    sse::pipe_sse(response, delta, channel).await
}

fn delta(value: &Value) -> Option<String> {
    value
        .get("choices")?
        .get(0)?
        .get("delta")?
        .get("content")?
        .as_str()
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_delta_content() {
        let v = json!({ "choices": [{ "delta": { "content": "Hello" } }] });
        assert_eq!(delta(&v).as_deref(), Some("Hello"));
    }

    #[test]
    fn no_delta_when_absent() {
        assert!(delta(&json!({ "choices": [{ "delta": {} }] })).is_none());
    }

    #[test]
    fn filters_non_chat_models() {
        assert!(is_chat_model("gpt-4o"));
        assert!(!is_chat_model("text-embedding-3-large"));
        assert!(!is_chat_model("whisper-1"));
    }
}
