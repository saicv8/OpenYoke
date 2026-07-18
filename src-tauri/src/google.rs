//! Google Gemini provider (Generative Language API).
//!
//! Gemini differs more than the others: turns live under `contents` with roles
//! `user`/`model` (not `assistant`), and text sits in `parts[].text`. We
//! translate our shared `{role, content}` messages into that shape.

use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::Channel;

use crate::ollama::ChatChunk;
use crate::sse;

const API: &str = "https://generativelanguage.googleapis.com/v1beta";

pub fn fallback() -> Vec<String> {
    vec!["gemini-2.0-flash".into(), "gemini-1.5-pro".into()]
}

pub async fn list_models(key: &str) -> Result<Vec<String>, String> {
    let response = reqwest::Client::new()
        .get(format!("{API}/models?key={key}"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Gemini models list returned {}", response.status()));
    }
    let data: Value = response.json().await.map_err(|e| e.to_string())?;
    let ids = data
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|m| supports_generate(m))
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                .map(|n| n.trim_start_matches("models/").to_string())
                .collect()
        })
        .unwrap_or_default();
    Ok(ids)
}

fn supports_generate(model: &Value) -> bool {
    model
        .get("supportedGenerationMethods")
        .and_then(|s| s.as_array())
        .map(|methods| methods.iter().any(|x| x.as_str() == Some("generateContent")))
        .unwrap_or(false)
}

pub async fn chat_stream(
    key: &str,
    model: &str,
    system: &str,
    messages: &[Value],
    channel: &Channel<ChatChunk>,
) -> Result<String, String> {
    let mut body = json!({ "contents": to_contents(messages) });
    if !system.trim().is_empty() {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    let url = format!("{API}/models/{model}:streamGenerateContent?alt=sse&key={key}");
    let response = reqwest::Client::new()
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Gemini error ({status}): {text}"));
    }
    sse::pipe_sse(response, delta, channel).await
}

/// Map `{role: user|assistant, content}` -> Gemini `{role: user|model, parts}`.
fn to_contents(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            let role = match m.get("role").and_then(|r| r.as_str()) {
                Some("assistant") => "model",
                _ => "user",
            };
            let text = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
            json!({ "role": role, "parts": [{ "text": text }] })
        })
        .collect()
}

fn delta(value: &Value) -> Option<String> {
    let parts = value
        .get("candidates")?
        .get(0)?
        .get("content")?
        .get("parts")?
        .as_array()?;
    let text: String = parts
        .iter()
        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
        .collect();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_candidate_text() {
        let v = json!({ "candidates": [{ "content": { "parts": [{ "text": "Hi" }] } }] });
        assert_eq!(delta(&v).as_deref(), Some("Hi"));
    }

    #[test]
    fn maps_roles_to_gemini() {
        let messages = vec![
            json!({ "role": "user", "content": "q" }),
            json!({ "role": "assistant", "content": "a" }),
        ];
        let contents = to_contents(&messages);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model"); // assistant -> model
        assert_eq!(contents[0]["parts"][0]["text"], "q");
    }
}
