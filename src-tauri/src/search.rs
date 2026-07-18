//! Keyless web search via DuckDuckGo's HTML endpoint (POST).
//!
//! This is provider-agnostic on purpose: OpenYoke does the search itself and
//! injects the results into the prompt, so web context works for EVERY model —
//! local Ollama models and every cloud provider alike — without relying on any
//! provider's own (cloud-only) search tool.
//!
//! It's screen-scraping, so it degrades gracefully: on any failure the caller
//! simply proceeds without web context.

use std::time::Duration;

use regex::Regex;
use serde::Serialize;

const ENDPOINT: &str = "https://html.duckduckgo.com/html/";
const UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko)";
const MAX_RESULTS: usize = 6;

#[derive(Serialize, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub async fn web_search(query: &str) -> Result<Vec<SearchResult>, String> {
    let response = reqwest::Client::new()
        .post(ENDPOINT)
        .header("User-Agent", UA)
        .form(&[("q", query)])
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("web search returned {}", response.status()));
    }
    let html = response.text().await.map_err(|e| e.to_string())?;
    Ok(parse_results(&html))
}

/// Format results as a prompt block injected ahead of the user's question.
pub fn format_context(results: &[SearchResult]) -> String {
    let mut out = String::from(
        "Web search results (today's web). Use them to answer and cite sources by their [n] number where relevant:\n\n",
    );
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!("[{}] {} — {}\n{}\n\n", i + 1, r.title, r.url, r.snippet));
    }
    out
}

// --- Pure parsing (unit-tested) ---------------------------------------------

fn parse_results(html: &str) -> Vec<SearchResult> {
    // <a rel="nofollow" class="result__a" href="URL">TITLE</a>
    let anchor = Regex::new(r#"(?s)class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#).unwrap();
    let snippet = Regex::new(r#"(?s)class="result__snippet"[^>]*>(.*?)</a>"#).unwrap();

    let anchors: Vec<(String, String)> = anchor
        .captures_iter(html)
        .filter_map(|c| Some((c.get(1)?.as_str().to_string(), clean(c.get(2)?.as_str()))))
        .collect();
    let snippets: Vec<String> = snippet
        .captures_iter(html)
        .filter_map(|c| Some(clean(c.get(1)?.as_str())))
        .collect();

    anchors
        .into_iter()
        .take(MAX_RESULTS)
        .enumerate()
        .filter(|(_, (url, title))| !url.is_empty() && !title.is_empty())
        .map(|(i, (url, title))| SearchResult {
            title,
            url,
            snippet: snippets.get(i).cloned().unwrap_or_default(),
        })
        .collect()
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

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

fn clean(s: &str) -> String {
    decode_entities(&strip_tags(s)).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ddg_results() {
        // Mirrors the real DuckDuckGo HTML markup.
        let html = r#"
        <a rel="nofollow" class="result__a" href="https://rust-lang.org/">Rust <b>Programming</b> Language</a>
        <a class="result__snippet" href="https://rust-lang.org/"><b>Rust</b> is a fast &amp; reliable language.</a>
        <a rel="nofollow" class="result__a" href="https://doc.rust-lang.org/book/">The Rust Book</a>
        <a class="result__snippet" href="https://doc.rust-lang.org/book/">Learn Rust step by step.</a>"#;
        let results = parse_results(html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language"); // <b> stripped
        assert_eq!(results[0].url, "https://rust-lang.org/");
        assert_eq!(results[0].snippet, "Rust is a fast & reliable language."); // entity decoded
        assert_eq!(results[1].title, "The Rust Book");
    }

    #[test]
    fn handles_no_results() {
        assert!(parse_results("<html>nothing here</html>").is_empty());
    }

    #[test]
    fn format_context_numbers_sources() {
        let r = vec![SearchResult {
            title: "T".into(),
            url: "u".into(),
            snippet: "s".into(),
        }];
        let ctx = format_context(&r);
        assert!(ctx.contains("[1] T — u"));
        assert!(ctx.contains("cite sources"));
    }
}
