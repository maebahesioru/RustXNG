//! Invidious engine, ported from `searx/engines/invidious.py`.
//!
//! Invidious no longer offers a public API, so `base_url` should point at a
//! self-hosted instance; a public default is kept so the engine still builds.
// engine-id: invidious

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos", "music"],
    shortcut: "iv",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

/// A "sensible public default"; upstream requires a self-hosted instance.
const BASE_URL: &str = "https://yewtu.be";

/// An Invidious instance.
pub struct Invidious {
    meta: EngineMeta,
}

impl Invidious {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Invidious {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("page", pageno.max(1).to_string()),
        ];
        let mut url = format!("{}/api/v1/search?{}", self.base_url(), query_string(&pairs));
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "today",
                TimeRange::Week => "week",
                TimeRange::Month => "month",
                TimeRange::Year => "year",
            };
            url.push_str(&format!("&date={value}"));
        }
        let language = query.engine_language();
        if language != "all" {
            let parts: Vec<&str> = language.split('-').collect();
            if parts.len() == 2 {
                url.push_str(&format!("&range={}", parts[1]));
            }
        }
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json.as_array() else {
            return Ok(Vec::new());
        };
        let base = self.base_url();
        let mut results = Vec::new();
        for item in items {
            if json_str(item, "type") != "video" {
                continue;
            }
            let video_id = json_str(item, "videoId");
            let title = json_str(item, "title");
            if video_id.is_empty() || title.is_empty() {
                continue;
            }
            let mut thumbnail = json_path(item, "videoThumbnails")
                .and_then(serde_json::Value::as_array)
                .and_then(|thumbnails| {
                    thumbnails
                        .iter()
                        .find(|thumbnail| json_str(thumbnail, "quality") == "sddefault")
                })
                .map(|thumbnail| json_str(thumbnail, "url"))
                .unwrap_or_default();
            if !thumbnail.is_empty() && !is_absolute(&thumbnail) {
                thumbnail = format!("{base}{thumbnail}");
            }
            let published = json_path(item, "published")
                .and_then(serde_json::Value::as_i64)
                .and_then(|seconds| chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0))
                .map(|date| date.to_rfc3339())
                .unwrap_or_default();
            let length = json_path(item, "lengthSeconds")
                .and_then(serde_json::Value::as_i64)
                .map(format_duration)
                .unwrap_or_default();
            let views = json_path(item, "viewCount")
                .and_then(serde_json::Value::as_f64)
                .map(humanize_number)
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(format!("{base}/watch?v={video_id}"), title)
                    .content(json_str(item, "description"))
                    .author(json_str(item, "author"))
                    .thumbnail(thumbnail)
                    .published_date(published)
                    .template(Template::Videos)
                    .metadata("length", length)
                    .metadata("views", views)
                    .metadata("iframe_src", format!("{base}/embed/{video_id}"))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_absolute(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
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

    const FIXTURE: &str = r#"[
      {
        "type": "video",
        "videoId": "dQw4w9WgXcQ",
        "title": "A video",
        "description": "Description",
        "author": "Someone",
        "viewCount": 12345,
        "lengthSeconds": 125,
        "published": 1700000000,
        "videoThumbnails": [
          {"quality": "sddefault", "url": "/vi/dQw4w9WgXcQ/sddefault.jpg"}
        ]
      }
    ]"#;

    fn engine() -> Arc<Invidious> {
        Invidious::from_config(&EngineConfig::new(
            "invidious",
            "invidious",
            "iv",
            &["videos", "music"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_videos() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("video", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A video");
                assert_eq!(result.url, "https://yewtu.be/watch?v=dQw4w9WgXcQ");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://yewtu.be/vi/dQw4w9WgXcQ/sddefault.jpg")
                );
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("02:05")
                );
                assert_eq!(
                    result.metadata.get("views").map(String::as_str),
                    Some("12K")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_request() {
        let mut query = Query::parse("video", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.starts_with("https://yewtu.be/api/v1/search?"));
        assert!(request.url.contains("page=2"));
        assert!(request.url.contains("date=week"));
    }
}
