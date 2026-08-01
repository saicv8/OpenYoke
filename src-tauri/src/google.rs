//! Google Gemini provider (Generative Language API).
//!
//! Gemini differs more than the others: turns live under `contents` with roles
//! `user`/`model` (not `assistant`), and text sits in `parts[].text`. We
//! translate our shared `{role, content}` messages into that shape.

use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::Channel;

use crate::ollama::ChatChunk;
use crate::sse::{self, Completion, Event};

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
) -> Result<Completion, String> {
    let mut body = json!({ "contents": to_contents(messages) });
    if !system.trim().is_empty() {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    let url = format!("{API}/models/{model}:streamGenerateContent?alt=sse&key={key}");
    let response = sse::streaming_client()?
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
    sse::pipe_sse(response, classify, channel).await
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

fn classify(value: &Value) -> Vec<Event> {
    if let Some(message) = value.pointer("/error/message").and_then(|m| m.as_str()) {
        return vec![Event::Failed(format!("Gemini: {message}"))];
    }
    let candidate = match value.get("candidates").and_then(|c| c.get(0)) {
        Some(candidate) => candidate,
        None => return vec![],
    };

    let mut events = Vec::new();
    let text: String = candidate
        .pointer("/content/parts")
        .and_then(|p| p.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect()
        })
        .unwrap_or_default();
    if !text.is_empty() {
        events.push(Event::Text(text));
    }
    // Gemini rides the finish reason on the same chunk as the final text, so
    // both have to come out of one payload.
    match candidate.get("finishReason").and_then(|r| r.as_str()) {
        Some("STOP") => events.push(Event::Done),
        Some("MAX_TOKENS") => events.push(Event::Cutoff(sse::OUTPUT_LIMIT.to_string())),
        Some(other) => events.push(Event::Cutoff(format!("Gemini stopped the reply ({other})"))),
        None => {}
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
    fn extracts_candidate_text() {
        let v = json!({ "candidates": [{ "content": { "parts": [{ "text": "Hi" }] } }] });
        assert_eq!(texts(&classify(&v)), vec!["Hi".to_string()]);
    }

    /// The last chunk carries text AND the terminal signal; losing either one
    /// costs a word of the answer or the ability to tell a clean end from a
    /// dropped connection.
    #[test]
    fn final_chunk_yields_text_and_done() {
        let v = json!({
            "candidates": [{ "content": { "parts": [{ "text": "bye" }] }, "finishReason": "STOP" }]
        });
        match &classify(&v)[..] {
            [Event::Text(text), Event::Done] => assert_eq!(text, "bye"),
            other => panic!("expected text + done, got {} event(s)", other.len()),
        }
    }

    #[test]
    fn max_tokens_is_a_cutoff() {
        let v = json!({ "candidates": [{ "finishReason": "MAX_TOKENS" }] });
        assert!(matches!(classify(&v)[..], [Event::Cutoff(_)]));
    }

    /// SAFETY, RECITATION, and friends all mean the answer is incomplete.
    #[test]
    fn other_finish_reasons_are_cutoffs_too() {
        let v = json!({ "candidates": [{ "finishReason": "SAFETY" }] });
        match &classify(&v)[..] {
            [Event::Cutoff(reason)] => assert!(reason.contains("SAFETY"), "{reason}"),
            other => panic!("expected a cutoff, got {} event(s)", other.len()),
        }
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
