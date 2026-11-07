//! YouTube API engine, ported from searx/engines/youtube_api.py.
// engine-id: youtube_api

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos", "music"],
    shortcut: "yta",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.googleapis.com/youtube/v3/search";
const WATCH_URL: &str = "https://www.youtube.com/watch?v=";
const EMBED_URL: &str = "https://www.youtube-nocookie.com/embed/";

/// The official YouTube search API, it needs an api_key.
pub struct YouTubeApi {
    meta: EngineMeta,
}

impl YouTubeApi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn endpoint(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
    }

    fn api_key(&self) -> Result<&str, EngineError> {
        self.meta
            .api_key
            .as_deref()
            .or_else(|| self.meta.opt_str("api_key"))
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| EngineError::Message("youtube_api requires an api_key".into()))
    }
}

impl Engine for YouTubeApi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let key = self.api_key()?;
        let pairs: Vec<(&str, String)> = vec![
            ("part", "snippet".to_string()),
            ("q", query.query_text()),
            ("maxResults", "20".to_string()),
            ("key", key.to_string()),
        ];
        let mut url = format!("{}?{}", self.endpoint(), query_string(&pairs));
        let language = locale_language(&query.locale);
        if language != "all" {
            url.push_str(&format!("&relevanceLanguage={language}"));
        }
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        if let Some(message) = json_path(&json, "error.message").and_then(|value| value.as_str()) {
            return Err(EngineError::Message(message.to_string()));
        }
        let Some(items) = json_path(&json, "items").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let video_id = json_str(item, "id.videoId");
            let title = json_str(item, "snippet.title");
            // Channels have no videoId, upstream ignores them.
            if video_id.is_empty() || title.is_empty() {
                continue;
            }
            let url = format!("{WATCH_URL}{video_id}");
            let mut builder = ResultBuilder::new(url, title)
                .template(Template::Videos)
                .content(json_str(item, "snippet.description"))
                .published_date(iso_date(&json_str(item, "snippet.publishedAt")))
                .metadata("iframe_src", format!("{EMBED_URL}{video_id}"));
            let thumbnail = json_str(item, "snippet.thumbnails.high.url");
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream parses publishedAt with dateutil and serialises ISO 8601.
fn iso_date(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|date| date.to_rfc3339())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{
      "kind": "youtube#searchListResponse",
      "items": [
        {
          "id": {"kind": "youtube#video", "videoId": "abc123"},
          "snippet": {
            "publishedAt": "2024-05-01T12:00:00Z",
            "title": "Rust in 100 seconds",
            "description": "A quick intro",
            "thumbnails": {"high": {"url": "https://i.ytimg.com/vi/abc123/hqdefault.jpg"}}
          }
        },
        {
          "id": {"kind": "youtube#channel", "channelId": "chan1"},
          "snippet": {"title": "A channel"}
        }
      ]
    }"##;

    fn engine() -> Arc<YouTubeApi> {
        let mut config = EngineConfig::new("youtube_api", "youtube_api", "yta", &["videos"]);
        config.api_key = Some("KEY".to_string());
        YouTubeApi::from_config(&config).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_video_items() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust in 100 seconds");
                assert_eq!(result.url, "https://www.youtube.com/watch?v=abc123");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.content, "A quick intro");
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-05-01T12:00:00+00:00")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://i.ytimg.com/vi/abc123/hqdefault.jpg")
                );
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://www.youtube-nocookie.com/embed/abc123")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn request_needs_a_key_and_forwards_the_language() {
        let mut query = query();
        query.locale = "ja-JP".to_string();
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(
            request.url,
            "https://www.googleapis.com/youtube/v3/search?part=snippet&q=rust&maxResults=20&key=KEY&relevanceLanguage=ja"
        );
        let without_key = YouTubeApi::from_config(&EngineConfig::new(
            "youtube_api",
            "youtube_api",
            "yta",
            &["videos"],
        ))
        .unwrap();
        assert!(matches!(
            without_key.build_request(&query, 1),
            Err(EngineError::Message(_))
        ));
    }

    #[test]
    fn api_errors_become_engine_errors() {
        let body = b"{\"error\": {\"message\": \"quota exceeded\"}}";
        let response = EngineResponse::new(200, BASE_URL, body.to_vec());
        assert!(matches!(
            engine().parse(&response, &query()),
            Err(EngineError::Message(_))
        ));
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
