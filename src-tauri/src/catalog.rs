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
        // Say why we degraded. A silent fallback is indistinguishable from
        // "the library really does only have a handful of models".
        Ok(_) => {
            eprintln!("catalog: {LIBRARY_URL} parsed to 0 models (page markup changed?), using bundled catalog");
            bundled()
        }
        Err(e) => {
            eprintln!("catalog: could not fetch {LIBRARY_URL} ({e}), using bundled catalog");
            bundled()
        }
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
/// Anchors on the `/library/<name>` link that each card is built around — the
/// one part of the card that carries meaning rather than styling. Ollama used
/// to ship `x-test-*` attributes and we keyed off those; they were removed in a
/// redesign, which silently emptied this parser, so prefer structure over hooks.
fn parse_library(html: &str) -> Vec<Value> {
    let title_re = Regex::new(r#"href="/library/([^"?#]+)"[^>]*class="group"#).unwrap();
    let desc_re =
        Regex::new(r#"(?s)<p class="max-w-lg break-words[^"]*"[^>]*>(.*?)</p>"#).unwrap();
    // Sizes and capabilities are identical badge markup distinguished only by
    // colour: sizes are blue, every other colour is a capability. Classifying
    // by "not the size colour" means a newly introduced capability colour (as
    // happened with `cloud`) still lands in tags instead of being dropped.
    let badge_re =
        Regex::new(r#"<span\s+class="inline-flex items-center rounded-md ([^"]*)">([^<]*)</span>"#)
            .unwrap();
    const SIZE_COLOR: &str = "#ddf4ff";

    // Each model's block runs from its link marker to the next one.
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

        let mut tags: Vec<Value> = Vec::new();
        let mut variants: Vec<Value> = Vec::new();
        for capture in badge_re.captures_iter(block) {
            let (Some(class), Some(text)) = (capture.get(1), capture.get(2)) else {
                continue;
            };
            let text = text.as_str().trim();
            if text.is_empty() {
                continue;
            }
            if class.as_str().contains(SIZE_COLOR) {
                variants.push(
                    json!({ "tag": format!("{name}:{text}"), "label": text.to_uppercase() }),
                );
            } else {
                tags.push(json!(text));
            }
        }
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

    /// Verbatim excerpt of live ollama.com/library markup, whitespace and all.
    /// Copy a fresh card in here if the parser ever comes up empty again.
    const LIBRARY_FIXTURE: &str = r#"
    <ul role="list" class="grid grid-cols-1 gap-y-3">
      <li  class="flex items-baseline border-b border-neutral-200 py-6">
        <a href="/library/llama3.1" class="group w-full space-y-5">
          <div  title="llama3.1" class="flex flex-col">
            <h2 class="truncate text-xl font-medium underline-offset-2 md:text-2xl">
              <div class="flex space-x-2 items-center">
                <span class="group-hover:underline truncate">llama3.1</span>
              </div>
            </h2>
            <p class="max-w-lg break-words text-neutral-800 text-md">Meta model &amp; friends.</p>
          </div>
          <div class="flex flex-col space-y-2">
            <div class="flex flex-wrap space-x-2">
              <span  class="inline-flex items-center rounded-md bg-indigo-50 px-2 py-0.5 text-xs font-medium text-indigo-600 sm:text-[13px]">tools</span>
              <span  class="inline-flex items-center rounded-md bg-[#ddf4ff] px-2 py-0.5 text-xs font-medium text-blue-600 sm:text-[13px]">8b</span>
              <span  class="inline-flex items-center rounded-md bg-[#ddf4ff] px-2 py-0.5 text-xs font-medium text-blue-600 sm:text-[13px]">70b</span>
            </div>
          </div>
        </a>
      </li>
      <li  class="flex items-baseline border-b border-neutral-200 py-6">
        <a href="/library/llava" class="group w-full space-y-5">
          <div  title="llava" class="flex flex-col">
            <p class="max-w-lg break-words text-neutral-800 text-md">A vision model.</p>
          </div>
          <div class="flex flex-wrap space-x-2">
            <span  class="inline-flex items-center rounded-md bg-indigo-50 px-2 py-0.5 text-xs font-medium text-indigo-600 sm:text-[13px]">vision</span>
            <span  class="inline-flex items-center rounded-md bg-[#ddf4ff] px-2 py-0.5 text-xs font-medium text-blue-600 sm:text-[13px]">7b</span>
          </div>
        </a>
      </li>
    </ul>"#;

    #[test]
    fn parse_library_extracts_models() {
        let models = parse_library(LIBRARY_FIXTURE);
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

    /// Hits the network, so it is not part of the default run. This is the test
    /// that would have caught the `x-test-*` removal — the fixture tests cannot,
    /// because a fixture keeps passing long after the real page has moved on.
    /// Run periodically: `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "requires network"]
    async fn parse_library_still_matches_the_live_page() {
        let models = scrape_library().await.expect("fetch ollama.com/library");
        assert!(models.len() > 50, "only parsed {} models", models.len());
        assert!(
            models.iter().all(|m| !m["description"].as_str().unwrap_or("").is_empty()),
            "some models parsed with no description"
        );
        assert!(
            models.iter().any(|m| m["name"] == "llama3.1"),
            "expected a well-known model in the library"
        );
    }

    /// End-to-end check on what the UI actually renders: an empty `catalog_url`
    /// (the default) must reach the live scrape, not silently degrade to
    /// `source: "bundled"` — which is the state this fixed.
    #[tokio::test]
    #[ignore = "requires network"]
    async fn fetch_with_no_custom_url_serves_the_live_library() {
        let catalog = fetch(Some("")).await;
        assert_eq!(catalog["source"], "live");
        assert!(catalog["models"].as_array().unwrap().len() > 50);
    }

    #[test]
    fn parse_library_handles_no_matches() {
        assert!(parse_library("<html>no models here</html>").is_empty());
    }

    #[test]
    fn parse_library_defaults_variant_when_no_sizes() {
        let html = r#"<a href="/library/nomic-embed-text" class="group w-full space-y-5">
            <p class="max-w-lg break-words text-md">Embeddings.</p></a>"#;
        let models = parse_library(html);
        assert_eq!(models[0]["variants"], json!([{ "tag": "nomic-embed-text", "label": "latest" }]));
    }

    /// Ollama introduced a new badge colour for `cloud` after this parser was
    /// written; anything that isn't the size colour must land in tags.
    #[test]
    fn parse_library_treats_unknown_badge_colour_as_capability() {
        let html = r#"<a href="/library/kimi-k2" class="group w-full space-y-5">
            <p class="max-w-lg break-words text-md">Big model.</p>
            <span  class="inline-flex items-center rounded-md bg-cyan-50 px-2 py-0.5 text-xs font-medium text-cyan-600 sm:text-[13px]">cloud</span>
            <span  class="inline-flex items-center rounded-md bg-[#ddf4ff] px-2 py-0.5 text-xs font-medium text-blue-600 sm:text-[13px]">1t</span></a>"#;
        let models = parse_library(html);
        assert_eq!(models[0]["tags"], json!(["cloud"]));
        assert_eq!(models[0]["variants"], json!([{ "tag": "kimi-k2:1t", "label": "1T" }]));
    }
}
