//! Peertube engine, ported from `searx/engines/peertube.py`.
//!
//! Peertube and SepiaSearch share the same REST API and result schema; the
//! parsing lives here and is mirrored by the SepiaSearch engine.
// engine-id: peertube

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "ptb",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 6.0,
};

const BASE_URL: &str = "https://peer.tube";

/// A Peertube instance.
pub struct Peertube {
    meta: EngineMeta,
}

impl Peertube {
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

impl Engine for Peertube {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        if query.query_text().is_empty() {
            return Err(EngineError::Message("peertube: empty query".into()));
        }
        let pairs: Vec<(&str, String)> = vec![
            // `searchTarget=search-index` is 400 on instances running an older
            // PeerTube (peer.tube); the default target already searches videos.
            ("search", query.query_text()),
            ("resultType", "videos".to_string()),
            ("start", ((pageno.max(1) - 1) * 10).to_string()),
            ("count", "10".to_string()),
            ("sort", "-match".to_string()),
            ("nsfw", safesearch_value(query.safesearch).to_string()),
        ];
        let mut url = format!(
            "{}/api/v1/search/videos?{}",
            self.base_url(),
            query_string(&pairs)
        );
        let language = locale_language(&query.locale);
        if language != "all" {
            // Upstream appends these unencoded.
            url.push_str(&format!("&languageOneOf[]={language}"));
            url.push_str(&format!("&boostLanguages[]={language}"));
        }
        if let Some(range) = query.time_range {
            if let Some(date) = start_date(range) {
                url.push_str(&format!("&startDate={date}"));
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
        Ok(video_response(&json))
    }
}

/// Upstream `safesearch_table`: 0 "both", 1 and 2 "false".
pub fn safesearch_value(safesearch: u8) -> &'static str {
    if safesearch == 0 { "both" } else { "false" }
}

/// Upstream `time_range_table` as an ISO date.
pub fn start_date(range: TimeRange) -> Option<String> {
    let today = chrono::Utc::now().date_naive();
    let date = match range {
        TimeRange::Day => Some(today),
        TimeRange::Week => today.checked_sub_days(chrono::Days::new(7)),
        TimeRange::Month => today.checked_sub_months(chrono::Months::new(1)),
        TimeRange::Year => today.checked_sub_months(chrono::Months::new(12)),
    };
    date.map(|date| date.format("%Y-%m-%d").to_string())
}

/// The `video_response` shared by Peertube and SepiaSearch.
pub fn video_response(json: &serde_json::Value) -> Vec<EngineItem> {
    let Some(items) = json_path(json, "data").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut results = Vec::new();
    for item in items {
        let url = json_str(item, "url");
        let title = json_str(item, "name");
        if !is_http_url(&url) || title.is_empty() {
            continue;
        }
        let channel = json_str(item, "channel.displayName");
        let account = {
            let name = json_str(item, "channel.name");
            let host = json_str(item, "channel.host");
            if name.is_empty() || host.is_empty() {
                String::new()
            } else {
                format!("{name}@{host}")
            }
        };
        let tags = json_path(item, "tags")
            .and_then(serde_json::Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let length = json_path(item, "duration")
            .and_then(serde_json::Value::as_i64)
            .map(format_duration)
            .unwrap_or_default();
        let views = json_path(item, "views")
            .and_then(serde_json::Value::as_f64)
            .map(humanize_number)
            .unwrap_or_default();
        let thumbnail = {
            let thumbnail = json_str(item, "thumbnailUrl");
            if thumbnail.is_empty() {
                json_str(item, "previewUrl")
            } else {
                thumbnail
            }
        };
        results.push(
            ResultBuilder::new(url, title)
                .content(strip_tags(
                    json_path(item, "description")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                ))
                .author(json_str(item, "account.displayName"))
                .thumbnail(thumbnail)
                .published_date(json_str(item, "publishedAt"))
                .template(Template::Videos)
                .metadata("length", length)
                .metadata("views", views)
                .metadata("iframe_src", json_str(item, "embedUrl"))
                .metadata("channel", channel)
                .metadata("account", account)
                .metadata("tags", tags)
                .build(),
        );
    }
    results
}

fn is_http_url(url: &str) -> bool {
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

    const FIXTURE: &str = r#"{
      "total": 1,
      "data": [
        {
          "url": "https://peer.tube/w/abc123",
          "name": "A peertube video",
          "description": "<p>A description</p>",
          "duration": 125,
          "views": 4200,
          "publishedAt": "2024-01-01T00:00:00.000Z",
          "embedUrl": "https://peer.tube/videos/embed/abc123",
          "thumbnailUrl": "https://peer.tube/thumb.jpg",
          "previewUrl": "https://peer.tube/preview.jpg",
          "account": {"displayName": "An account"},
          "channel": {"displayName": "A channel", "name": "channel", "host": "peer.tube"},
          "tags": ["rust", "video"]
        }
      ]
    }"#;

    fn engine() -> Arc<Peertube> {
        Peertube::from_config(&EngineConfig::new(
            "peertube",
            "peertube",
            "ptb",
            &["videos"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_videos() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A peertube video");
                assert_eq!(result.url, "https://peer.tube/w/abc123");
                assert_eq!(result.content, "A description");
                assert_eq!(result.author.as_deref(), Some("An account"));
                assert_eq!(result.template, Template::Videos);
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("02:05")
                );
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://peer.tube/videos/embed/abc123")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        query.safesearch = 1;
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://peer.tube/api/v1/search/videos?")
        );
        assert!(request.url.contains("start=10"));
        assert!(!request.url.contains("searchTarget"));
        assert!(request.url.contains("nsfw=false"));
        assert!(request.url.contains("&startDate="));
    }
}
