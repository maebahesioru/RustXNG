//! Findborg engine, ported from `searx/engines/findborg.py` (web search).
//!
//! Upstream supports the categories `search`, `images`, `videos` and `news`
//! through the `findborg_categ` setting; this port implements the main `search`
//! variant and reads the category from the `search_type` config key.
//! Findborg's result items use Brave's schema, so the field mapping follows
//! `searx/engines/brave.py`.
// engine-id: findborg

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "fbg",
    paging: false,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.findborg.com";

/// Findborg, a small search engine that gets its results from Brave.
pub struct Findborg {
    meta: EngineMeta,
    categ: String,
}

impl Findborg {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config
            .opt_str("search_type")
            .unwrap_or("search")
            .to_string();
        if !matches!(categ.as_str(), "search" | "images" | "videos" | "news") {
            return Err(format!("findborg: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for Findborg {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> =
            vec![("q", query.query_text()), ("type", self.categ.clone())];
        if let Some(range) = query.time_range {
            pairs.push(("time", range.as_str().to_string()));
        }
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/apis/proxy.php?{}",
            query_string(&pairs)
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(data) = json_path(&json, "data") else {
            return Ok(Vec::new());
        };
        let path = if self.categ == "search" {
            "web.results"
        } else {
            "results"
        };
        let Some(items) = json_path(data, path).and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            match self.categ.as_str() {
                "search" => results.push(search_result(item)),
                "news" => results.push(news_result(item)),
                "videos" => results.push(video_result(item)),
                _ => results.push(image_result(item)),
            }
        }
        Ok(results)
    }
}

/// `searx/engines/brave.py:parse_search_result`
fn search_result(item: &serde_json::Value) -> EngineItem {
    let mut builder = ResultBuilder::new(json_str(item, "url"), json_str(item, "title"))
        .content(strip_tags(&json_str(item, "description")));
    if json_path(item, "thumbnail.logo").is_none_or(|logo| logo.is_null()) {
        builder = builder.thumbnail(json_str(item, "thumbnail.src"));
    }
    if let Some(age) = json_path(item, "page_age").and_then(serde_json::Value::as_str) {
        builder = builder.published_date(take_first(age, 10));
    }
    builder.build()
}

/// `searx/engines/brave.py:parse_news_result`
fn news_result(item: &serde_json::Value) -> EngineItem {
    let mut builder = ResultBuilder::new(json_str(item, "url"), json_str(item, "title"))
        .content(strip_tags(&json_str(item, "description")))
        .thumbnail(json_str(item, "thumbnail.src"));
    if !json_str(item, "age").is_empty() {
        builder = builder.published_date(json_str(item, "age"));
    }
    builder.build()
}

/// `searx/engines/brave.py:parse_image_result`
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

fn take_first(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::TimeRange;

    const FIXTURE: &str = r#"{
      "data": {
        "web": {"results": [
          {"title": "Rust lang", "url": "https://www.rust-lang.org/",
           "description": "A <b>fast</b> language", "page_age": "2024-04-01T10:00:00Z",
           "thumbnail": {"src": "https://img.example.com/a.png"}}
        ]}
      }
    }"#;

    fn engine() -> Arc<Findborg> {
        Findborg::from_config(&EngineConfig::new(
            "findborg",
            "findborg",
            "fbg",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_search_results() {
        let response = EngineResponse::new(
            200,
            "https://www.findborg.com/apis/proxy.php",
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
                assert_eq!(result.published_date.as_deref(), Some("2024-04-01"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_request_with_the_category_and_time_range() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Month);
        let request = engine().build_request(&query, 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://www.findborg.com/apis/proxy.php?")
        );
        assert!(request.url.contains("type=search"));
        assert!(request.url.contains("time=month"));
    }
}
