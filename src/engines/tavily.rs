//! Tavily Search API engine.
//!
//! A licensed web search API for agents. The API key is required; the
//! account's terms govern automated use. See docs/audit/2026-11-05/.
// engine-id: tavily

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "tv",
    paging: false,
    time_range_support: true,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 6.0,
};

const BASE_URL: &str = "https://api.tavily.com/search";

/// Tavily, a licensed web search API.
pub struct Tavily {
    meta: EngineMeta,
}

impl Tavily {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let key = config
            .api_key
            .as_deref()
            .or_else(|| config.opt_str("api_key"));
        if key.map(str::trim).unwrap_or_default().is_empty() {
            return Err("tavily requires an api_key".to_string());
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn api_key(&self) -> Result<&str, EngineError> {
        let key = self
            .meta
            .api_key
            .as_deref()
            .or_else(|| self.meta.opt_str("api_key"));
        key.map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| EngineError::Message("tavily requires an api_key".into()))
    }
}

impl Engine for Tavily {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let key = self.api_key()?;
        let body = serde_json::json!({
            "query": query.query_text(),
            "max_results": self.meta.opt_i64("max_results").unwrap_or(10),
            "search_depth": self.meta.opt_str("search_depth").unwrap_or("basic"),
        });
        Ok(EngineRequest::post(BASE_URL)
            .header("Authorization", format!("Bearer {key}").as_str())
            .header("Accept", "application/json")
            .json_body(body))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            if !url.starts_with("https://") && !url.starts_with("http://") {
                continue;
            }
            let title = strip_tags(&json_str(item, "title"));
            if title.is_empty() {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(strip_tags(&json_str(item, "content")))
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    fn engine() -> Arc<Tavily> {
        let mut config = EngineConfig::new("tavily", "tavily", "tv", &["general", "web"]);
        config.api_key = Some("KEY".to_string());
        Tavily::from_config(&config).expect("tavily engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_api_request() {
        let request = engine()
            .build_request(&query("rust async"), 1)
            .expect("request");
        assert_eq!(request.url, BASE_URL);
        let body = request.json.as_ref().expect("json body");
        assert_eq!(body["query"].as_str(), Some("rust async"));
        assert_eq!(body["max_results"].as_i64(), Some(10));
        assert_eq!(
            request
                .headers
                .get("authorization")
                .and_then(|v| v.to_str().ok()),
            Some("Bearer KEY")
        );
    }

    #[test]
    fn parses_the_results() {
        let body = r##"{"results": [
          {"url": "https://www.rust-lang.org/", "title": "Rust <b>Programming</b>", "content": "A language."},
          {"url": "nope", "title": "bad url"},
          {"url": "https://x.example/", "title": ""}
        ]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).expect("items");
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust Programming");
                assert_eq!(result.content, "A language.");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn api_key_is_required() {
        let config = EngineConfig::new("tavily", "tavily", "tv", &["general", "web"]);
        assert!(Tavily::from_config(&config).is_err());
    }
}
