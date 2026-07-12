// Prevent an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod catalog;
mod ollama;
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

#[derive(Serialize, Deserialize, Clone)]
struct Settings {
    base_url: String,
    default_model: String,
    /// Optional URL to refresh the model library from. Empty = bundled catalog.
    #[serde(default)]
    catalog_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            base_url: DEFAULT_BASE_URL.to_string(),
            default_model: String::new(),
            catalog_url: String::new(),
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
) -> Result<Settings, String> {
    let settings = Settings {
        base_url,
        default_model,
        catalog_url,
    };
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
    let messages = tree::build_chat_messages(&path, &question);

    // The one and only generation call. Tokens stream to the frontend over
    // `channel` as they arrive; the full text is returned when done. Transport
    // errors abort before any node is created; an empty reply still persists
    // (the interaction happened). NO lock is held across this await.
    let answer = ollama::chat_stream(&base_url, &model, &messages, &channel).await?;

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
    let node = json!({
        "id": new_id,
        "parentId": parent_id,
        "question": question,
        "answer": answer,
        "model": model,
        "x": x,
        "y": y,
    });

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

// --- Model backend commands (delegate to the ollama module) -----------------

#[tauri::command]
async fn list_models(base_url: String) -> Result<Value, String> {
    ollama::list_models(&base_url).await
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
            list_conversations,
            create_conversation,
            create_node,
            update_node_position,
            delete_node,
            rename_conversation,
            list_models,
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
