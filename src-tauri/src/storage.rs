//! Where OpenYoke keeps its data.
//!
//! The user chooses a storage folder; conversations and settings live there.
//! We can't store *that choice* in the same folder (we wouldn't know where to
//! look), so a tiny `config.json` pointer stays in the platform app-data dir
//! and records the chosen path. Everything else is resolved relative to it.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

/// Status of the storage location, surfaced to the frontend so it can decide
/// whether to block the app behind the "choose a folder" modal.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    pub configured: bool,
    pub path: String,
    pub valid: bool,
    pub message: String,
}

/// Path to the pointer file in the default app-data dir.
fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("config.json"))
}

/// Read the configured storage folder, if one has been set.
fn read_pointer(app: &AppHandle) -> Result<Option<PathBuf>, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(value
        .get("storage_dir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from))
}

fn write_pointer(app: &AppHandle, dir: &Path) -> Result<(), String> {
    let body = serde_json::to_string_pretty(&json!({ "storage_dir": dir.to_string_lossy() }))
        .map_err(|e| e.to_string())?;
    fs::write(config_path(app)?, body).map_err(|e| e.to_string())
}

/// Validate a candidate folder path and make sure we can actually write to it.
/// Returns the prepared path, or a user-facing error message.
pub fn validate_and_prepare(input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Please enter a folder path.".into());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("Please enter an absolute path, e.g. /Users/you/OpenYoke.".into());
    }
    fs::create_dir_all(&path).map_err(|e| format!("Could not create that folder: {e}"))?;
    if !path.is_dir() {
        return Err("That path exists but is not a folder.".into());
    }
    // Confirm writability with a probe file rather than trusting metadata.
    let probe = path.join(".openyoke_write_test");
    fs::write(&probe, b"ok").map_err(|e| format!("That folder is not writable: {e}"))?;
    let _ = fs::remove_file(&probe);
    Ok(path)
}

/// Persist the user's chosen folder. Returns the normalized path on success.
pub fn set_dir(app: &AppHandle, input: &str) -> Result<String, String> {
    let path = validate_and_prepare(input)?;
    write_pointer(app, &path)?;
    Ok(path.to_string_lossy().to_string())
}

/// Report whether storage is configured and still usable.
pub fn status(app: &AppHandle) -> Result<StorageStatus, String> {
    match read_pointer(app)? {
        None => Ok(StorageStatus {
            configured: false,
            path: String::new(),
            valid: false,
            message: String::new(),
        }),
        Some(dir) => {
            let path = dir.to_string_lossy().to_string();
            match validate_and_prepare(&path) {
                Ok(_) => Ok(StorageStatus {
                    configured: true,
                    path,
                    valid: true,
                    message: String::new(),
                }),
                Err(message) => Ok(StorageStatus {
                    configured: true,
                    path,
                    valid: false,
                    message,
                }),
            }
        }
    }
}

/// Resolve the active data directory, erroring if storage isn't set yet.
pub fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = read_pointer(app)?
        .ok_or_else(|| "Storage location is not set. Choose a folder to continue.".to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_path() {
        assert!(validate_and_prepare("   ").is_err());
    }

    #[test]
    fn rejects_relative_path() {
        assert!(validate_and_prepare("some/relative/dir").is_err());
    }

    #[test]
    fn prepares_absolute_writable_dir() {
        let mut base = std::env::temp_dir();
        base.push("openyoke_storage_test_dir");
        let input = base.to_string_lossy().to_string();

        let prepared = validate_and_prepare(&input).expect("should prepare temp dir");
        assert!(prepared.is_dir());
        assert!(!prepared.join(".openyoke_write_test").exists(), "probe file cleaned up");

        let _ = fs::remove_dir_all(&base);
    }
}
