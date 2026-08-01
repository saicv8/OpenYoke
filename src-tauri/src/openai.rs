//! OpenAI-compatible provider.
//!
//! Covers OpenAI itself and any API that speaks the same `/chat/completions`
//! dialect (OpenRouter, Groq, Together, DeepSeek, LM Studio, …) via a
//! configurable base URL.

use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::Channel;

use crate::ollama::ChatChunk;
use crate::sse::{self, Completion, Event};

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
) -> Result<Completion, String> {
    let url = format!("{}/chat/completions", base(base_url));
    // No `max_tokens`: the default is the model's own ceiling, which is what we
    // want. Pinning a number here would cap long answers for no benefit.
    let body = json!({
        "model": model,
        "messages": sse::with_system(system, messages),
        "stream": true,
    });
    let response = sse::streaming_client()?
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
    sse::pipe_sse(response, classify, channel).await
}

fn classify(value: &Value) -> Vec<Event> {
    if let Some(message) = value.pointer("/error/message").and_then(|m| m.as_str()) {
        return vec![Event::Failed(format!("OpenAI: {message}"))];
    }
    let choice = match value.get("choices").and_then(|c| c.get(0)) {
        Some(choice) => choice,
        None => return vec![],
    };

    let mut events = Vec::new();
    if let Some(text) = choice.pointer("/delta/content").and_then(|c| c.as_str()) {
        events.push(Event::Text(text.to_string()));
    }
    // Read alongside the text rather than instead of it: some providers put the
    // reason on the same chunk as the last delta. Treating a natural stop as
    // terminal also covers the compatible servers that never send `[DONE]` —
    // otherwise every reply from those would be reported as cut off.
    match choice.get("finish_reason").and_then(|r| r.as_str()) {
        Some("stop") | Some("tool_calls") => events.push(Event::Done),
        Some("length") => events.push(Event::Cutoff(sse::OUTPUT_LIMIT.to_string())),
        Some("content_filter") => {
            events.push(Event::Cutoff("a content filter stopped the reply".to_string()))
        }
        _ => {}
    }
    events
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
    fn extracts_delta_content() {
        let v = json!({ "choices": [{ "delta": { "content": "Hello" } }] });
        assert_eq!(texts(&classify(&v)), vec!["Hello".to_string()]);
    }

    #[test]
    fn no_delta_when_absent() {
        assert!(classify(&json!({ "choices": [{ "delta": {} }] })).is_empty());
    }

    #[test]
    fn length_finish_reason_is_a_cutoff() {
        let v = json!({ "choices": [{ "delta": {}, "finish_reason": "length" }] });
        assert!(matches!(classify(&v)[..], [Event::Cutoff(_)]));
    }

    /// A provider that packs the reason onto the final content chunk must not
    /// cost us either the text or the warning.
    #[test]
    fn keeps_text_and_cutoff_from_one_chunk() {
        let v = json!({ "choices": [{ "delta": { "content": "end" }, "finish_reason": "length" }] });
        match &classify(&v)[..] {
            [Event::Text(text), Event::Cutoff(_)] => assert_eq!(text, "end"),
            other => panic!("expected text + cutoff, got {} event(s)", other.len()),
        }
    }

    /// A natural stop ends the stream rather than flagging it — compatible
    /// servers that skip `[DONE]` would otherwise look like dropped
    /// connections on every single reply.
    #[test]
    fn a_natural_stop_ends_the_stream_cleanly() {
        let v = json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] });
        assert!(matches!(classify(&v)[..], [Event::Done]));
    }

    #[test]
    fn surfaces_a_mid_stream_error() {
        let v = json!({ "error": { "message": "server had an error" } });
        assert!(matches!(classify(&v)[..], [Event::Failed(_)]));
    }

    #[test]
    fn filters_non_chat_models() {
        assert!(is_chat_model("gpt-4o"));
        assert!(!is_chat_model("text-embedding-3-large"));
        assert!(!is_chat_model("whisper-1"));
    }
}
