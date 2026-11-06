//! Meilisearch engine, ported from searx/engines/meilisearch.py.
//!
//! Meilisearch is a self-hosted search backend; the engine queries one index
//! of an instance configured with base_url and index.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

// engine-id: meilisearch

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "mes",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// A Meilisearch instance.
pub struct Meilisearch {
    meta: EngineMeta,
    index: String,
    facet_filters: Vec<String>,
}

impl Meilisearch {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let index = meta.opt_str("index").unwrap_or_default().to_string();
        if index.is_empty() {
            return Err("meilisearch requires the index option".into());
        }
        let facet_filters = meta
            .opt_str("facet_filters")
            .map(|value| {
                value
                    .split(',')
                    .map(|filter| filter.trim().to_string())
                    .filter(|filter| !filter.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Ok(Arc::new(Self {
            meta,
            index,
            facet_filters,
        }))
    }
}

impl Engine for Meilisearch {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .ok_or_else(|| EngineError::Message("meilisearch requires base_url".into()))?
            .trim_end_matches('/');
        let mut payload = serde_json::json!({
            "q": query.query_text(),
            "offset": (pageno.saturating_sub(1)) * 10,
            "limit": 10,
        });
        if !self.facet_filters.is_empty() {
            payload["facetFilters"] = serde_json::json!(self.facet_filters);
        }
        let mut request = EngineRequest::post(format!("{base}/indexes/{}/search", self.index))
            .json_body(payload)
            .header("Content-Type", "application/json");
        if let Some(key) = self.meta.api_key.as_deref() {
            if !key.is_empty() {
                request = request.header("Authorization", &format!("Bearer {key}"));
            }
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(hits) = json_path(&json, "hits").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for hit in hits {
            // Meilisearch returns arbitrary documents: pick the usual fields.
            let url = ["url", "link", "href"]
                .iter()
                .map(|key| json_str(hit, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            let title = ["title", "name", "heading"]
                .iter()
                .map(|key| json_str(hit, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let content = ["content", "description", "body", "text"]
                .iter()
                .map(|key| json_str(hit, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, strip_tags(&title))
                    .content(strip_tags(&content))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Meilisearch> {
        let mut config = EngineConfig::new("meilisearch", "meilisearch", "mes", &["general"]);
        config.base_url = Some("http://localhost:7700".to_string());
        config
            .extra
            .insert("index".into(), toml::Value::String("pages".into()));
        Meilisearch::from_config(&config).expect("engine")
    }

    #[test]
    fn requires_an_index() {
        let config = EngineConfig::new("meilisearch", "meilisearch", "mes", &["general"]);
        assert!(Meilisearch::from_config(&config).is_err());
    }

    #[test]
    fn posts_to_the_index_search_endpoint() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(request.url, "http://localhost:7700/indexes/pages/search");
        let body = request.json.clone().unwrap();
        assert_eq!(body["q"], serde_json::json!("rust"));
        assert_eq!(body["offset"], serde_json::json!(10));
    }

    #[test]
    fn parses_the_hits() {
        let body = r##"{"hits":[
            {"url":"https://example.test/a","title":"A","content":"About A"},
            {"url":"","title":"no url"},
            {"url":"https://example.test/b","name":"B"}
        ]}"##;
        let response = EngineResponse::new(200, "http://localhost:7700", body.as_bytes().to_vec());
        let query = Query::parse("a", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.test/a");
                assert_eq!(result.title, "A");
                assert_eq!(result.content, "About A");
            }
            _ => panic!("expected a result"),
        }
    }
}
