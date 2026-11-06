//! Mojeek engine, ported from `searx/engines/mojeek.py`.
//!
//! Mojeek answers a rate limited client with an Altcha proof-of-work page
//! instead of results. The current widget solves a PBKDF2/SHA-256 puzzle and
//! posts it to `/captcha/verify`; the server then sets a `chllg` cookie that is
//! good for a month. The frozen `build_request` interface cannot perform those
//! extra round trips, so the cookie is read from the `mojeek_cookie_file`
//! option, exactly like `tiger` reads its arithmetic answer: `mojeek_solve.py`
//! refreshes the file out of band and every request picks it up.

use std::sync::Arc;

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

/// Split a Cookie header ("a=1; b=2") into its name/value pairs.
fn cookie_pairs(raw: &str) -> Vec<(String, String)> {
    raw.split(';')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            let name = name.trim();
            (!name.is_empty()).then(|| (name.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Mojeek, an independent crawler based search engine.
pub struct Mojeek {
    meta: EngineMeta,
    /// Path of the cookie file written by mojeek_solve.py (see the module docs).
    cookie_file: Option<String>,
}

impl Mojeek {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            cookie_file: config
                .opt_str("mojeek_cookie_file")
                .filter(|path| !path.is_empty())
                .map(str::to_string),
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
        let mut request = EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
            .cookie("language", &locale_language(&query.locale))
            .cookie("region", &locale_region(&query.locale));
        // The Altcha clearance cookie. It is not bound to the exit address, so
        // one solved session serves every request until it expires.
        if let Some(path) = self.cookie_file.as_deref() {
            if let Ok(raw) = std::fs::read_to_string(path) {
                for (name, value) in cookie_pairs(&raw) {
                    request = request.cookie(&name, &value);
                }
            }
        }
        Ok(request)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(cookie_file: Option<&str>) -> Arc<Mojeek> {
        let mut config = EngineConfig::new("mojeek", "mojeek", "mjk", &["general"]);
        if let Some(path) = cookie_file {
            config.extra.insert(
                "mojeek_cookie_file".to_string(),
                toml::Value::String(path.to_string()),
            );
        }
        Mojeek::from_config(&config).expect("engine")
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_search_request() {
        let request = engine(None).build_request(&query(), 2).expect("request");
        assert_eq!(
            request.url,
            "https://www.mojeek.com/search?q=rust&safe=0&s=10"
        );
    }

    #[test]
    fn a_cookie_file_carries_the_altcha_clearance() {
        let path = std::env::temp_dir().join(format!("mojeek-cookie-{}.txt", std::process::id()));
        std::fs::write(&path, "chllg=abc.def").expect("write");
        let request = engine(Some(path.to_str().expect("utf-8")))
            .build_request(&query(), 1)
            .expect("request");
        let _ = std::fs::remove_file(&path);
        assert!(
            request
                .cookies
                .iter()
                .any(|(name, value)| name == "chllg" && value == "abc.def")
        );
    }

    #[test]
    fn a_challenge_page_is_a_captcha() {
        let response = EngineResponse::new(
            200,
            "https://www.mojeek.com/search",
            b"<title>Captcha</title><altcha-widget>".to_vec(),
        );
        assert!(matches!(
            engine(None).parse(&response, &query()),
            Err(EngineError::Captcha)
        ));
    }

    #[test]
    fn parses_the_result_list() {
        let body = concat!(
            "<html><body><ul class=\"results-standard\">",
            "<li class=\"r1\"><a href=\"https://rust-lang.org/\" class=\"ob\"><p class=\"i\">",
            "<span class=\"url\">https://rust-lang.org/</span></p></a>",
            "<h2><a class=\"title\" href=\"https://rust-lang.org/\">Rust Programming Language</a></h2>",
            "<p class=\"s\">Rust is blazingly fast.</p></li>",
            "</ul></body></html>"
        );
        let response = EngineResponse::new(200, "https://www.mojeek.com/search", body.as_bytes().to_vec());
        let items = engine(None).parse(&response, &query()).expect("parse");
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust Programming Language");
                assert_eq!(result.url, "https://rust-lang.org/");
                assert_eq!(result.content, "Rust is blazingly fast.");
            }
            _ => panic!("expected a result"),
        }
    }
}
