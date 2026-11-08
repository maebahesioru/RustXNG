//! north.rip, a Japanese microblog (ActivityPub-ish SNS) with a public search
//! API at `/api/search`.
//!
//! The search response carries no permalink, so the canonical address is built
//! from the author handle and the post id: `/:handle/status/:id` answers 200
//! (measured), while `/p/:id`, `/post/:id` and `/note/:id` are all 404.
// engine-id: north_rip

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["social media"],
    shortcut: "nrt",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 8.0,
};

const BASE_URL: &str = "https://north.rip";

/// A north.rip instance.
pub struct NorthRip {
    meta: EngineMeta,
}

impl NorthRip {
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

impl Engine for NorthRip {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![("q", query.query_text())];
        Ok(EngineRequest::get(format!(
            "{}/api/search?{}",
            self.base_url(),
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
        let Some(items) = json_path(&json, "items").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let base = self.base_url();
        let mut results = Vec::new();
        for item in items {
            if json_path(item, "deleted").and_then(|value| value.as_bool()) == Some(true) {
                continue;
            }
            let id = json_str(item, "id");
            let handle = json_str(item, "author.handle");
            let text = json_str(item, "text");
            if id.is_empty() || handle.is_empty() || text.is_empty() {
                continue;
            }
            let url = format!("{base}/{handle}/status/{id}");
            let name = json_str(item, "author.name");
            let title = if name.is_empty() || name == handle {
                format!("@{handle}")
            } else {
                format!("{name} (@{handle})")
            };
            let mut builder = ResultBuilder::new(url, title).content(text);
            let avatar = json_str(item, "author.avatarUrl");
            if !avatar.is_empty() {
                builder = builder.thumbnail(avatar);
            }
            // The API sends ISO 8601 with milliseconds; ResultBuilder wants an RFC 3339
            // stamp, so the milliseconds are dropped rather than truncating the string.
            let created = json_str(item, "createdAt");
            if let Ok(stamp) = chrono::DateTime::parse_from_rfc3339(&created) {
                builder = builder.published_date(stamp.to_rfc3339());
            }
            let media = json_path(item, "media")
                .and_then(|value| value.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            if media > 0 {
                builder = builder.metadata("media", media.to_string());
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r#"{"items":[
      {"id":"2114282935879206885","text":"ストアから落とせます","createdAt":"2026-10-25T09:08:28.047Z",
       "author":{"id":"a1","handle":"uirusuniki","name":"uirusuniki","avatarUrl":"https://north.rip/avatar/a1.png"},
       "media":[],"deleted":false},
      {"id":"2114282935879206886","text":"","author":{"handle":"empty"},"deleted":false},
      {"id":"2114282935879206887","text":"gone","author":{"handle":"ghost"},"deleted":true}
    ],"nextCursor":null}"#;

    fn engine() -> Arc<NorthRip> {
        let config = EngineConfig::new("north_rip", "north_rip", "nrt", &["social media"]);
        NorthRip::from_config(&config).unwrap()
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        assert_eq!(
            engine().build_request(&query, 1).unwrap().url,
            "https://north.rip/api/search?q=rust"
        );
    }

    #[test]
    fn parses_posts_into_status_permalinks() {
        let response = EngineResponse::new(
            200,
            "https://north.rip/api/search",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result");
        };
        assert_eq!(
            result.url,
            "https://north.rip/uirusuniki/status/2114282935879206885"
        );
        assert_eq!(result.title, "@uirusuniki");
        assert_eq!(result.content, "ストアから落とせます");
        assert!(result
            .published_date
            .as_deref()
            .is_some_and(|date| date.starts_with("2026-10-25")));
    }

    #[test]
    fn an_empty_response_is_not_an_error() {
        let response = EngineResponse::new(
            200,
            "https://north.rip/api/search",
            br#"{"items":[],"nextCursor":null}"#.to_vec(),
        );
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }
}
