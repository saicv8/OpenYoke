//! All communication with the Ollama HTTP API lives here.
//!
//! This module is the single seam between OpenYoke and its model backend. When
//! a second backend (an embedded engine, a cloud endpoint) is added, promote
//! these free functions to a `ModelProvider` trait with one impl per backend —
//! the call sites in `main.rs` won't need to change shape.

use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::ipc::Channel;

fn normalize(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string()
}

/// Streamed progress for a model download, delivered to the frontend over a
/// Tauri channel. `serde` camelCases the fields to match JS conventions.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub status: String,
    pub total: Option<u64>,
    pub completed: Option<u64>,
    pub done: bool,
    pub error: Option<String>,
}

/// One streamed piece of an assistant reply, delivered to the frontend over a
/// Tauri channel so the UI can render tokens as they arrive.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatChunk {
    pub content: String,
    pub done: bool,
}

// --- Pure helpers (unit-tested below) ---------------------------------------

/// Shape Ollama's `/api/tags` payload into `{ name, size }` entries.
pub fn parse_models(payload: &Value) -> Value {
    let models: Vec<Value> = payload
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .map(|item| json!({ "name": item.get("name"), "size": item.get("size") }))
                .collect()
        })
        .unwrap_or_default();
    json!({ "models": models })
}

/// Pull the assistant text out of Ollama's `/api/chat` response.
pub fn extract_chat_content(data: &Value) -> String {
    data.get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string()
}

/// Convert one streamed NDJSON line from `/api/pull` into a `PullProgress`.
fn to_progress(value: &Value) -> PullProgress {
    let status = value
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let error = value
        .get("error")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    PullProgress {
        done: status == "success",
        total: value.get("total").and_then(|t| t.as_u64()),
        completed: value.get("completed").and_then(|c| c.as_u64()),
        error,
        status,
    }
}

// --- Backend calls ----------------------------------------------------------

pub async fn list_models(base_url: &str) -> Result<Value, String> {
    let url = format!("{}/api/tags", normalize(base_url));
    match reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => {
            let payload: Value = response.json().await.map_err(|e| e.to_string())?;
            Ok(parse_models(&payload))
        }
        Err(_) => Ok(json!({
            "models": [],
            "message": "Ollama is not reachable. Start it locally to load models.",
        })),
    }
}

/// Multi-turn chat, STREAMED. `messages` is a caller-assembled array of
/// `{ "role", "content" }` objects (built in `tree::build_chat_messages` so the
/// context-isolation invariant is enforced in the backend). Each NDJSON line
/// from Ollama carries a token delta, which is forwarded over `channel` and
/// accumulated. Returns the full assistant text once the stream completes.
///
/// No total timeout: a long reply may legitimately take minutes to generate.
pub async fn chat_stream(
    base_url: &str,
    model: &str,
    system: &str,
    messages: &[Value],
    channel: &Channel<ChatChunk>,
) -> Result<String, String> {
    let url = format!("{}/api/chat", normalize(base_url));
    let payload = json!({
        "model": model,
        "messages": crate::sse::with_system(system, messages),
        "stream": true,
    });

    let response = reqwest::Client::new()
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Chat failed ({status}): {text}"));
    }

    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut full = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        buffer.extend_from_slice(&chunk);

        // Ollama streams one JSON object per line.
        while let Some(newline) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=newline).collect();
            let trimmed = &line[..line.len() - 1];
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_slice::<Value>(trimmed) {
                if let Some(error) = value.get("error").and_then(|e| e.as_str()) {
                    return Err(error.to_string());
                }
                let delta = extract_chat_content(&value); // reuse: reads message.content
                if !delta.is_empty() {
                    full.push_str(&delta);
                    channel
                        .send(ChatChunk { content: delta, done: false })
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }

    // Flush a trailing line that wasn't newline-terminated.
    if !buffer.is_empty() {
        if let Ok(value) = serde_json::from_slice::<Value>(&buffer) {
            let delta = extract_chat_content(&value);
            if !delta.is_empty() {
                full.push_str(&delta);
                let _ = channel.send(ChatChunk { content: delta, done: false });
            }
        }
    }

    channel
        .send(ChatChunk { content: String::new(), done: true })
        .map_err(|e| e.to_string())?;
    Ok(full)
}

pub async fn delete_model(base_url: &str, model: &str) -> Result<Value, String> {
    let url = format!("{}/api/delete", normalize(base_url));
    let response = reqwest::Client::new()
        .delete(&url)
        .json(&json!({ "model": model }))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.status().is_success() {
        Ok(json!({ "ok": true }))
    } else {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        Err(format!("Delete failed ({status}): {text}"))
    }
}

/// Download a model, streaming NDJSON progress lines to `channel` as they
/// arrive. Ollama can send gigabytes over minutes, so we never buffer the whole
/// body — we parse line-by-line off the byte stream.
pub async fn pull_model(
    base_url: &str,
    model: &str,
    channel: Channel<PullProgress>,
) -> Result<(), String> {
    let url = format!("{}/api/pull", normalize(base_url));
    let response = reqwest::Client::new()
        .post(&url)
        .json(&json!({ "model": model, "stream": true }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Pull failed ({status}): {text}"));
    }

    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        buffer.extend_from_slice(&chunk);

        // Emit every complete NDJSON line; keep any partial tail in the buffer.
        while let Some(newline) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=newline).collect();
            let trimmed = &line[..line.len() - 1];
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_slice::<Value>(trimmed) {
                let progress = to_progress(&value);
                let error = progress.error.clone();
                let done = progress.done;
                channel.send(progress).map_err(|e| e.to_string())?;
                if let Some(error) = error {
                    return Err(error);
                }
                if done {
                    return Ok(());
                }
            }
        }
    }

    // Flush a trailing line that wasn't newline-terminated.
    if !buffer.is_empty() {
        if let Ok(value) = serde_json::from_slice::<Value>(&buffer) {
            let progress = to_progress(&value);
            let error = progress.error.clone();
            channel.send(progress).map_err(|e| e.to_string())?;
            if let Some(error) = error {
                return Err(error);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_models_maps_name_and_size() {
        let payload = json!({ "models": [{ "name": "llama3.2", "size": 2019393792_u64 }] });
        assert_eq!(
            parse_models(&payload),
            json!({ "models": [{ "name": "llama3.2", "size": 2019393792_u64 }] })
        );
    }

    #[test]
    fn parse_models_handles_missing_key() {
        assert_eq!(parse_models(&json!({})), json!({ "models": [] }));
    }

    #[test]
    fn extract_chat_content_reads_message() {
        let data = json!({ "message": { "role": "assistant", "content": "hi" } });
        assert_eq!(extract_chat_content(&data), "hi");
    }

    #[test]
    fn extract_chat_content_defaults_to_empty() {
        assert_eq!(extract_chat_content(&json!({})), "");
    }

    #[test]
    fn to_progress_marks_success_done() {
        let p = to_progress(&json!({ "status": "success" }));
        assert!(p.done);
        assert!(p.error.is_none());
    }

    #[test]
    fn to_progress_reads_byte_counts() {
        let p = to_progress(&json!({
            "status": "downloading", "total": 100, "completed": 40
        }));
        assert!(!p.done);
        assert_eq!(p.total, Some(100));
        assert_eq!(p.completed, Some(40));
    }

    #[test]
    fn to_progress_surfaces_error() {
        let p = to_progress(&json!({ "error": "file does not exist" }));
        assert_eq!(p.error.as_deref(), Some("file does not exist"));
    }
}
