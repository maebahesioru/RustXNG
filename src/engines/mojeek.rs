//! Mojeek engine, ported from `searx/engines/mojeek.py`.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "mjk",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.mojeek.com/search";

/// Mojeek, an independent crawler based search engine.
pub struct Mojeek {
    meta: EngineMeta,
}

impl Mojeek {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Mojeek {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("safe", query.safesearch.min(1).to_string()),
        ];
        if pageno > 1 {
            pairs.push(("s", (10 * (pageno - 1)).to_string()));
        }
        if let Some(range) = query.time_range {
            let now = chrono::Utc::now();
            let since = match range {
                TimeRange::Day => now - chrono::Duration::days(1),
                TimeRange::Week => now - chrono::Duration::weeks(1),
                TimeRange::Month => now - chrono::Duration::days(31),
                TimeRange::Year => now - chrono::Duration::days(365),
            };
            pairs.push(("since", since.format("%Y%m%d").to_string()));
        }
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .cookie("language", &locale_language(&query.locale))
                .cookie("region", &locale_region(&query.locale)),
        )
    }

    /// Mojeek answers with an Altcha proof-of-work page instead of a CAPTCHA.
    /// The widget is solvable without a browser: the challenge carries a
    /// PBKDF2/SHA-256 puzzle, and the answer is posted to /captcha/verify.
    fn followup_request(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Option<EngineRequest>, EngineError> {
        let body = response.text();
        if !body.contains("altcha") {
            return Ok(None);
        }
        let Some(challenge) = altcha_challenge(&body) else {
            return Ok(None);
        };
        let Some(solution) = solve_altcha(&challenge) else {
            return Ok(None);
        };
        Ok(Some(
            EngineRequest::post(format!("{BASE_URL}/captcha/verify"))
                .form(vec![("altcha".to_string(), solution)]),
        ))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        // Mojeek answers with an Altcha proof-of-work page when it rate limits.
        if body.contains("altcha") || body.contains("captcha") {
            return Err(EngineError::Captcha);
        }
        let html = parse_html(&body);
        let mut results = Vec::new();
        for link in select_elements(&html, "ul.results-standard li a.ob, ul.results li a.title")? {
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, "https://www.mojeek.com/") else {
                continue;
            };
            let Some(item) = link.parent().and_then(scraper::ElementRef::wrap) else {
                continue;
            };
            let title = text_of(item, "h2 a").unwrap_or_else(|| text(link));
            if title.is_empty() {
                continue;
            }
            let content = text_of(item, "p.s").unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        Ok(results)
    }
}

/// The base64 challenge the Altcha widget embeds in its `challenge` attribute.
fn altcha_challenge(body: &str) -> Option<String> {
    let marker = "challenge";
    let mut rest = body;
    while let Some(index) = rest.find(marker) {
        let after = &rest[index + marker.len()..];
        let quote_at = after.find(|c: char| c == char::from(39) || c == char::from(34));
        if let Some(quote) = quote_at {
            let value = &after[quote + 1..];
            if let Some(end) = value.find(|c: char| c == char::from(39) || c == char::from(34)) {
                let candidate = &value[..end];
                if candidate.len() > 40
                    && candidate
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
                {
                    return Some(candidate.to_string());
                }
            }
        }
        rest = &rest[index + marker.len()..];
    }
    None
}

/// Solve the Altcha proof of work and return the JSON answer the widget posts.
fn solve_altcha(challenge_b64: &str) -> Option<String> {
    let raw = base64_decode(challenge_b64)?;
    let json: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let algorithm = json.get("algorithm")?.as_str()?;
    if algorithm != "SHA-256" {
        return None;
    }
    let challenge = json.get("challenge")?.as_str()?.to_string();
    let salt = json.get("salt")?.as_str()?.to_string();
    let signature = json.get("signature")?.as_str()?.to_string();
    let max = json.get("maxnumber")?.as_u64().unwrap_or(1_000_000);
    let cost = json.get("cost")?.as_u64().unwrap_or(0);
    let zeros = vec![0u8; (cost as usize).div_ceil(2)];
    let mut number = 0u64;
    let mut hasher = Sha256::new();
    while number <= max {
        hasher.update(format!("{salt}{number}").as_bytes());
        let digest = hasher.finalize_reset();
        if digest[..].starts_with(&zeros) {
            break;
        }
        number += 1;
    }
    if number > max {
        return None;
    }
    let payload = serde_json::json!({
        "algorithm": algorithm,
        "challenge": challenge,
        "number": number,
        "salt": salt,
        "signature": signature,
    });
    Some(base64_encode(payload.to_string().as_bytes()))
}

fn base64_decode(value: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(value).ok()
}

fn base64_encode(value: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value)
}
