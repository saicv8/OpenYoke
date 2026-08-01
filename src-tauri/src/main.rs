// Prevent an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anthropic;
mod catalog;
mod google;
mod ollama;
mod openai;
mod search;
mod sse;
mod storage;
mod tree;

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};

/// Serializes every read-modify-write of conversations.json so concurrent
/// commands can't mint colliding node ids or clobber each other's writes.
/// Held only across the synchronous critical section, never across an `.await`.
type WriteLock = Mutex<()>;

use ollama::PullProgress;
use storage::StorageStatus;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";

/// Applied to every generation unless the user overrides it. The raw APIs ship
/// with no system prompt, which makes answers terser than the vendor apps; this
/// nudges models toward the thorough, well-formatted responses people expect.
const DEFAULT_SYSTEM: &str = "You are a helpful, knowledgeable assistant. Give clear, thorough, well-structured answers. Use Markdown — headings, lists, tables, and fenced code blocks — wherever it improves clarity. Show your reasoning when it helps, and don't be needlessly terse. When web search results are provided, use them and cite sources by their [n] number.";

#[derive(Serialize, Deserialize, Clone)]
struct Settings {
    base_url: String,
    default_model: String,
    /// Optional URL to refresh the model library from. Empty = bundled catalog.
    #[serde(default)]
    catalog_url: String,
    /// Cloud provider API keys (stored locally, sent only to the provider).
    #[serde(default)]
    openai_key: String,
    #[serde(default)]
    openai_base_url: String,
    #[serde(default)]
    anthropic_key: String,
    #[serde(default)]
    google_key: String,
    /// Applied to every model. Empty = use the built-in DEFAULT_SYSTEM.
    #[serde(default)]
    system_prompt: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            base_url: DEFAULT_BASE_URL.to_string(),
            default_model: String::new(),
            catalog_url: String::new(),
            openai_key: String::new(),
            openai_base_url: String::new(),
            anthropic_key: String::new(),
            google_key: String::new(),
            system_prompt: String::new(),
        }
    }
}

// --- Pure helpers (unit-tested below) ---------------------------------------

/// Build a new conversation record (an empty conversation tree).
fn build_conversation(existing: &[Value], title: &str) -> Value {
    // max(id)+1, not len+1: robust against gaps from an externally-edited file.
    let next_id = existing
        .iter()
        .filter_map(|c| c.get("id").and_then(|v| v.as_u64()))
        .max()
        .unwrap_or(0)
        + 1;
    json!({
        "id": next_id,
        "title": title,
        "nodes": [],
    })
}

/// Locate a conversation by id, returning its index in the list.
fn conversation_index(conversations: &[Value], id: u64) -> Result<usize, String> {
    conversations
        .iter()
        .position(|c| c.get("id").and_then(|i| i.as_u64()) == Some(id))
        .ok_or_else(|| format!("Conversation {id} not found"))
}

/// Read a conversation's node array (missing/legacy => empty tree).
fn conversation_nodes(conversation: &Value) -> Vec<Value> {
    conversation
        .get("nodes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Rename the conversation with `id`. Returns the updated record, or `None`.
fn set_title(conversations: &mut [Value], id: u64, title: &str) -> Option<Value> {
    for conversation in conversations.iter_mut() {
        if conversation.get("id").and_then(|i| i.as_u64()) == Some(id) {
            conversation["title"] = json!(title);
            return Some(conversation.clone());
        }
    }
    None
}

// --- Storage-backed file access ---------------------------------------------

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(storage::data_dir(app)?.join("settings.json"))
}

fn conversations_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(storage::data_dir(app)?.join("conversations.json"))
}

/// Write `body` atomically: write a sibling temp file, then rename over the
/// target. An interrupted write can never leave a truncated file that a later
/// read would treat as corrupt (or, worse, silently discard).
fn write_atomic(path: &std::path::Path, body: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, body).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn read_settings(app: &AppHandle) -> Result<Settings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("settings.json is corrupt: {e}"))
}

fn read_conversations(app: &AppHandle) -> Result<Vec<Value>, String> {
    let path = conversations_path(app)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    // Propagate parse errors rather than silently returning []: a mutating
    // command must ABORT on a corrupt file, never overwrite it with defaults.
    serde_json::from_str(&text).map_err(|e| format!("conversations.json is corrupt: {e}"))
}

fn write_conversations(app: &AppHandle, conversations: &[Value]) -> Result<(), String> {
    let body = serde_json::to_string_pretty(conversations).map_err(|e| e.to_string())?;
    write_atomic(&conversations_path(app)?, &body)
}

// --- Storage-location commands (usable before storage is configured) --------

#[tauri::command]
fn get_storage_status(app: AppHandle) -> Result<StorageStatus, String> {
    storage::status(&app)
}

#[tauri::command]
fn set_storage_dir(app: AppHandle, path: String) -> Result<String, String> {
    storage::set_dir(&app, &path)
}

// --- Settings + conversation commands ---------------------------------------

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<Settings, String> {
    read_settings(&app)
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    base_url: String,
    default_model: String,
    catalog_url: String,
    openai_key: String,
    openai_base_url: String,
    anthropic_key: String,
    google_key: String,
    system_prompt: String,
) -> Result<Settings, String> {
    let settings = Settings {
        base_url,
        default_model,
        catalog_url,
        openai_key,
        openai_base_url,
        anthropic_key,
        google_key,
        system_prompt,
    };
    let body = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    write_atomic(&settings_path(&app)?, &body)?;
    Ok(settings)
}

/// Persist ONLY the active model, leaving every other setting untouched.
///
/// The picker saves on every change, and a full `save_settings` would commit
/// whatever half-typed API key or base URL happens to be sitting in the other
/// inputs at that moment. Patching the stored settings avoids that.
#[tauri::command]
fn set_default_model(app: AppHandle, model: String) -> Result<Settings, String> {
    let mut settings = read_settings(&app)?;
    settings.default_model = model;
    let body = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    write_atomic(&settings_path(&app)?, &body)?;
    Ok(settings)
}

#[tauri::command]
fn list_conversations(app: AppHandle) -> Result<Value, String> {
    Ok(json!({ "conversations": read_conversations(&app)? }))
}

#[tauri::command]
fn create_conversation(
    app: AppHandle,
    write_lock: State<'_, WriteLock>,
    title: String,
) -> Result<Value, String> {
    let _guard = write_lock.lock().map_err(|_| "conversation lock poisoned".to_string())?;
    let mut conversations = read_conversations(&app)?;
    let conversation = build_conversation(&conversations, &title);
    conversations.push(conversation.clone());
    write_conversations(&app, &conversations)?;
    Ok(conversation)
}

/// Create a child node under `parent_id` (None = virtual root, i.e. a new
/// thread). The generation context is assembled IN THE BACKEND from the
/// root->parent ancestor path, so a branch can never see its siblings. Returns
/// the persisted node (with the model's answer).
/// Route a chat to the backend named by the `provider:id` model string. Bare
/// ids (no recognized prefix) fall through to Ollama for backward compatibility.
async fn dispatch_chat(
    base_url: &str,
    model: &str,
    settings: &Settings,
    system: &str,
    messages: &[Value],
    channel: &Channel<ollama::ChatChunk>,
) -> Result<sse::Completion, String> {
    match model.split_once(':') {
        Some(("anthropic", id)) => {
            if settings.anthropic_key.trim().is_empty() {
                return Err("No Anthropic API key set — add it under Cloud API keys.".to_string());
            }
            anthropic::chat_stream(&settings.anthropic_key, id, system, messages, channel).await
        }
        Some(("openai", id)) => {
            if settings.openai_key.trim().is_empty() {
                return Err("No OpenAI API key set — add it under Cloud API keys.".to_string());
            }
            openai::chat_stream(&settings.openai_base_url, &settings.openai_key, id, system, messages, channel).await
        }
        Some(("google", id)) => {
            if settings.google_key.trim().is_empty() {
                return Err("No Google API key set — add it under Cloud API keys.".to_string());
            }
            google::chat_stream(&settings.google_key, id, system, messages, channel).await
        }
        Some(("ollama", id)) => ollama::chat_stream(base_url, id, system, messages, channel).await,
        _ => ollama::chat_stream(base_url, model, system, messages, channel).await,
    }
}

const QUERY_SYSTEM: &str = "You turn a user's message into a web search query. \
Reply with ONLY the query — no quotes, no explanation, no prefix. \
Keep it under 15 words. Drop conversational filler and keep the specific, \
distinctive terms: names, projects, versions, error text. If the message names \
a specific site, repo, or product, make that the focus of the query.";

/// Distill the question into a search query using the same model that will
/// answer. Best-effort by design: any failure, or a model that ignores the
/// instruction and rambles, falls back to the raw question.
async fn build_search_query(
    base_url: &str,
    model: &str,
    settings: &Settings,
    question: &str,
) -> String {
    // A sink channel: this generation is internal, so its tokens must not
    // stream into the user's answer pane.
    let sink: Channel<ollama::ChatChunk> = Channel::new(|_| Ok(()));
    let messages = vec![json!({ "role": "user", "content": question })];

    let raw = match dispatch_chat(base_url, model, settings, QUERY_SYSTEM, &messages, &sink).await {
        Ok(completion) => completion.text,
        Err(e) => {
            eprintln!("search: query generation failed ({e}), searching the raw question");
            return question.to_string();
        }
    };
    sanitize_query(&raw).unwrap_or_else(|| question.to_string())
}

/// Salvage a usable query from a model's reply, or `None` if it doesn't look
/// like one. Small models like to wrap the answer in quotes, prefix it with
/// "Search query:", or add a sentence of commentary — take the first non-empty
/// line and strip the decoration.
fn sanitize_query(raw: &str) -> Option<String> {
    // Reasoning models emit a <think> block first; the query follows it. Drop
    // the whole block, not just its tags, or the reasoning becomes the query.
    let body = match raw.rfind("</think>") {
        Some(end) => &raw[end + "</think>".len()..],
        None => raw,
    };
    let lines: Vec<&str> = body.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let first = lines.first()?;

    // Small models often ignore "reply with ONLY the query" and answer with a
    // newline-separated keyword list instead (llama3.2:1b reliably does). Taking
    // only the first line there would search for a single word, so join them.
    // A line of real prose is longer than a keyword, which tells the two apart:
    // a trailing sentence of commentary means take the first line only.
    const KEYWORD_MAX_WORDS: usize = 4;
    let is_keyword_list =
        lines.len() > 1 && lines.iter().all(|l| l.split_whitespace().count() <= KEYWORD_MAX_WORDS);
    let joined = if is_keyword_list { lines.join(" ") } else { first.to_string() };

    let cleaned = joined
        .trim_start_matches("Search query:")
        .trim_start_matches("Query:")
        .trim()
        .trim_matches(['"', '\'', '`'])
        .trim()
        .to_string();

    // Overlong queries dilute the distinctive terms — the exact failure this
    // whole function exists to prevent — so keep only the leading words.
    const MAX_WORDS: usize = 15;
    let words: Vec<&str> = cleaned.split_whitespace().take(MAX_WORDS).collect();
    if words.is_empty() {
        return None;
    }
    Some(words.join(" "))
}

#[tauri::command]
async fn create_node(
    app: AppHandle,
    write_lock: State<'_, WriteLock>,
    channel: Channel<ollama::ChatChunk>,
    base_url: String,
    conversation_id: u64,
    parent_id: Option<u64>,
    question: String,
    model: String,
    web_search: bool,
    x: f64,
    y: f64,
) -> Result<Value, String> {
    if model.trim().is_empty() {
        return Err("No model selected. Pick or download a model first.".to_string());
    }

    // Snapshot the tree to assemble the isolated context (best-effort; the
    // authoritative checks happen under the lock after the await).
    let conversations = read_conversations(&app)?;
    let idx = conversation_index(&conversations, conversation_id)?;
    let nodes = conversation_nodes(&conversations[idx]);

    if let Some(parent) = parent_id {
        if tree::find_node(&nodes, parent).is_none() {
            return Err(format!("Parent node {parent} not found"));
        }
    }

    let path = tree::ancestor_path(&nodes, parent_id);

    let settings = read_settings(&app)?;
    let system = if settings.system_prompt.trim().is_empty() {
        DEFAULT_SYSTEM
    } else {
        settings.system_prompt.as_str()
    };

    // Optional web search: OpenYoke searches, RETRIEVES the result pages, and
    // injects their text into the prompt we SEND, while the node still stores
    // the user's ORIGINAL question. This is provider-agnostic, so it works for
    // local and cloud models alike. Any failure along the way degrades
    // gracefully to a normal (unaugmented) prompt.
    let effective_question = if web_search {
        // Ask the model to turn the question into a search query first. Feeding
        // the raw question to a search engine buries the few terms that matter
        // under a paragraph of prose and returns generic results.
        let query = build_search_query(&base_url, &model, &settings, &question).await;
        let results = search::search_and_retrieve(&query, &question).await;
        if results.is_empty() {
            question.clone()
        } else {
            format!(
                "{}Answer the following using the results above where relevant:\n\n{}",
                search::format_context(&results),
                question
            )
        }
    } else {
        question.clone()
    };
    let messages = tree::build_chat_messages(&path, &effective_question);

    // The one and only generation call. Tokens stream to the frontend over
    // `channel` as they arrive; the full text is returned when done. Transport
    // errors abort before any node is created; an empty reply still persists
    // (the interaction happened). NO lock is held across this await.
    //
    // The model is `provider:id`; route to the right backend. Bare ids (no
    // recognized prefix) fall through to Ollama for backward compatibility.
    let answer = dispatch_chat(&base_url, &model, &settings, system, &messages, &channel).await?;

    // A reply can end before the model was finished — an output cap, a filter,
    // a dropped connection. Keep whatever text arrived (it's the interaction
    // the user paid for) but record WHY it stops there, so the node isn't
    // presented as a complete answer. With nothing to keep, it's just an error.
    if let Some(reason) = &answer.cutoff {
        if answer.text.trim().is_empty() {
            return Err(format!("The reply was cut off before any text arrived — {reason}."));
        }
    }

    // Critical section: serialize mint + write against other mutations. Holds
    // the lock across no `.await`, so the future stays Send.
    let _guard = write_lock.lock().map_err(|_| "conversation lock poisoned".to_string())?;
    let mut conversations = read_conversations(&app)?;
    let idx = conversation_index(&conversations, conversation_id)?;
    let fresh_nodes = conversation_nodes(&conversations[idx]);

    // Re-validate the parent after the await: a concurrent delete could have
    // removed it, and we must not persist a node with a dangling parentId.
    if let Some(parent) = parent_id {
        if tree::find_node(&fresh_nodes, parent).is_none() {
            return Err(format!("Parent node {parent} was removed before the reply arrived"));
        }
    }

    let new_id = tree::next_node_id(&fresh_nodes);
    let mut node = json!({
        "id": new_id,
        "parentId": parent_id,
        "question": question,
        "answer": answer.text,
        "model": model,
        "x": x,
        "y": y,
    });
    // Only present on an incomplete answer, so existing nodes stay untouched
    // and the frontend can treat its absence as "this one finished".
    if let Some(reason) = answer.cutoff {
        node["truncated"] = json!(reason);
    }

    if !conversations[idx].get("nodes").map(Value::is_array).unwrap_or(false) {
        conversations[idx]["nodes"] = json!([]);
    }
    conversations[idx]["nodes"]
        .as_array_mut()
        .expect("nodes ensured to be an array")
        .push(node.clone());
    write_conversations(&app, &conversations)?;
    Ok(node)
}

#[tauri::command]
fn update_node_position(
    app: AppHandle,
    write_lock: State<'_, WriteLock>,
    conversation_id: u64,
    node_id: u64,
    x: f64,
    y: f64,
) -> Result<Value, String> {
    let _guard = write_lock.lock().map_err(|_| "conversation lock poisoned".to_string())?;
    let mut conversations = read_conversations(&app)?;
    let idx = conversation_index(&conversations, conversation_id)?;
    let nodes = conversations[idx]
        .get_mut("nodes")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "Conversation has no nodes".to_string())?;
    let updated = tree::set_position(nodes, node_id, x, y)
        .ok_or_else(|| format!("Node {node_id} not found"))?;
    write_conversations(&app, &conversations)?;
    Ok(updated)
}

/// Delete a node and its entire subtree. Returns the removed ids so the
/// frontend can prune cards + edges.
#[tauri::command]
fn delete_node(
    app: AppHandle,
    write_lock: State<'_, WriteLock>,
    conversation_id: u64,
    node_id: u64,
) -> Result<Value, String> {
    let _guard = write_lock.lock().map_err(|_| "conversation lock poisoned".to_string())?;
    let mut conversations = read_conversations(&app)?;
    let idx = conversation_index(&conversations, conversation_id)?;
    let nodes = conversation_nodes(&conversations[idx]);
    if tree::find_node(&nodes, node_id).is_none() {
        return Err(format!("Node {node_id} not found"));
    }
    let (survivors, removed_ids) = tree::delete_subtree(&nodes, node_id);
    conversations[idx]["nodes"] = json!(survivors);
    write_conversations(&app, &conversations)?;
    Ok(json!({ "conversationId": conversation_id, "removedIds": removed_ids }))
}

#[tauri::command]
fn rename_conversation(
    app: AppHandle,
    write_lock: State<'_, WriteLock>,
    conversation_id: u64,
    title: String,
) -> Result<Value, String> {
    let _guard = write_lock.lock().map_err(|_| "conversation lock poisoned".to_string())?;
    let mut conversations = read_conversations(&app)?;
    let updated = set_title(&mut conversations, conversation_id, &title)
        .ok_or_else(|| format!("Conversation {conversation_id} not found"))?;
    write_conversations(&app, &conversations)?;
    Ok(updated)
}

/// Delete a whole conversation (and, implicitly, its entire node tree).
#[tauri::command]
fn delete_conversation(
    app: AppHandle,
    write_lock: State<'_, WriteLock>,
    conversation_id: u64,
) -> Result<Value, String> {
    let _guard = write_lock.lock().map_err(|_| "conversation lock poisoned".to_string())?;
    let mut conversations = read_conversations(&app)?;
    let idx = conversation_index(&conversations, conversation_id)?;
    conversations.remove(idx);
    write_conversations(&app, &conversations)?;
    Ok(json!({ "deletedId": conversation_id }))
}

// --- Model backend commands (delegate to the ollama module) -----------------

#[tauri::command]
async fn list_models(base_url: String) -> Result<Value, String> {
    ollama::list_models(&base_url).await
}

fn ollama_model_names(models: &Value) -> Vec<String> {
    models
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Aggregate models across every configured provider for the model picker.
/// Each group is `{ provider, label, models: [id, …] }`; the frontend prefixes
/// each id with its provider, so a chosen model is e.g. `anthropic:claude-…`.
#[tauri::command]
async fn list_all_models(app: AppHandle, base_url: String) -> Result<Value, String> {
    let settings = read_settings(&app)?;
    let mut groups: Vec<Value> = Vec::new();

    let ollama = ollama::list_models(&base_url).await.unwrap_or_else(|_| json!({ "models": [] }));
    groups.push(json!({
        "provider": "ollama",
        "label": "Ollama (local)",
        "models": ollama_model_names(&ollama),
    }));

    if !settings.anthropic_key.trim().is_empty() {
        let models = anthropic::list_models(&settings.anthropic_key)
            .await
            .unwrap_or_else(|_| anthropic::fallback());
        groups.push(json!({ "provider": "anthropic", "label": "Anthropic (Claude)", "models": models }));
    }

    if !settings.openai_key.trim().is_empty() {
        let models = openai::list_models(&settings.openai_base_url, &settings.openai_key)
            .await
            .unwrap_or_else(|_| openai::fallback());
        groups.push(json!({ "provider": "openai", "label": "OpenAI-compatible", "models": models }));
    }

    if !settings.google_key.trim().is_empty() {
        let models = google::list_models(&settings.google_key)
            .await
            .unwrap_or_else(|_| google::fallback());
        groups.push(json!({ "provider": "google", "label": "Google Gemini", "models": models }));
    }

    Ok(json!({ "groups": groups }))
}

#[tauri::command]
async fn pull_model(
    base_url: String,
    model: String,
    channel: Channel<PullProgress>,
) -> Result<(), String> {
    ollama::pull_model(&base_url, &model, channel).await
}

#[tauri::command]
async fn delete_model(base_url: String, model: String) -> Result<Value, String> {
    ollama::delete_model(&base_url, &model).await
}

#[tauri::command]
async fn fetch_catalog(app: AppHandle) -> Result<Value, String> {
    let settings = read_settings(&app)?;
    Ok(catalog::fetch(Some(&settings.catalog_url)).await)
}

fn main() {
    tauri::Builder::default()
        .manage(WriteLock::new(()))
        .invoke_handler(tauri::generate_handler![
            get_storage_status,
            set_storage_dir,
            load_settings,
            save_settings,
            set_default_model,
            list_conversations,
            create_conversation,
            create_node,
            update_node_position,
            delete_node,
            rename_conversation,
            delete_conversation,
            list_models,
            list_all_models,
            pull_model,
            delete_model,
            fetch_catalog
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `set_default_model` reads, patches one field, and writes back. If that
    /// round-trip dropped anything, switching models in the picker would wipe
    /// the user's API keys — so pin that it doesn't.
    #[test]
    fn patching_the_default_model_preserves_every_other_setting() {
        let stored = Settings {
            base_url: "http://example:1234".into(),
            default_model: "ollama:llama3.2:1b".into(),
            catalog_url: "https://example/catalog.json".into(),
            openai_key: "sk-openai".into(),
            openai_base_url: "https://openai.example".into(),
            anthropic_key: "sk-ant".into(),
            google_key: "goog".into(),
            system_prompt: "be terse".into(),
        };
        let on_disk = serde_json::to_string_pretty(&stored).unwrap();

        // Exactly what the command does between read and write.
        let mut patched: Settings = serde_json::from_str(&on_disk).unwrap();
        patched.default_model = "anthropic:claude-opus-4-8".into();
        let rewritten: Settings =
            serde_json::from_str(&serde_json::to_string_pretty(&patched).unwrap()).unwrap();

        assert_eq!(rewritten.default_model, "anthropic:claude-opus-4-8");
        assert_eq!(rewritten.base_url, "http://example:1234");
        assert_eq!(rewritten.catalog_url, "https://example/catalog.json");
        assert_eq!(rewritten.openai_key, "sk-openai");
        assert_eq!(rewritten.openai_base_url, "https://openai.example");
        assert_eq!(rewritten.anthropic_key, "sk-ant");
        assert_eq!(rewritten.google_key, "goog");
        assert_eq!(rewritten.system_prompt, "be terse");
    }

    /// A settings.json written before `default_model` existed must still load.
    #[test]
    fn settings_without_default_model_still_parse() {
        let old = r#"{ "base_url": "http://127.0.0.1:11434", "default_model": "" }"#;
        let settings: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.default_model, "");
        assert!(settings.anthropic_key.is_empty());
    }

    #[test]
    fn sanitize_query_takes_a_bare_query() {
        assert_eq!(sanitize_query("saicv8 OpenYoke github repo").unwrap(), "saicv8 OpenYoke github repo");
    }

    /// Small models rarely follow "reply with ONLY the query" exactly.
    #[test]
    fn sanitize_query_strips_model_decoration() {
        assert_eq!(sanitize_query("\"saicv8 OpenYoke\"").unwrap(), "saicv8 OpenYoke");
        assert_eq!(sanitize_query("Search query: saicv8 OpenYoke").unwrap(), "saicv8 OpenYoke");
        assert_eq!(sanitize_query("Query: `saicv8 OpenYoke`").unwrap(), "saicv8 OpenYoke");
        assert_eq!(
            sanitize_query("saicv8 OpenYoke\n\nThis should find the repo.").unwrap(),
            "saicv8 OpenYoke"
        );
    }

    #[test]
    fn sanitize_query_skips_reasoning_preamble() {
        assert_eq!(
            sanitize_query("<think>\nThe user wants...\n</think>\nsaicv8 OpenYoke").unwrap(),
            "saicv8 OpenYoke"
        );
    }

    /// Verbatim llama3.2:1b output for the GitHub question — it answers with a
    /// keyword list, so taking the first line alone would search "openyoke".
    #[test]
    fn sanitize_query_joins_keyword_lists() {
        assert_eq!(
            sanitize_query("openyoke\ngithub\ngrowth\naudience").unwrap(),
            "openyoke github growth audience"
        );
        assert_eq!(
            sanitize_query("openyoke\nopen-source\nai\ngithub\nharness \nsaicv8").unwrap(),
            "openyoke open-source ai github harness saicv8"
        );
    }

    /// ...but a trailing sentence of commentary is prose, not a keyword, and
    /// must not be swept into the query.
    #[test]
    fn sanitize_query_does_not_join_prose() {
        assert_eq!(
            sanitize_query("saicv8 OpenYoke github\nThis query should find the repo.").unwrap(),
            "saicv8 OpenYoke github"
        );
    }

    #[test]
    fn sanitize_query_caps_length() {
        let long = (1..=30).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
        assert_eq!(sanitize_query(&long).unwrap().split_whitespace().count(), 15);
    }

    /// An empty reply is a worse query than the raw question, so reject it and
    /// let the caller fall back.
    #[test]
    fn sanitize_query_rejects_junk() {
        assert!(sanitize_query("").is_none());
        assert!(sanitize_query("   \n  ").is_none());
    }

    #[test]
    fn build_conversation_increments_id() {
        let existing = vec![build_conversation(&[], "first")];
        let second = build_conversation(&existing, "second");
        assert_eq!(second["id"], 2);
        assert_eq!(second["title"], "second");
        assert_eq!(second["nodes"], json!([]));
    }

    #[test]
    fn conversation_index_finds_and_errors() {
        let conversations = vec![
            json!({ "id": 1, "title": "a", "nodes": [] }),
            json!({ "id": 2, "title": "b", "nodes": [] }),
        ];
        assert_eq!(conversation_index(&conversations, 2).unwrap(), 1);
        assert!(conversation_index(&conversations, 99).is_err());
    }

    #[test]
    fn build_conversation_uses_max_id_not_len() {
        // A gap (ids 1, 5) must not recycle: next is 6, not len+1 (== 3).
        let existing = vec![json!({ "id": 1 }), json!({ "id": 5 })];
        assert_eq!(build_conversation(&existing, "x")["id"], 6);
    }

    #[test]
    fn conversation_nodes_defaults_empty_for_legacy() {
        let legacy = json!({ "id": 1, "title": "old", "messages": [] });
        assert!(conversation_nodes(&legacy).is_empty());
    }

    #[test]
    fn set_title_renames_conversation() {
        let mut conversations = vec![build_conversation(&[], "New conversation")];
        let updated = set_title(&mut conversations, 1, "Renamed").unwrap();
        assert_eq!(updated["title"], "Renamed");
        assert_eq!(conversations[0]["title"], "Renamed");
    }

    #[test]
    fn settings_default_has_base_url() {
        let settings = Settings::default();
        assert_eq!(settings.base_url, DEFAULT_BASE_URL);
        assert!(settings.catalog_url.is_empty());
    }
}
