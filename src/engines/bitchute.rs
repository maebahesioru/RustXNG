//! Bitchute engine, ported from `searx/engines/bitchute.py`.

// engine-id: bitchute

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
    shortcut: "bit",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.bitchute.com/api/beta/search2/videos";

/// The public web client sends a client-generated 28 character token in
/// the X-Service-Info header; any token of that length is accepted, so a fixed
/// one keeps the request deterministic.
const SERVICE_INFO: &str = "rustxngserviceinfo0000000001";
const RESULTS_PER_PAGE: u32 = 20;

/// Bitchute video search.
pub struct Bitchute {
    meta: EngineMeta,
}

impl Bitchute {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Bitchute {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let body = serde_json::json!({
            "offset": (pageno.saturating_sub(1)) * RESULTS_PER_PAGE,
            "limit": RESULTS_PER_PAGE,
            "query": query.query_text(),
            "sensitivity_id": "normal",
            "sort": "new",
        });
        Ok(EngineRequest::post(BASE_URL)
            .json_body(body)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("X-Service-Info", SERVICE_INFO))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "videos").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let video_id = json_str(item, "video_id");
            let title = json_str(item, "video_name");
            if video_id.is_empty() || title.is_empty() {
                continue;
            }
            let url = format!("https://www.bitchute.com/video/{video_id}");
            let mut builder = ResultBuilder::new(url, title)
                .content(strip_tags(&json_str(item, "description")))
                .thumbnail(json_str(item, "thumbnail_url"))
                .published_date(json_str(item, "date_published"))
                .template(Template::Videos)
                .metadata(
                    "iframe_src",
                    format!("https://www.bitchute.com/embed/{video_id}"),
                );
            if let Some(author) = json_path(item, "channel.channel_name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
            {
                builder = builder.author(author);
            }
            let length = json_str(item, "duration");
            if !length.is_empty() {
                builder = builder.metadata("length", length);
            }
            let views = json_str(item, "view_count");
            if !views.is_empty() {
                builder = builder.metadata("views", views);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Bitchute> {
        Bitchute::from_config(&EngineConfig::new(
            "bitchute",
            "bitchute",
            "bit",
            &["videos"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_videos_json() {
        let body = r#"{"videos":[
          {"video_id":"v1","video_name":"A Video","description":"<p>Hello</p>",
           "thumbnail_url":"https://img.example.com/t.jpg","date_published":"2024-01-15T10:00:00",
           "duration":"PT5M","view_count":"120","channel":{"channel_name":"Some Channel"}}
        ]}"#;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let items = engine()
            .parse(
                &response,
                &Query::parse("test", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.bitchute.com/video/v1");
                assert_eq!(result.title, "A Video");
                assert_eq!(result.author.as_deref(), Some("Some Channel"));
            }
            _ => panic!("expected a video result"),
        }
    }

    #[test]
    fn builds_a_json_post() {
        let query = Query::parse("x", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(
            request.json.as_ref().and_then(|j| j["offset"].as_i64()),
            Some(20)
        );
    }

    #[test]
    fn sends_the_service_info_token() {
        assert_eq!(SERVICE_INFO.len(), 28);
        let query = Query::parse("x", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(
            request
                .headers
                .get("x-service-info")
                .and_then(|v| v.to_str().ok()),
            Some(SERVICE_INFO)
        );
    }
}
