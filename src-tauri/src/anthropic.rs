//! Anthropic (Claude) provider — the Messages API.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::Channel;

use crate::ollama::ChatChunk;
use crate::sse::{self, Completion, Event};

const API: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2023-06-01";

// --- Output sizing -----------------------------------------------------------
//
// `max_tokens` is required, and it caps EVERYTHING the model emits for one
// reply — on the current Claude models that includes the reasoning they do
// before answering, which is on by default and invisible to us (we only render
// `text` deltas). A cap sized for the visible answer alone gets partly eaten by
// that reasoning, and the reply stops mid-sentence with `stop_reason:
// max_tokens`. That was the bug: a fixed 8192 truncated long answers, silently,
// and the half-written text was saved as if it were complete.
//
// So we ask the model what its real ceiling is and use that.

/// Used only when the models endpoint can't tell us — the old fixed value.
const FALLBACK_MAX_TOKENS: u64 = 8192;

pub fn fallback() -> Vec<String> {
    vec![
        "claude-opus-5".into(),
        "claude-sonnet-5".into(),
        "claude-haiku-4-5".into(),
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

fn max_tokens_cache() -> &'static Mutex<HashMap<String, u64>> {
    static CACHE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `max_tokens` from a `/models/{id}` payload — the model's own output ceiling.
pub fn parse_max_tokens(model: &Value) -> Option<u64> {
    model.get("max_tokens").and_then(|m| m.as_u64())
}

/// The model's output ceiling, asked once per model and cached. Streaming is
/// what makes it safe to ask for the whole thing: a cap this large would risk
/// an HTTP timeout on a non-streamed request, and `max_tokens` is a limit
/// rather than a reservation, so nothing is spent by raising it.
async fn max_tokens(key: &str, model: &str) -> u64 {
    if let Some(hit) = max_tokens_cache().lock().ok().and_then(|c| c.get(model).copied()) {
        return hit;
    }
    let found = async {
        let response = reqwest::Client::new()
            .get(format!("{API}/models/{model}"))
            .header("x-api-key", key)
            .header("anthropic-version", VERSION)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?;
        parse_max_tokens(&response.json::<Value>().await.ok()?)
    }
    .await
    .unwrap_or(FALLBACK_MAX_TOKENS);

    if let Ok(mut cache) = max_tokens_cache().lock() {
        cache.insert(model.to_string(), found);
    }
    found
}

pub async fn chat_stream(
    key: &str,
    model: &str,
    system: &str,
    messages: &[Value],
    channel: &Channel<ChatChunk>,
) -> Result<Completion, String> {
    // The system prompt is a top-level field, not a message. Our tree already
    // yields strictly alternating user/assistant turns ending on the new user
    // question, which is what the Messages API expects.
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens(key, model).await,
        "messages": messages,
        "stream": true,
    });
    if !system.trim().is_empty() {
        body["system"] = json!(system);
    }
    let response = sse::streaming_client()?
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
    sse::pipe_sse(response, classify, channel).await
}

fn classify(value: &Value) -> Vec<Event> {
    match value.get("type").and_then(|t| t.as_str()) {
        // Only `text` deltas are part of the answer. Reasoning arrives as
        // `thinking_delta` on the same event type and is deliberately dropped.
        Some("content_block_delta") => match value.pointer("/delta/text").and_then(|t| t.as_str()) {
            Some(text) => vec![Event::Text(text.to_string())],
            None => vec![],
        },
        // Carries the stop reason. Anything other than a natural end means the
        // text we have is not the whole answer.
        Some("message_delta") => match value.pointer("/delta/stop_reason").and_then(|r| r.as_str())
        {
            Some("max_tokens") => vec![Event::Cutoff(sse::OUTPUT_LIMIT.to_string())],
            Some("refusal") => vec![Event::Cutoff("the model declined to continue".to_string())],
            _ => vec![],
        },
        Some("message_stop") => vec![Event::Done],
        // Overload and API errors arrive mid-stream, then the connection ends.
        // Without this they read as a normal finish.
        Some("error") => vec![Event::Failed(format!(
            "Anthropic: {}",
            value
                .pointer("/error/message")
                .and_then(|m| m.as_str())
                .unwrap_or("the stream failed")
        ))],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn extracts_text_delta() {
        let v = json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": "Hi" } });
        assert_eq!(texts(&classify(&v)), vec!["Hi".to_string()]);
    }

    /// Reasoning shares the `content_block_delta` type but is not the answer.
    #[test]
    fn ignores_thinking_delta() {
        let v = json!({ "type": "content_block_delta", "delta": { "type": "thinking_delta", "thinking": "hmm" } });
        assert!(classify(&v).is_empty());
    }

    #[test]
    fn message_stop_ends_the_stream() {
        assert!(matches!(classify(&json!({ "type": "message_stop" }))[..], [Event::Done]));
    }

    /// The regression this exists to catch: the model ran out of room, and the
    /// answer we have breaks off mid-sentence.
    #[test]
    fn max_tokens_stop_reason_is_a_cutoff() {
        let v = json!({ "type": "message_delta", "delta": { "stop_reason": "max_tokens" } });
        match &classify(&v)[..] {
            [Event::Cutoff(reason)] => assert_eq!(reason, sse::OUTPUT_LIMIT),
            other => panic!("expected a cutoff, got {} event(s)", other.len()),
        }
    }

    #[test]
    fn a_natural_end_is_not_a_cutoff() {
        let v = json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } });
        assert!(classify(&v).is_empty());
    }

    /// A mid-stream error used to be indistinguishable from a finished reply.
    #[test]
    fn surfaces_a_mid_stream_error() {
        let v = json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } });
        match &classify(&v)[..] {
            [Event::Failed(message)] => assert!(message.contains("Overloaded"), "{message}"),
            other => panic!("expected a failure, got {} event(s)", other.len()),
        }
    }

    #[test]
    fn ignores_non_content_events() {
        assert!(classify(&json!({ "type": "message_start" })).is_empty());
        assert!(classify(&json!({ "type": "ping" })).is_empty());
    }

    #[test]
    fn parse_max_tokens_reads_the_model_ceiling() {
        assert_eq!(parse_max_tokens(&json!({ "max_tokens": 128_000 })), Some(128_000));
        assert_eq!(parse_max_tokens(&json!({})), None);
    }
}
