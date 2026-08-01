//! Shared Server-Sent-Events reader for the cloud providers.
//!
//! OpenAI, Anthropic, and Gemini all stream chat responses as SSE (`data: {…}`
//! lines), differing only in how each payload is shaped. This reader handles
//! the transport; each provider passes a `classify` closure that maps one
//! parsed line onto the `Event`s below. Deltas are forwarded to the frontend
//! over the same `ChatChunk` channel used by Ollama.

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{json, Value};
use tauri::ipc::Channel;

use crate::ollama::ChatChunk;

/// How long a stream may go silent before we give up. Generous, because a
/// model that is still thinking sends nothing at all; bounded, because with no
/// timeout a half-open connection leaves the UI generating forever.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Client for streaming a chat. Deliberately has no *total* timeout — a long
/// reply legitimately takes minutes — but it does bound the gap between reads.
pub fn streaming_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .read_timeout(IDLE_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

/// What one parsed payload means to the reader.
///
/// The distinction that matters is `Done` vs. everything else: without an
/// explicit terminal event, a stream that simply stops — a dropped connection,
/// a proxy timeout, a mid-stream server error — is indistinguishable from a
/// finished answer, and we would save a half-written reply as if it were whole.
pub enum Event {
    /// A piece of the visible answer.
    Text(String),
    /// The stream may still end cleanly, but the answer is incomplete. Carries
    /// a user-facing reason.
    Cutoff(String),
    /// The provider's terminal event.
    Done,
    /// The provider reported an error mid-stream.
    Failed(String),
}

/// A finished generation. `cutoff` is `None` when the provider signalled a
/// clean end; otherwise it says why the text stops where it does.
pub struct Completion {
    pub text: String,
    pub cutoff: Option<String>,
}

/// The message shown when a stream just stops without saying why.
const DROPPED: &str = "the connection closed before the reply finished";

/// Reason text for hitting a per-reply output cap. Shared so every provider
/// words it the same way.
pub const OUTPUT_LIMIT: &str = "the model reached its output limit for one reply";

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

/// Close out a generation: tell the frontend it's over (and why, if it ended
/// early) and hand back the accumulated text. Shared with the Ollama reader,
/// which streams NDJSON but has the same "did it actually finish?" question.
pub fn finish(
    text: String,
    cutoff: Option<String>,
    channel: &Channel<ChatChunk>,
) -> Result<Completion, String> {
    channel
        .send(ChatChunk { content: String::new(), done: true, error: cutoff.clone() })
        .map_err(|e| e.to_string())?;
    Ok(Completion { text, cutoff })
}

/// Reason to report when a stream ends without a terminal event.
pub fn dropped_if_unfinished(finished: bool, cutoff: Option<String>) -> Option<String> {
    match cutoff {
        Some(reason) => Some(reason),
        None if !finished => Some(DROPPED.to_string()),
        None => None,
    }
}

pub async fn pipe_sse<F>(
    response: reqwest::Response,
    classify: F,
    channel: &Channel<ChatChunk>,
) -> Result<Completion, String>
where
    F: Fn(&Value) -> Vec<Event>,
{
    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut full = String::new();
    let mut cutoff: Option<String> = None;
    let mut finished = false;

    'read: while let Some(chunk) = stream.next().await {
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
                finished = true;
                break 'read;
            }
            let value = match serde_json::from_str::<Value>(data) {
                Ok(value) => value,
                Err(_) => continue,
            };

            for event in classify(&value) {
                match event {
                    Event::Text(delta) => {
                        if delta.is_empty() {
                            continue;
                        }
                        full.push_str(&delta);
                        channel.send(ChatChunk::text(delta)).map_err(|e| e.to_string())?;
                    }
                    Event::Cutoff(reason) => cutoff = Some(reason),
                    Event::Done => {
                        finished = true;
                        break 'read;
                    }
                    Event::Failed(message) => {
                        // Nothing salvageable: report it as a failure so no
                        // empty node is created. With text already streamed,
                        // keep it and flag where it broke off.
                        if full.is_empty() {
                            return Err(message);
                        }
                        cutoff = Some(message);
                        finished = true;
                        break 'read;
                    }
                }
            }
        }
    }

    finish(full, dropped_if_unfinished(finished, cutoff), channel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfinished_stream_reports_a_dropped_connection() {
        assert_eq!(dropped_if_unfinished(false, None).as_deref(), Some(DROPPED));
    }

    #[test]
    fn finished_stream_reports_nothing() {
        assert_eq!(dropped_if_unfinished(true, None), None);
    }

    /// A known reason always wins over the generic one.
    #[test]
    fn an_explicit_cutoff_survives_either_way() {
        let reason = Some(OUTPUT_LIMIT.to_string());
        assert_eq!(dropped_if_unfinished(true, reason.clone()).as_deref(), Some(OUTPUT_LIMIT));
        assert_eq!(dropped_if_unfinished(false, reason).as_deref(), Some(OUTPUT_LIMIT));
    }

    #[test]
    fn with_system_prepends_once_and_skips_when_blank() {
        let messages = vec![json!({ "role": "user", "content": "hi" })];
        let out = with_system("be nice", &messages);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(with_system("   ", &messages).len(), 1);
    }
}
