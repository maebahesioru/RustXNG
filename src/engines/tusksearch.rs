//! Tusksearch engine, ported from `searx/engines/tusksearch.py` (web search).
//!
//! Upstream supports the categories `web`, `images`, `videos` and `news`
//! through the `tusk_categ` setting; this port implements the main `web` variant
//! and reads the category from the `search_type` config key.
//!
//! Upstream fetches `/revcontent/embed.js` for every request to decode a
//! one-shot `x-sid-<uuid>` header. The frozen `build_request` interface cannot
//! perform that extra round trip, so the header name and value are read from the
//! `x_sid_header` / `x_sid_value` config options. Tusksearch's result items use
//! Brave's schema, so the field mapping follows `searx/engines/brave.py`.
// engine-id: tusksearch

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "tsk",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const API_URL: &str = "https://api.tusksearch.com";

/// Tusksearch, a search engine whose results come (partially) from Brave.
pub struct Tusksearch {
    meta: EngineMeta,
    categ: String,
}

impl Tusksearch {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config.opt_str("search_type").unwrap_or("web").to_string();
        if !matches!(categ.as_str(), "web" | "images" | "videos" | "news") {
            return Err(format!("tusksearch: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for Tusksearch {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let endpoint = if self.categ == "images" {
            "Image"
        } else {
            "Web"
        };
        let pairs = vec![
            ("q", query.query_text()),
            ("p", pageno.max(1).to_string()),
            // Political direction: "left", "center" or "right".
            ("l", "center".to_string()),
        ];
        let mut request = EngineRequest::get(format!(
            "{API_URL}/Search/{endpoint}?{}",
            query_string(&pairs)
        ))
        .header("Referer", "https://tusksearch.com/")
        .header("Origin", "https://tusksearch.com")
        .header("Sec-Fetch-Site", "same-site")
        .header("Sec-Fetch-Mode", "cors")
        .header("Sec-Fetch-Dest", "empty")
        .header("Accept", "application/json");
        // Upstream sends a random location with every request.
        request.set_header("x-lon", &format!("{:.4}", pseudo_random()));
        request.set_header("x-lat", &format!("{:.4}", pseudo_random()));
        if let Some(header) = self.meta.opt_str("x_sid_header") {
            request.set_header(header, self.meta.opt_str("x_sid_value").unwrap_or_default());
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(root) = json_path(&json, "results") else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        match self.categ.as_str() {
            "images" => {
                if let Some(items) = root.as_array() {
                    for item in items {
                        results.push(image_result(item));
                    }
                }
            }
            "news" => {
                if let Some(items) =
                    json_path(root, "news.results").and_then(serde_json::Value::as_array)
                {
                    for item in items {
                        results.push(news_result(item));
                    }
                }
            }
            "videos" => {
                if let Some(items) =
                    json_path(root, "videos.results").and_then(serde_json::Value::as_array)
                {
                    for item in items {
                        results.push(video_result(item));
                    }
                }
            }
            _ => {
                if let Some(items) =
                    json_path(root, "web.results").and_then(serde_json::Value::as_array)
                {
                    for item in items {
                        results.push(search_result(item));
                    }
                }
            }
        }
        Ok(results)
    }
}

/// A deterministic stand-in for upstream's random location value.
fn pseudo_random() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    (f64::from(nanos % 900_000)) / 10_000.0 - 45.0
}

/// `searx/engines/brave.py:parse_search_result`
fn search_result(item: &serde_json::Value) -> EngineItem {
    let title = json_str(item, "title");
    let url = json_str(item, "url");
    let thumbnail = json_str(item, "thumbnail.src");
    let mut builder =
        ResultBuilder::new(url, title).content(strip_tags(&json_str(item, "description")));
    if json_path(item, "thumbnail.logo").is_none_or(|logo| logo.is_null()) {
        builder = builder.thumbnail(thumbnail);
    }
    if let Some(date) = json_path(item, "page_age").and_then(serde_json::Value::as_i64) {
        builder = builder.published_date(format!("{date}"));
    }
    builder.build()
}

fn news_result(item: &serde_json::Value) -> EngineItem {
    let mut builder = ResultBuilder::new(json_str(item, "url"), json_str(item, "title"))
        .content(strip_tags(&json_str(item, "description")))
        .thumbnail(json_str(item, "thumbnail.src"));
    if !json_str(item, "age").is_empty() {
        builder = builder.published_date(json_str(item, "age"));
    }
    builder.build()
}

fn image_result(item: &serde_json::Value) -> EngineItem {
    let width = json_str(item, "properties.width");
    let height = json_str(item, "properties.height");
    let resolution = if width.is_empty() || height.is_empty() {
        String::new()
    } else {
        format!("{width}x{height}")
    };
    ResultBuilder::new(json_str(item, "url"), json_str(item, "title"))
        .image(json_str(item, "properties.url"))
        .thumbnail(json_str(item, "thumbnail.src"))
        .template(crate::results::Template::Images)
        .metadata("resolution", resolution)
        .metadata("source", json_str(item, "source"))
        .build()
}

/// `searx/engines/brave.py:parse_video_result`
#[allow(dead_code)]
fn video_result(item: &serde_json::Value) -> EngineItem {
    let mut builder = ResultBuilder::new(json_str(item, "url"), json_str(item, "title"))
        .content(strip_tags(&json_str(item, "description")))
        .thumbnail(json_str(item, "thumbnail.src"))
        .template(crate::results::Template::Videos)
        .metadata("length", json_str(item, "video.duration"))
        .metadata("views", json_str(item, "video.views"));
    if !json_str(item, "age").is_empty() {
        builder = builder.published_date(json_str(item, "age"));
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "results": {
        "web": {"results": [
          {"title": "Rust lang", "url": "https://www.rust-lang.org/",
           "description": "A <b>fast</b> language", "page_age": 1712000000,
           "thumbnail": {"src": "https://img.example.com/a.png"}}
        ]}
      }
    }"#;

    fn engine() -> Arc<Tusksearch> {
        Tusksearch::from_config(&EngineConfig::new(
            "tusksearch",
            "tusksearch",
            "tsk",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_web_results() {
        let response = EngineResponse::new(
            200,
            "https://api.tusksearch.com/Search/Web",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust lang");
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.content, "A fast language");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_a_paged_request_to_the_web_endpoint() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://api.tusksearch.com/Search/Web?")
        );
        assert!(request.url.contains("p=2"));
        assert!(request.url.contains("l=center"));
        assert_eq!(
            request
                .headers
                .get("referer")
                .map(|value| value.to_str().unwrap()),
            Some("https://tusksearch.com/")
        );
    }
}
