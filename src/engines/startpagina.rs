//! Startpagina engine, ported from `searx/engines/startpagina.py` (web search).
//!
//! Upstream picks the endpoint from `startpagina_categ` (`web`, `images`,
//! `videos`, `news`); this port implements the main `web` variant and reads the
//! category from the `search_type` config key.
// engine-id: startpagina

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "spg",
    paging: true,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const API_URL: &str = "https://search.kompas.services";

/// Startpagina (Kompas), a Dutch search engine backed by Google.
pub struct Startpagina {
    meta: EngineMeta,
    categ: String,
    page_size: i64,
}

impl Startpagina {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config.opt_str("search_type").unwrap_or("web").to_string();
        if !matches!(categ.as_str(), "web" | "images" | "videos" | "news") {
            return Err(format!("startpagina: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            page_size: config.opt_i64("page_size").unwrap_or(10),
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for Startpagina {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("q", query.query_text()),
            ("page_size", self.page_size.to_string()),
            ("page", pageno.max(1).to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{API_URL}/api/v2/search/{}/?{}",
            self.categ,
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
        let mut results = Vec::new();
        if let Some(items) = json_path(&json, "results").and_then(serde_json::Value::as_array) {
            for item in items {
                let title = json_str(item, "title");
                let url = json_str(item, "original_url");
                if title.is_empty() || !is_http_url(&url) {
                    continue;
                }
                results.push(
                    ResultBuilder::new(url, title)
                        .content(json_str(item, "description"))
                        .build(),
                );
            }
        }
        if let Some(related) =
            json_path(&json, "related_searches").and_then(serde_json::Value::as_array)
        {
            for item in related {
                let suggestion = json_str(item, "query");
                if !suggestion.is_empty() {
                    results.push(EngineItem::Suggestion(suggestion));
                }
            }
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

    const FIXTURE: &str = r#"{
      "results": [
        {"title": "Rust taal", "original_url": "https://example.nl/rust",
         "description": "Een systeem programmeertaal"}
      ],
      "related_searches": [{"query": "rust async"}]
    }"#;

    fn engine() -> Arc<Startpagina> {
        Startpagina::from_config(&EngineConfig::new(
            "startpagina",
            "startpagina",
            "spg",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_web_results_and_suggestions() {
        let response = EngineResponse::new(
            200,
            "https://search.kompas.services/",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust taal");
                assert_eq!(result.url, "https://example.nl/rust");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Suggestion(suggestion) => assert_eq!(suggestion, "rust async"),
            _ => panic!("expected a suggestion"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://search.kompas.services/api/v2/search/web/?")
        );
        assert!(request.url.contains("page_size=10"));
        assert!(request.url.contains("page=2"));
    }
}
