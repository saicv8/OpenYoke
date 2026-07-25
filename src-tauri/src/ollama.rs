//! All communication with the Ollama HTTP API lives here.
//!
//! This module is the single seam between OpenYoke and its model backend. When
//! a second backend (an embedded engine, a cloud endpoint) is added, promote
//! these free functions to a `ModelProvider` trait with one impl per backend —
//! the call sites in `main.rs` won't need to change shape.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::ipc::Channel;

fn normalize(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string()
}

// --- Context window sizing ---------------------------------------------------
//
// Ollama defaults to a small context and SILENTLY DISCARDS whatever doesn't fit
// — from the front of the prompt, which is exactly where the conversation
// history lives. Measured against a live instance: an ~8,000-token prompt was
// processed as 2,050 tokens by default, and as 8,027 with `num_ctx` set. Left
// unset, a long thread (or any answer with web results injected) loses its
// earlier turns and the model appears to forget the conversation.
//
// So we size `num_ctx` to the prompt on every call, clamped to what the model
// actually supports.

/// Never ask for less than this, so short chats still have room to grow.
const MIN_CTX: u64 = 4096;
/// Ceiling used when a model's true maximum can't be determined.
const FALLBACK_MAX_CTX: u64 = 32_768;
/// Room reserved for the reply on top of the prompt estimate.
const RESPONSE_HEADROOM: u64 = 2048;

/// Conservative chars-per-token ratio. Real English averages ~4; we use 3 so
/// the estimate errs high — overshooting costs a little memory, undershooting
/// silently truncates the conversation, which is the bug being fixed.
const CHARS_PER_TOKEN: u64 = 3;

fn max_ctx_cache() -> &'static Mutex<HashMap<String, u64>> {
    static CACHE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Pull `<arch>.context_length` out of an `/api/show` payload. The key is
/// prefixed with the architecture (`llama.`, `qwen2.`, …), so match on the
/// suffix rather than enumerating architectures.
pub fn parse_max_context(show: &Value) -> Option<u64> {
    show.get("model_info")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.ends_with(".context_length"))
        .and_then(|(_, v)| v.as_u64())
}

/// The model's maximum context, asked once per model and cached. Falls back to
/// `FALLBACK_MAX_CTX` when Ollama can't tell us.
async fn max_context(base_url: &str, model: &str) -> u64 {
    if let Some(hit) = max_ctx_cache().lock().ok().and_then(|c| c.get(model).copied()) {
        return hit;
    }
    let url = format!("{}/api/show", normalize(base_url));
    let found = async {
        let response = reqwest::Client::new()
            .post(&url)
            .json(&json!({ "model": model }))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?;
        parse_max_context(&response.json::<Value>().await.ok()?)
    }
    .await
    .unwrap_or(FALLBACK_MAX_CTX);

    if let Ok(mut cache) = max_ctx_cache().lock() {
        cache.insert(model.to_string(), found);
    }
    found
}

/// Estimated prompt size in tokens, from the assembled messages plus system.
fn estimate_tokens(system: &str, messages: &[Value]) -> u64 {
    let content: usize = messages
        .iter()
        .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
        .map(str::len)
        .sum();
    // A few tokens of chat-template scaffolding wrap every message.
    let overhead = messages.len() as u64 * 8;
    (content + system.len()) as u64 / CHARS_PER_TOKEN + overhead
}

/// Choose `num_ctx` for this request.
///
/// Rounded up to a power of two on purpose: Ollama reallocates the KV cache
/// whenever the requested size changes, so snapping to a few discrete sizes
/// keeps consecutive turns on the same allocation instead of paying that cost
/// on every message.
pub fn choose_num_ctx(system: &str, messages: &[Value], model_max: u64) -> u64 {
    let needed = estimate_tokens(system, messages) + RESPONSE_HEADROOM;
    let ceiling = model_max.max(MIN_CTX);
    needed.next_power_of_two().clamp(MIN_CTX, ceiling)
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
    let num_ctx = choose_num_ctx(system, messages, max_context(base_url, model).await);
    let payload = json!({
        "model": model,
        "messages": crate::sse::with_system(system, messages),
        "stream": true,
        // Without this Ollama truncates the prompt and the thread loses its
        // history. See the context-window notes at the top of this module.
        "options": { "num_ctx": num_ctx },
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

    fn msgs(contents: &[&str]) -> Vec<Value> {
        contents.iter().map(|c| json!({ "role": "user", "content": c })).collect()
    }

    #[test]
    fn choose_num_ctx_floors_at_min() {
        assert_eq!(choose_num_ctx("sys", &msgs(&["hi"]), 131_072), MIN_CTX);
    }

    #[test]
    fn choose_num_ctx_grows_with_the_prompt() {
        // ~60k chars of web context is ~20k tokens; the window must cover it
        // rather than letting Ollama silently drop the conversation history.
        let big = "x".repeat(60_000);
        let chosen = choose_num_ctx("sys", &msgs(&[&big]), 131_072);
        assert!(chosen >= 20_000 + RESPONSE_HEADROOM, "chose only {chosen}");
        assert!(chosen.is_power_of_two(), "{chosen} is not a power of two");
    }

    /// Never request more than the model can do — Ollama would either error or
    /// waste the allocation.
    #[test]
    fn choose_num_ctx_clamps_to_model_maximum() {
        let big = "x".repeat(200_000);
        assert_eq!(choose_num_ctx("sys", &msgs(&[&big]), 8192), 8192);
    }

    /// A model reporting a tiny maximum must not push us below the floor.
    #[test]
    fn choose_num_ctx_min_wins_over_small_maximum() {
        assert_eq!(choose_num_ctx("sys", &msgs(&["hi"]), 512), MIN_CTX);
    }

    #[test]
    fn choose_num_ctx_counts_the_whole_thread_not_just_the_last_turn() {
        let turn = "y".repeat(5_000);
        let one = choose_num_ctx("sys", &msgs(&[&turn]), 131_072);
        let many = choose_num_ctx("sys", &msgs(&[&turn, &turn, &turn, &turn, &turn]), 131_072);
        assert!(many > one, "history ignored: {many} vs {one}");
    }

    #[test]
    fn parse_max_context_reads_any_architecture_prefix() {
        let show = json!({ "model_info": { "llama.context_length": 131_072 } });
        assert_eq!(parse_max_context(&show), Some(131_072));
        let show = json!({ "model_info": { "qwen2.context_length": 32_768 } });
        assert_eq!(parse_max_context(&show), Some(32_768));
    }

    #[test]
    fn parse_max_context_absent_is_none() {
        assert_eq!(parse_max_context(&json!({ "model_info": {} })), None);
        assert_eq!(parse_max_context(&json!({})), None);
    }

    /// The regression this sizing exists to prevent: with a large block of web
    /// context ahead of the question, the model must still see the earlier
    /// turns. Asserts on `prompt_eval_count` — the count Ollama reports for
    /// what it ACTUALLY processed, which is how the truncation was found.
    /// Run with `cargo test -- --ignored` (needs Ollama + llama3.2:1b).
    #[tokio::test]
    #[ignore = "requires a running Ollama with llama3.2:1b"]
    async fn num_ctx_prevents_prompt_truncation() {
        let base = "http://127.0.0.1:11434";
        let model = "llama3.2:1b";
        let system = "You are helpful.";
        // ~60k chars of retrieved page text, as the aggressive web caps allow.
        let web = "Lorem ipsum dolor sit amet. ".repeat(2_200);
        let messages = vec![
            json!({ "role": "user", "content": "I am building a bridge out of bamboo in Kerala." }),
            json!({ "role": "assistant", "content": "A bamboo bridge in Kerala sounds great!" }),
            json!({ "role": "user", "content": format!("{web}\n\nWhat material am I using?") }),
        ];

        let chosen = choose_num_ctx(system, &messages, max_context(base, model).await);
        let sent = crate::sse::with_system(system, &messages);

        // How many prompt tokens Ollama actually processed, with and without
        // our sizing. Comparing the two is the proof; a fixed threshold would
        // just encode this machine's tokenizer.
        async fn processed(base: &str, model: &str, sent: &[Value], num_ctx: Option<u64>) -> u64 {
            let mut options = json!({ "num_predict": 1 });
            if let Some(n) = num_ctx {
                options["num_ctx"] = json!(n);
            }
            let response = reqwest::Client::new()
                .post(format!("{base}/api/chat"))
                .json(&json!({ "model": model, "messages": sent, "stream": false, "options": options }))
                .timeout(Duration::from_secs(300))
                .send()
                .await
                .expect("ollama reachable");
            response.json::<Value>().await.expect("json body")["prompt_eval_count"]
                .as_u64()
                .expect("prompt_eval_count")
        }

        let with_sizing = processed(base, model, &sent, Some(chosen)).await;
        let default = processed(base, model, &sent, None).await;

        assert!(
            with_sizing > default * 4,
            "sizing barely helped: {with_sizing} vs {default} tokens processed"
        );
        // The window must be able to hold the whole prompt, or the front of it
        // (the conversation history) is silently discarded.
        assert!(
            chosen >= with_sizing,
            "window {chosen} is smaller than the {with_sizing}-token prompt"
        );
    }

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
