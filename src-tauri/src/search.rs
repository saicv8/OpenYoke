//! Keyless web search via DuckDuckGo's HTML endpoint (POST), plus retrieval of
//! the pages it returns.
//!
//! This is provider-agnostic on purpose: OpenYoke does the search itself and
//! injects the results into the prompt, so web context works for EVERY model —
//! local Ollama models and every cloud provider alike — without relying on any
//! provider's own (cloud-only) search tool.
//!
//! Search alone is not enough. A result's DuckDuckGo snippet is ~150 characters
//! of blurb, which is why grounding used to fail even when the right page ranked
//! first: the model was handed "GitHub is where people build software…" instead
//! of the README it needed. So we also FETCH the result pages and inject their
//! text. Any URL pasted into the question is fetched directly and always wins a
//! slot, since an explicitly named page is the one the user actually meant.
//!
//! It's screen-scraping, so it degrades gracefully: a page that fails to fetch
//! falls back to its snippet, and total search failure proceeds with no web
//! context at all.

use std::time::Duration;

use futures_util::future::join_all;
use regex::Regex;
use serde::Serialize;

const ENDPOINT: &str = "https://html.duckduckgo.com/html/";
const UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko)";
const MAX_RESULTS: usize = 6;

/// Per-page and whole-block caps on injected text. Deliberately generous — the
/// point is to give the model the real page — but still bounded, because this
/// context is also fed to small local models with modest windows.
const PAGE_CHAR_CAP: usize = 10_000;
const TOTAL_CHAR_CAP: usize = 60_000;
/// Pages are fetched concurrently; a slow one must not stall the whole answer.
const PAGE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// Extracted page text. Empty when the fetch failed or was skipped, in
    /// which case the snippet is all we have.
    #[serde(default)]
    pub content: String,
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

/// Search, then retrieve. URLs pasted into `question` are fetched directly and
/// placed first; the remaining slots come from search results for `query`.
///
/// A pasted URL is treated as an instruction to read that page, so it survives
/// even if the search half fails entirely.
pub async fn search_and_retrieve(query: &str, question: &str) -> Vec<SearchResult> {
    let mut results: Vec<SearchResult> = extract_urls(question)
        .into_iter()
        .map(|url| SearchResult {
            // Naming the provenance beats repeating the URL as its own title,
            // and tells the model this source is the one the user pointed at.
            title: "Page linked in the question".to_string(),
            url,
            snippet: String::new(),
            content: String::new(),
        })
        .collect();

    // Search results fill whatever slots the pasted URLs left, skipping any URL
    // we are already about to fetch.
    if let Ok(found) = web_search(query).await {
        for r in found {
            if results.len() >= MAX_RESULTS {
                break;
            }
            if !results.iter().any(|existing| existing.url == r.url) {
                results.push(r);
            }
        }
    }
    results.truncate(MAX_RESULTS);

    // Fetch every page concurrently — serially this would be 6 × latency.
    let pages = join_all(results.iter().map(|r| fetch_page(r.url.clone()))).await;
    for (result, page) in results.iter_mut().zip(pages) {
        result.content = page.unwrap_or_default();
    }
    results
}

/// Rewrite a URL to a version that yields prose rather than app chrome.
///
/// A GitHub repo page is the motivating case: its README is not in the HTML as
/// markup at all, but JSON-escaped inside a `<script>` block that text
/// extraction (rightly) discards — so scraping it yields navigation and a file
/// list, never the README. `raw.githubusercontent.com` serves the same README
/// as clean markdown. Returns `None` when no rewrite applies.
fn readable_alternative(url: &str) -> Option<String> {
    let repo = Regex::new(r#"^https?://(?:www\.)?github\.com/([^/\s]+)/([^/\s?#]+)/?$"#).unwrap();
    let caps = repo.captures(url)?;
    let owner = caps.get(1)?.as_str();
    let name = caps.get(2)?.as_str().trim_end_matches(".git");
    // Reserved paths that look like `owner/repo` but aren't.
    if matches!(owner, "orgs" | "topics" | "settings" | "features" | "sponsors" | "collections") {
        return None;
    }
    Some(format!("https://raw.githubusercontent.com/{owner}/{name}/HEAD/README.md"))
}

/// Fetch one page and reduce it to plain text. Errors are values, not failures:
/// the caller falls back to the snippet.
async fn fetch_page(url: String) -> Result<String, String> {
    // Prefer a prose-bearing equivalent when one exists, but never let that
    // choice lose us the page: a repo with no README falls back to the original.
    if let Some(alternative) = readable_alternative(&url) {
        if let Ok(text) = fetch_text(&alternative).await {
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }
    }
    fetch_text(&url).await
}

async fn fetch_text(url: &str) -> Result<String, String> {
    let response = reqwest::Client::new()
        .get(url)
        .header("User-Agent", UA)
        .timeout(PAGE_TIMEOUT)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("{url} returned {}", response.status()));
    }
    // Only parse things that are actually text; a PDF or image would otherwise
    // become megabytes of mojibake in the prompt.
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !content_type.is_empty()
        && !content_type.contains("text/")
        && !content_type.contains("json")
        && !content_type.contains("xml")
    {
        return Err(format!("{url} is {content_type}, not text"));
    }
    let body = response.text().await.map_err(|e| e.to_string())?;
    Ok(extract_text(&body))
}

/// Format results as a prompt block injected ahead of the user's question.
/// Page text is included where we have it, snippets where we don't.
pub fn format_context(results: &[SearchResult]) -> String {
    let mut out = String::from(
        "Web search results (today's web), including the retrieved text of each page. \
         Use them to answer and cite sources by their [n] number where relevant. \
         Prefer the page content over your prior assumptions about these sources:\n\n",
    );
    let mut budget = TOTAL_CHAR_CAP;
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!("[{}] {} — {}\n", i + 1, r.title, r.url));
        if r.content.is_empty() {
            // No page text: the snippet is the only grounding available, and
            // saying so keeps the model from treating a blurb as a full read.
            out.push_str(&format!("(could not retrieve page; search snippet only)\n{}\n\n", r.snippet));
            continue;
        }
        // Share the remaining budget so one huge early page cannot starve the
        // rest — every result gets at least an equal slice of what is left.
        let remaining_results = results.len() - i;
        let share = (budget / remaining_results).min(PAGE_CHAR_CAP);
        let text = truncate_chars(&r.content, share);
        budget = budget.saturating_sub(text.chars().count());
        out.push_str(&text);
        out.push_str("\n\n");
    }
    out
}

/// Truncate on a character boundary, flagging that we cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}… [truncated]")
}

/// Pull http(s) URLs out of free text, trimming trailing punctuation that
/// naturally ends a sentence rather than a URL.
fn extract_urls(text: &str) -> Vec<String> {
    let re = Regex::new(r#"https?://[^\s<>"'\)\]]+"#).unwrap();
    let mut seen: Vec<String> = Vec::new();
    for m in re.find_iter(text) {
        let url = m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']).to_string();
        if !url.is_empty() && !seen.contains(&url) {
            seen.push(url);
        }
    }
    seen
}

/// Reduce an HTML document to readable text: drop the elements that carry no
/// prose, strip the remaining tags, and collapse the whitespace that markup
/// leaves behind. Capped, because some pages are enormous.
fn extract_text(html: &str) -> String {
    // One pattern per tag: the `regex` crate has no backreferences, so a single
    // `<(a|b)>…</\1>` alternation is not available here.
    let mut stripped = html.to_string();
    for tag in ["script", "style", "noscript", "svg", "head"] {
        let noise = Regex::new(&format!(r#"(?is)<{tag}\b[^>]*>.*?</\s*{tag}\s*>"#)).unwrap();
        stripped = noise.replace_all(&stripped, " ").into_owned();
    }
    // Turn block-level boundaries into newlines so structure survives as text.
    let breaks = Regex::new(r#"(?i)</(p|div|li|tr|h[1-6]|section|article|br)\s*>|<br\s*/?>"#).unwrap();
    let spaced = breaks.replace_all(&stripped, "\n");
    let text = decode_entities(&strip_tags(&spaced));

    // Collapse runs of blank lines and trailing spaces from removed markup.
    let mut out = String::with_capacity(text.len().min(PAGE_CHAR_CAP * 2));
    let mut blank_run = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
            out.push('\n');
        } else {
            blank_run = 0;
            out.push_str(line);
            out.push('\n');
        }
        if out.chars().count() >= PAGE_CHAR_CAP {
            break;
        }
    }
    truncate_chars(out.trim(), PAGE_CHAR_CAP)
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
            content: String::new(), // filled in later by search_and_retrieve
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

    /// The case that motivated retrieval: a question with a pasted repo URL.
    /// Asserts the model would actually receive the page's text — the old
    /// snippet-only path could not, no matter how well the search ranked.
    /// Run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "requires network"]
    async fn retrieves_the_content_of_a_pasted_url() {
        let results = search_and_retrieve(
            "saicv8 OpenYoke github",
            "Tell me about my repo https://github.com/saicv8/OpenYoke",
        )
        .await;

        let repo = results
            .iter()
            .find(|r| r.url.contains("saicv8/OpenYoke"))
            .expect("pasted URL must be among the results");
        assert!(
            repo.content.len() > 500,
            "pasted URL fetched only {} chars of text",
            repo.content.len()
        );
        // Prose from the README body — NOT the navigation chrome that scraping
        // the repo page directly would yield.
        assert!(
            repo.content.contains("local-first"),
            "got page chrome instead of README prose: {:?}",
            repo.content.chars().take(300).collect::<String>()
        );
        assert!(!repo.content.contains("<script"), "raw markup leaked into the prompt");
    }

    #[test]
    fn readable_alternative_rewrites_github_repos() {
        assert_eq!(
            readable_alternative("https://github.com/saicv8/OpenYoke").unwrap(),
            "https://raw.githubusercontent.com/saicv8/OpenYoke/HEAD/README.md"
        );
        // Trailing slash and .git suffix are both common in pasted URLs.
        assert_eq!(
            readable_alternative("https://github.com/saicv8/OpenYoke/").unwrap(),
            "https://raw.githubusercontent.com/saicv8/OpenYoke/HEAD/README.md"
        );
        assert_eq!(
            readable_alternative("https://github.com/saicv8/OpenYoke.git").unwrap(),
            "https://raw.githubusercontent.com/saicv8/OpenYoke/HEAD/README.md"
        );
    }

    /// Deep links and non-repo GitHub pages must be fetched as-is — rewriting a
    /// link to a specific issue into a README would answer the wrong question.
    #[test]
    fn readable_alternative_leaves_other_urls_alone() {
        assert!(readable_alternative("https://github.com/saicv8/OpenYoke/issues/3").is_none());
        assert!(readable_alternative("https://github.com/topics/ai-harness").is_none());
        assert!(readable_alternative("https://github.com/saicv8").is_none());
        assert!(readable_alternative("https://example.com/a/b").is_none());
    }

    #[tokio::test]
    #[ignore = "requires network"]
    async fn search_results_arrive_with_page_text() {
        let results = search_and_retrieve("rust programming language", "What is Rust?").await;
        assert!(!results.is_empty(), "no results at all");
        assert!(
            results.iter().filter(|r| !r.content.is_empty()).count() >= 2,
            "expected most result pages to be retrievable"
        );
    }

    #[test]
    fn handles_no_results() {
        assert!(parse_results("<html>nothing here</html>").is_empty());
    }

    fn result(url: &str, snippet: &str, content: &str) -> SearchResult {
        SearchResult {
            title: "T".into(),
            url: url.into(),
            snippet: snippet.into(),
            content: content.into(),
        }
    }

    #[test]
    fn format_context_numbers_sources() {
        let ctx = format_context(&[result("u", "s", "")]);
        assert!(ctx.contains("[1] T — u"));
        assert!(ctx.contains("cite sources"));
    }

    #[test]
    fn format_context_injects_page_text_not_just_snippet() {
        let ctx = format_context(&[result("u", "blurb", "The full README body.")]);
        assert!(ctx.contains("The full README body."));
    }

    /// A failed fetch must be labelled, so the model doesn't mistake 150
    /// characters of blurb for having read the page.
    #[test]
    fn format_context_marks_unretrieved_pages() {
        let ctx = format_context(&[result("u", "blurb", "")]);
        assert!(ctx.contains("could not retrieve page"));
        assert!(ctx.contains("blurb"));
    }

    #[test]
    fn format_context_respects_total_budget() {
        let huge = "x".repeat(PAGE_CHAR_CAP);
        let results: Vec<SearchResult> =
            (0..MAX_RESULTS).map(|_| result("u", "s", &huge)).collect();
        let ctx = format_context(&results);
        assert!(ctx.chars().count() <= TOTAL_CHAR_CAP + 2_000, "got {} chars", ctx.chars().count());
    }

    /// One enormous first result must not consume the budget and starve the
    /// rest — every source should still appear.
    #[test]
    fn format_context_shares_budget_across_results() {
        let huge = "x".repeat(PAGE_CHAR_CAP * 2);
        let results = vec![result("first", "s", &huge), result("second", "s", "short tail")];
        let ctx = format_context(&results);
        assert!(ctx.contains("short tail"), "later result was starved out");
    }

    #[test]
    fn extract_urls_finds_and_dedupes() {
        let urls = extract_urls(
            "See https://github.com/saicv8/OpenYoke and https://example.com/a. Also https://github.com/saicv8/OpenYoke again.",
        );
        assert_eq!(urls, vec!["https://github.com/saicv8/OpenYoke", "https://example.com/a"]);
    }

    #[test]
    fn extract_urls_trims_sentence_punctuation() {
        assert_eq!(extract_urls("go to https://example.com/x."), vec!["https://example.com/x"]);
        assert_eq!(extract_urls("(https://example.com/y)"), vec!["https://example.com/y"]);
    }

    #[test]
    fn extract_urls_empty_when_none() {
        assert!(extract_urls("no links here at all").is_empty());
    }

    #[test]
    fn extract_text_drops_scripts_and_styles() {
        let html = r#"<html><head><title>T</title></head><body>
            <script>var secret = "should not appear";</script>
            <style>.a { color: red }</style>
            <p>Real prose here.</p></body></html>"#;
        let text = extract_text(html);
        assert!(text.contains("Real prose here."));
        assert!(!text.contains("should not appear"));
        assert!(!text.contains("color: red"));
    }

    #[test]
    fn extract_text_decodes_entities_and_keeps_structure() {
        let text = extract_text("<p>Tom &amp; Jerry</p><p>Second line</p>");
        assert!(text.contains("Tom & Jerry"));
        assert!(text.contains("Second line"));
        assert!(text.lines().count() >= 2, "block structure lost: {text:?}");
    }

    #[test]
    fn extract_text_is_capped() {
        let html = format!("<p>{}</p>", "word ".repeat(50_000));
        assert!(extract_text(&html).chars().count() <= PAGE_CHAR_CAP + 20);
    }
}


