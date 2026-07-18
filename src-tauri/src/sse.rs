//! Shared Server-Sent-Events reader for the cloud providers.
//!
//! OpenAI, Anthropic, and Gemini all stream chat responses as SSE (`data: {…}`
//! lines), differing only in how the text delta is nested in each JSON payload.
//! This reader handles the transport; each provider passes an `extract` closure
//! that pulls its delta out of a parsed line. Deltas are forwarded to the
//! frontend over the same `ChatChunk` channel used by Ollama.

use futures_util::StreamExt;
use serde_json::{json, Value};
use tauri::ipc::Channel;

use crate::ollama::ChatChunk;

/// Prepend a system message to a `{role, content}` list (for providers that
/// take the system prompt inline, i.e. OpenAI and Ollama). No-op if empty.
pub fn with_system(system: &str, messages: &[Value]) -> Vec<Value> {
    if system.trim().is_empty() {
        return messages.to_vec();
    }
    let mut out = Vec::with_capacity(messages.len() + 1);
    out.push(json!({ "role": "system", "content": system }));
    out.extend_from_slice(messages);
    out
}

pub async fn pipe_sse<F>(
    response: reqwest::Response,
    extract: F,
    channel: &Channel<ChatChunk>,
) -> Result<String, String>
where
    F: Fn(&Value) -> Option<String>,
{
    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut full = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        buffer.extend_from_slice(&chunk);

        while let Some(newline) = buffer.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buffer.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim();

            let data = match line.strip_prefix("data:") {
                Some(d) => d.trim(),
                None => continue, // ignore `event:` / comments / blank lines
            };
            if data == "[DONE]" {
                let _ = channel.send(ChatChunk { content: String::new(), done: true });
                return Ok(full);
            }
            if let Ok(value) = serde_json::from_str::<Value>(data) {
                if let Some(delta) = extract(&value) {
                    if !delta.is_empty() {
                        full.push_str(&delta);
                        channel
                            .send(ChatChunk { content: delta, done: false })
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
        }
    }

    channel
        .send(ChatChunk { content: String::new(), done: true })
        .map_err(|e| e.to_string())?;
    Ok(full)
}
