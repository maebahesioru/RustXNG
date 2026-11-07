//! Piped engine, ported from `searx/engines/piped.py`.
//!
//! Piped is a privacy friendly YouTube frontend.  The *nextpage* based paging
//! of upstream cannot be ported: `build_request` has no access to the engine
//! data returned by the previous response, so the plain `/search` endpoint is
//! always used.
// engine-id: piped

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "ppd",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 3.0,
};

const BACKEND_URL: &str = "https://pipedapi.kavin.rocks";
const FRONTEND_URL: &str = "https://piped.video";

/// A Piped backend/frontend pair.
pub struct Piped {
    meta: EngineMeta,
}

impl Piped {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn backend_url(&self) -> String {
        self.meta
            .opt_str("backend_url")
            .map(str::to_string)
            .or_else(|| self.meta.base_url.clone())
            .unwrap_or_else(|| BACKEND_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }

    fn frontend_url(&self) -> String {
        self.meta
            .opt_str("frontend_url")
            .unwrap_or(FRONTEND_URL)
            .trim_end_matches('/')
            .to_string()
    }

    fn filter(&self) -> &str {
        self.meta.opt_str("piped_filter").unwrap_or("all")
    }
}

impl Engine for Piped {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("filter", self.filter().to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/search?{}",
            self.backend_url(),
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
        let Some(items) = json_path(&json, "items").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let frontend = self.frontend_url();
        let filter = self.filter();
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "title");
            let path = json_str(item, "url");
            if title.is_empty() || path.is_empty() {
                continue;
            }
            let uploaded = json_path(item, "uploaded")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(-1);
            let published = if uploaded == -1 {
                String::new()
            } else {
                chrono::DateTime::<chrono::Utc>::from_timestamp(uploaded / 1000, 0)
                    .map(|date| date.to_rfc3339())
                    .unwrap_or_default()
            };
            let views = json_path(item, "views")
                .and_then(serde_json::Value::as_f64)
                .map(humanize_number)
                .unwrap_or_default();
            let mut builder = ResultBuilder::new(format!("{frontend}{path}"), title)
                .published_date(published)
                .metadata("iframe_src", format!("{frontend}/embed{path}"))
                .metadata("views", views);
            let duration = json_path(item, "duration")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            if duration > 0 {
                builder = builder.metadata("length", format_duration(duration));
            }
            match filter {
                "videos" => {
                    let description = json_str(item, "shortDescription");
                    builder = builder
                        .template(Template::Videos)
                        .content(description)
                        .thumbnail(json_str(item, "thumbnail"));
                }
                "music_songs" => {
                    builder = builder
                        .content(json_str(item, "uploaderName"))
                        .thumbnail(json_str(item, "thumbnail"));
                }
                _ => {}
            }
            results.push(builder.build());
        }
        if let Some(nextpage) = json_path(&json, "nextpage").and_then(serde_json::Value::as_str) {
            if !nextpage.is_empty() {
                results.push(EngineItem::EngineData {
                    key: "nextpage".to_string(),
                    value: nextpage.to_string(),
                });
            }
        }
        Ok(results)
    }
}

fn format_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Upstream's `humanize_number` (base 1000, no decimals).
fn humanize_number(mut size: f64) -> String {
    const SUFFIX: [&str; 5] = ["", "K", "M", "B", "T"];
    let mut position = 0usize;
    while size > 1000.0 && position < SUFFIX.len() {
        position += 1;
        size /= 1000.0;
    }
    let position = position.min(SUFFIX.len() - 1);
    format!("{size:.0}{}", SUFFIX[position])
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "items": [
        {
          "url": "/watch?v=abc123",
          "title": "A video",
          "uploaderName": "A channel",
          "shortDescription": "A description",
          "thumbnail": "https://pipedproxy.example/thumb.jpg",
          "duration": 125,
          "views": 12345,
          "uploaded": 1700000000000
        }
      ],
      "nextpage": "abc"
    }"#;

    fn engine() -> Arc<Piped> {
        let mut config = EngineConfig::new("piped", "piped", "ppd", &["videos"]);
        config.extra.insert(
            "piped_filter".to_string(),
            toml::Value::String("videos".to_string()),
        );
        Piped::from_config(&config).unwrap()
    }

    #[test]
    fn parses_videos() {
        let response = EngineResponse::new(200, BACKEND_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("video", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        // one result plus the nextpage engine data
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A video");
                assert_eq!(result.url, "https://piped.video/watch?v=abc123");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.content, "A description");
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://piped.video/embed/watch?v=abc123")
                );
                assert_eq!(
                    result.metadata.get("views").map(String::as_str),
                    Some("12K")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::EngineData { key, value } => {
                assert_eq!(key, "nextpage");
                assert_eq!(value, "abc");
            }
            _ => panic!("expected engine data"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("video", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(
            request.url,
            "https://pipedapi.kavin.rocks/search?q=video&filter=videos"
        );
    }
}
