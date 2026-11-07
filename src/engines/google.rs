//! Google engine, ported from `searx/engines/google.py`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "go",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.google.com/search";

/// Google web search (HTML endpoint).
pub struct Google {
    meta: EngineMeta,
}

impl Google {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Google {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("num", "10".to_string()),
            ("start", ((pageno - 1) * 10).to_string()),
            // the basic HTML endpoint is much easier to parse than the JS one
            ("gbv", "1".to_string()),
            ("filter", "0".to_string()),
        ];
        if language != "all" {
            pairs.push(("hl", language.clone()));
            pairs.push(("lr", format!("lang_{language}")));
        }
        let safe = if query.safesearch >= 1 {
            "active"
        } else {
            "off"
        };
        pairs.push(("safe", safe.to_string()));
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "qdr:d",
                TimeRange::Week => "qdr:w",
                TimeRange::Month => "qdr:m",
                TimeRange::Year => "qdr:y",
            };
            pairs.push(("tbs", value.to_string()));
        }
        let url = format!("{BASE_URL}?{}", query_string(&pairs));
        Ok(EngineRequest::get(url)
            .header("Accept-Language", &format!("{language},*;q=0.5"))
            .header("Sec-Fetch-Site", "same-origin"))
    }

    /// Google answers a plain request with a "Verifying..." interstitial that
    /// only a real browser clears, so ask the solver to render the page.
    fn needs_render(&self, response: &EngineResponse) -> bool {
        let body = response.text();
        !body.contains("yuRUbf") && !body.contains("did not match any documents")
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        // Google's own scripts mention "/sorry/index" on every result page, so
        // the interstitial is only recognised by its small size or its text.
        if body.contains("detected unusual traffic")
            || (body.len() < 20_000 && body.contains("/sorry/index"))
        {
            return Err(EngineError::Captcha);
        }
        let html = parse_html(&body);
        let blocked = !body.contains("did not match any documents")
            && !body.contains("/url?q=")
            && !body.contains("MjjYud")
            && !body.contains("class=\"g\"");
        // Google now hides the destination behind an opaque /goto?url=<token>
        // link and only publishes the real address in a trailing TgQPHd
        // comment, so the visible title is the join key.
        let published = published_urls(&body);
        let mut seen = HashSet::new();
        let mut results = Vec::new();
        for element in select_elements(&html, "div.g, div.tF2Cxc, div.MjjYud")? {
            let Some(link) = child(element, "a[href]") else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let heading = text_of(element, "h3").unwrap_or_else(|| text(link));
            let url = match unwrap_google_url(&href) {
                Some(url) => url,
                None => match published
                    .get(heading.trim())
                    .cloned()
                    .or_else(|| text_of(element, "cite").and_then(|cite| cite_to_url(&cite)))
                {
                    Some(url) => url,
                    None => continue,
                },
            };
            if !seen.insert(url.clone()) {
                continue;
            }
            let title = text_of(element, "h3").unwrap_or_else(|| text(link));
            if title.is_empty() || title.starts_with("http") {
                continue;
            }
            let content = text_of(element, "div.VwiC3b")
                .or_else(|| text_of(element, "span.aCOpRe"))
                .or_else(|| text_of(element, "div[data-sncf]"))
                .or_else(|| text_of(element, "div.IsZvec"))
                .unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        tracing::debug!(
            len = body.len(),
            yu = body.matches("yuRUbf").count(),
            mj = body.matches("MjjYud").count(),
            tf = body.matches("tF2Cxc").count(),
            g = body.matches("class=\"g\"").count(),
            uq = body.matches("/url?q=").count(),
            results = results.len(),
            blocked,
            "google parse"
        );
        if results.is_empty() && blocked {
            // Google serves an interstitial (no result markup at all) to
            // clients it does not trust; report it instead of "no results".
            return Err(EngineError::AccessDenied);
        }
        Ok(results)
    }
}

/// Collect the destination addresses Google publishes in the trailing
/// `<!--TgQPHd|||[[&quot;<url>&quot;` comment of a result anchor.
///
/// The visible anchor now points at an opaque `/goto?url=<token>` that only
/// the browser can resolve, so the comment is the only readable source. The
/// comment follows the anchor text, which makes the title the join key.
fn published_urls(body: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let marker = "<!--TgQPHd";
    let mut rest = body;
    while let Some(at) = rest.find(marker) {
        rest = &rest[at + marker.len()..];
        let Some(open) = rest.find("[[&quot;http") else {
            continue;
        };
        let tail = &rest[open + "[[&quot;".len()..];
        let Some(end) = tail.find("&quot;") else {
            continue;
        };
        let url = tail[..end].replace("&amp;", "&");
        // The title is the text of the anchor that precedes the comment.
        let before = &rest[..open];
        if let Some(close) = before.rfind("</a>") {
            let head = &before[..close];
            if let Some(gt) = head.rfind('>') {
                let title = head[gt + 1..].trim();
                if !title.is_empty() && !title.contains('<') {
                    map.entry(title.to_string()).or_insert(url);
                }
            }
        }
    }
    map
}

/// Rebuild the destination from the breadcrumb Google prints under a result
/// ("https://host › path › page"). The separators are decorative, so they
/// collapse to slashes.
fn cite_to_url(cite: &str) -> Option<String> {
    let trimmed = cite.trim();
    if !trimmed.starts_with("http") {
        return None;
    }
    let mut url = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        match ch {
            '›' | '»' | '‣' => url.push('/'),
            c if c.is_whitespace() => {}
            c => url.push(c),
        }
    }
    let url = url.trim_end_matches('/').to_string();
    if url.starts_with("http") && url.len() > 8 {
        Some(url)
    } else {
        None
    }
}

/// Unwrap the `/url?q=...` redirects Google uses in its result links.
fn unwrap_google_url(href: &str) -> Option<String> {
    if let Some(rest) = href.strip_prefix("/url?") {
        for pair in rest.split('&') {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or_default();
            let value = parts.next().unwrap_or_default();
            if key == "q" || key == "url" {
                if let Ok(decoded) = percent_encoding::percent_decode_str(value).decode_utf8() {
                    return Some(decoded.to_string());
                }
            }
        }
    }
    // Google hides the destination behind an opaque `/goto?url=<token>` that
    // only a browser can resolve; returning the Google URL would be a lie, so
    // leave it to the caller's breadcrumb fallback.
    if href.starts_with("/goto?") {
        return None;
    }
    if href.starts_with('/') {
        return Some(format!("https://www.google.com{href}"));
    }
    absolute_url(href, "https://www.google.com/")
}
