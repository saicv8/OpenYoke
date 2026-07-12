//! The browsable model library.
//!
//! Ollama has no official API to list its library, so OpenYoke SCRAPES
//! `ollama.com/library` at runtime for a live, comprehensive list. If that
//! fetch fails for any reason — offline, a non-2xx response, or a page redesign
//! that our parser no longer understands — we fall back to the curated catalog
//! bundled into the binary, so browsing always works.
//!
//! A user-set catalog URL (a hosted JSON) overrides the scrape entirely.

use std::time::Duration;

use regex::Regex;
use serde_json::{json, Value};

/// Curated catalog compiled into the binary as the offline fallback.
const BUNDLED: &str = include_str!("../catalog.json");

const LIBRARY_URL: &str = "https://ollama.com/library";

/// Fetch the model library. Precedence: a user-set JSON URL, else the live
/// scrape, else the bundled fallback.
pub async fn fetch(url: Option<&str>) -> Value {
    // 1. An explicit custom JSON URL wins.
    if let Some(url) = url.filter(|u| !u.trim().is_empty()) {
        if let Ok(response) = reqwest::Client::new()
            .get(url)
            .timeout(Duration::from_secs(8))
            .send()
            .await
        {
            if response.status().is_success() {
                if let Ok(value) = response.json::<Value>().await {
                    return annotate(value, "remote");
                }
            }
        }
        return bundled(); // custom URL failed -> safe fallback
    }

    // 2. Default: scrape the live Ollama library.
    match scrape_library().await {
        Ok(models) if !models.is_empty() => {
            annotate(json!({ "version": 1, "models": models }), "live")
        }
        _ => bundled(),
    }
}

fn bundled() -> Value {
    let value = serde_json::from_str(BUNDLED).unwrap_or_else(|_| json!({ "models": [] }));
    annotate(value, "bundled")
}

fn annotate(mut value: Value, source: &str) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("source".into(), json!(source));
    }
    value
}

async fn scrape_library() -> Result<Vec<Value>, String> {
    let response = reqwest::Client::new()
        .get(LIBRARY_URL)
        .header("User-Agent", "OpenYoke")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        return Err(format!("library returned {}", response.status()));
    }
    let html = response.text().await.map_err(|e| e.to_string())?;
    Ok(parse_library(&html))
}

/// Decode the handful of HTML entities that show up in model descriptions.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
        .replace("&nbsp;", " ")
}

/// Parse the `ollama.com/library` HTML into catalog model entries. Pure and
/// unit-tested so a page change is caught by tests, not silently at runtime.
///
/// Anchors on Ollama's `x-test-*` attributes (their own test hooks), which are
/// more stable than CSS classes.
fn parse_library(html: &str) -> Vec<Value> {
    let title_re = Regex::new(r#"x-test-model-title\s+title="([^"]+)""#).unwrap();
    let desc_re =
        Regex::new(r#"(?s)<p class="max-w-lg break-words[^"]*"[^>]*>(.*?)</p>"#).unwrap();
    let size_re = Regex::new(r#"x-test-size[^>]*>([^<]+)</span>"#).unwrap();
    let cap_re = Regex::new(r#"x-test-capability[^>]*>([^<]+)</span>"#).unwrap();

    // Each model's block runs from its title marker to the next one.
    let marks: Vec<(usize, String)> = title_re
        .captures_iter(html)
        .filter_map(|c| Some((c.get(0)?.start(), c.get(1)?.as_str().to_string())))
        .collect();

    let mut models = Vec::with_capacity(marks.len());
    for (i, (start, name)) in marks.iter().enumerate() {
        let end = marks.get(i + 1).map(|(s, _)| *s).unwrap_or(html.len());
        let block = &html[*start..end];

        let description = desc_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| decode_entities(m.as_str().trim()))
            .unwrap_or_default();

        let tags: Vec<Value> = cap_re
            .captures_iter(block)
            .filter_map(|c| c.get(1))
            .map(|m| json!(m.as_str().trim()))
            .collect();

        let mut variants: Vec<Value> = size_re
            .captures_iter(block)
            .filter_map(|c| c.get(1))
            .map(|m| {
                let size = m.as_str().trim();
                json!({ "tag": format!("{name}:{size}"), "label": size.to_uppercase() })
            })
            .collect();
        if variants.is_empty() {
            variants.push(json!({ "tag": name, "label": "latest" }));
        }

        models.push(json!({
            "name": name,
            "title": name,
            "description": description,
            "tags": tags,
            "variants": variants,
        }));
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_is_valid_json_with_models() {
        let value: Value = serde_json::from_str(BUNDLED).expect("bundled catalog must be valid JSON");
        assert!(value.get("models").and_then(|m| m.as_array()).is_some());
    }

    #[test]
    fn annotate_sets_source() {
        assert_eq!(annotate(json!({ "models": [] }), "bundled")["source"], "bundled");
    }

    #[test]
    fn parse_library_extracts_models() {
        // Mirrors the real ollama.com/library markup (x-test-* hooks).
        let html = r#"
        <li x-test-model class="flex">
          <a href="/library/llama3.1" class="group">
            <div x-test-model-title title="llama3.1" class="flex">
              <p class="max-w-lg break-words text-neutral-800 text-md">Meta model &amp; friends.</p>
            </div>
            <div class="flex flex-wrap space-x-2">
              <span x-test-capability class="x">tools</span>
              <span x-test-size class="x">8b</span>
              <span x-test-size class="x">70b</span>
            </div>
          </a>
        </li>
        <li x-test-model class="flex">
          <a href="/library/llava" class="group">
            <div x-test-model-title title="llava" class="flex">
              <p class="max-w-lg break-words text-neutral-800 text-md">A vision model.</p>
            </div>
            <div class="flex flex-wrap space-x-2">
              <span x-test-capability class="x">vision</span>
              <span x-test-size class="x">7b</span>
            </div>
          </a>
        </li>"#;

        let models = parse_library(html);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["name"], "llama3.1");
        assert_eq!(models[0]["description"], "Meta model & friends."); // entity decoded
        assert_eq!(models[0]["tags"], json!(["tools"]));
        assert_eq!(
            models[0]["variants"],
            json!([
                { "tag": "llama3.1:8b", "label": "8B" },
                { "tag": "llama3.1:70b", "label": "70B" }
            ])
        );
        assert_eq!(models[1]["name"], "llava");
        assert_eq!(models[1]["variants"][0]["tag"], "llava:7b");
    }

    #[test]
    fn parse_library_handles_no_matches() {
        assert!(parse_library("<html>no models here</html>").is_empty());
    }

    #[test]
    fn parse_library_defaults_variant_when_no_sizes() {
        let html = r#"<div x-test-model-title title="nomic-embed-text" class="x">
            <p class="max-w-lg break-words text-md">Embeddings.</p></div>"#;
        let models = parse_library(html);
        assert_eq!(models[0]["variants"], json!([{ "tag": "nomic-embed-text", "label": "latest" }]));
    }
}
