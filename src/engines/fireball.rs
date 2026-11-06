//! Fireball engine, ported from `searx/engines/fireball.py` (web search).
//!
//! Upstream supports the categories `web`, `news` and `videos` through the
//! `fireball_category` setting; this port implements the main `web` variant and
//! reads the category from the `search_type` config key. All categories are
//! returned by the same request.
//!
//! Upstream obtains a `fireball` settings cookie from `POST /settings` (cached
//! for a month). The frozen `build_request` interface cannot perform that extra
//! round trip, so the cookie is read from the `fireball_cookie` config option.
// engine-id: fireball

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "fb",
    paging: false,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://fireball.com";

/// Fireball, a German privacy focused search engine.
pub struct Fireball {
    meta: EngineMeta,
    categ: String,
}

impl Fireball {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config.opt_str("search_type").unwrap_or("web").to_string();
        if !matches!(categ.as_str(), "web" | "news" | "videos") {
            return Err(format!("fireball: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for Fireball {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        // `/getResults/` answers 405 now; the results live in the HTML of the
        // search page itself (div.card.my-1 with a window.open() handler).
        let args = [("f", "web".to_string()), ("q", query.query_text())];
        let query = query_string(&args);
        let mut request = EngineRequest::get(format!("{BASE_URL}/search?{query}"))
            .header("Referer", &format!("{BASE_URL}/"))
            .header("Accept", "text/html,application/xhtml+xml");
        if let Some(cookie) = self.meta.opt_str("fireball_cookie") {
            request = request.cookie("fireball", cookie);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let category = match self.categ.as_str() {
            "news" => "news",
            "videos" => "videos",
            _ => "web",
        };
        let Some(items) =
            json_path(&json, &format!("{category}.results")).and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = strip_tags(&json_str(item, "title"));
            let url = json_str(item, "url");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder =
                ResultBuilder::new(url, title).content(strip_tags(&json_str(item, "description")));
            if let Some(date) = json_path(item, "page_age").and_then(serde_json::Value::as_str) {
                builder = builder.published_date(first_ten(date));
            }
            if category == "news" {
                builder = builder.thumbnail(json_str(item, "thumbnail.src"));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// `page_age` is an ISO-8601 timestamp; only the date part is kept.
fn first_ten(value: &str) -> String {
    value.chars().take(10).collect()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "web": {"results": [
        {"url": "https://example.com/rust", "title": "Rust <b>lang</b>",
         "description": "A systems language", "page_age": "2024-04-01T10:00:00Z"}
      ]}
    }"#;

    fn engine() -> Arc<Fireball> {
        Fireball::from_config(&EngineConfig::new(
            "fireball",
            "fireball",
            "fb",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_web_results() {
        let response = EngineResponse::new(
            200,
            "https://fireball.com/getResults/",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust lang");
                assert_eq!(result.url, "https://example.com/rust");
                assert_eq!(result.published_date.as_deref(), Some("2024-04-01"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_request_with_a_referer() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.starts_with("https://fireball.com/getResults/?"));
        assert_eq!(
            request
                .headers
                .get("referer")
                .map(|value| value.to_str().unwrap()),
            Some("https://fireball.com/search?f=web&q=rust")
        );
    }
}
