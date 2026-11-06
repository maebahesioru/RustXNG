//! Niconico engine, ported from searx/engines/niconico.py.
//!
//! The search page became a client rendered application, so the HTML listing no
//! longer carries any watch link. This port queries the public Snapshot Search
//! API of nicovideo.jp instead, which needs no credentials.
// engine-id: niconico

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
    shortcut: "nico",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.nicovideo.jp";
const EMBED_URL: &str = "https://embed.nicovideo.jp";
const API_URL: &str = "https://snapshot.search.nicovideo.jp/api/v2/snapshot/video/contents/search";
const PAGE_SIZE: u32 = 20;

/// The Niconico video search.
pub struct Niconico {
    meta: EngineMeta,
}

impl Niconico {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Niconico {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pageno = pageno.max(1);
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text().to_string()),
            ("targets", "title,description,tags".to_string()),
            (
                "fields",
                "contentId,title,description,thumbnailUrl,lengthSeconds,startTime".to_string(),
            ),
            ("_limit", PAGE_SIZE.to_string()),
            ("_offset", ((pageno - 1) * PAGE_SIZE).to_string()),
            ("_context", "rustxng".to_string()),
        ];
        match query.time_range {
            Some(range) => {
                let days = match range {
                    TimeRange::Day => 1,
                    TimeRange::Week => 7,
                    TimeRange::Month => 30,
                    TimeRange::Year => 365,
                };
                let start = chrono::Utc::now()
                    .date_naive()
                    .checked_sub_days(chrono::Days::new(days))
                    .map(|date| date.format("%Y-%m-%dT00:00:00+09:00").to_string())
                    .unwrap_or_default();
                pairs.push(("filters[startTime][gte]", start));
                pairs.push(("_sort", "+startTime".to_string()));
            }
            None => pairs.push(("_sort", "-viewCounter".to_string())),
        }
        Ok(EngineRequest::get(format!(
            "{API_URL}?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let Ok(document) = serde_json::from_str::<serde_json::Value>(&body) else {
            return Ok(Vec::new());
        };
        let Some(entries) = json_path(&document, "data").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in entries {
            let content_id = json_str(entry, "contentId");
            let title = strip_tags(&json_str(entry, "title"));
            if content_id.is_empty() || title.is_empty() {
                continue;
            }
            let url = format!("{BASE_URL}/watch/{content_id}");
            let mut builder = ResultBuilder::new(url, title)
                .template(Template::Videos)
                .content(strip_tags(&json_str(entry, "description")))
                .metadata("iframe_src", format!("{EMBED_URL}/watch/{content_id}"));
            let thumbnail = json_str(entry, "thumbnailUrl");
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            if let Some(length) = length(entry) {
                builder = builder.metadata("length", length);
            }
            if let Some(date) = published_date(&json_str(entry, "startTime")) {
                builder = builder.published_date(date);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The playing time, the API reports it in seconds.
fn length(entry: &serde_json::Value) -> Option<String> {
    let seconds = entry.get("lengthSeconds")?.as_u64()?;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        Some(format!("{hours}:{minutes:02}:{seconds:02}"))
    } else {
        Some(format!("{minutes:02}:{seconds:02}"))
    }
}

/// The upload time, the API reports it as an ISO 8601 timestamp.
fn published_date(value: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|date| date.naive_local().format("%Y-%m-%dT%H:%M:%S").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"meta":{"status":200,"totalCount":1},"data":[
      {"contentId":"sm12345","title":"Rust tutorial",
       "description":"A <br />description",
       "thumbnailUrl":"https://nicovideo.cdn.nimg.jp/thumb/sm12345",
       "lengthSeconds":330,"startTime":"2024-05-01T12:30:00+09:00"},
      {"contentId":"","title":"dropped"}
    ]}"##;

    fn engine() -> Arc<Niconico> {
        Niconico::from_config(&EngineConfig::new(
            "niconico",
            "niconico",
            "nico",
            &["videos"],
        ))
        .unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_api_payload() {
        let response = EngineResponse::new(200, API_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust tutorial");
                assert_eq!(result.url, "https://www.nicovideo.jp/watch/sm12345");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.content, "A description");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://nicovideo.cdn.nimg.jp/thumb/sm12345")
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-05-01T12:30:00")
                );
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("05:30")
                );
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://embed.nicovideo.jp/watch/sm12345")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn a_missing_payload_is_not_an_error() {
        for body in [
            "<html><body></body></html>".as_bytes().to_vec(),
            br#"{"meta":{"status":200}}"#.to_vec(),
        ] {
            let response = EngineResponse::new(200, API_URL, body);
            assert!(engine().parse(&response, &query()).unwrap().is_empty());
        }
    }

    #[test]
    fn paging_and_time_range_are_forwarded() {
        let request = engine().build_request(&query(), 3).unwrap();
        assert!(request.url.starts_with(API_URL));
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("_offset=40"));
        assert!(request.url.contains("_sort=-viewCounter"));

        let mut ranged = query();
        ranged.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&ranged, 1).unwrap();
        assert!(request.url.contains("_sort=%2BstartTime"));
        assert!(request.url.contains("filters%5BstartTime%5D%5Bgte%5D="));
    }

    #[test]
    fn formats_long_videos() {
        let entry: serde_json::Value = serde_json::from_str(r#"{"lengthSeconds":3725}"#).unwrap();
        assert_eq!(length(&entry).as_deref(), Some("1:02:05"));
    }
}
