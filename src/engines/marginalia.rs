//! Marginalia engine, ported from searx/engines/marginalia.py.
//!
//! The API key is required; the optional filter_name is forwarded as the
//! filter parameter. Upstream validates the filter name against the API
//! before startup, which is not done here.
// engine-id: marginalia

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "blogs"],
    shortcut: "mar",
    paging: true,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api2.marginalia-search.com";
/// The keyless public endpoint lives on the old API host; the api2 host
/// answers 404 for /public/search.
const PUBLIC_BASE_URL: &str = "https://api.marginalia.nu";
const RESULTS_PER_PAGE: i64 = 20;
const USER_AGENT: &str =
    "Mozilla/5.0 (compatible; rustxng/0.2; +https://github.com/sahenjp/rustxng)";

/// Marginalia, an independent search engine for the small web.
pub struct Marginalia {
    meta: EngineMeta,
}

impl Marginalia {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for Marginalia {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let key = self.meta.api_key.as_deref().unwrap_or_default().trim();
        let mut request = if key.is_empty() {
            // The public endpoint needs no key. It takes the query as a path
            // segment and only understands `page`; `count`, `nsfw` and
            // `filter` make upstream stall until the socket times out.
            EngineRequest::get(format!(
                "{PUBLIC_BASE_URL}/public/search/{}?page={}",
                encode(query.query_text().trim()),
                pageno.max(1)
            ))
        } else {
            let mut pairs: Vec<(&str, String)> = vec![
                ("page", pageno.max(1).to_string()),
                ("count", RESULTS_PER_PAGE.to_string()),
                ("nsfw", query.safesearch.min(1).to_string()),
                ("query", query.query_text()),
            ];
            if let Some(filter) = self.meta.opt_str("filter_name") {
                if !filter.is_empty() {
                    pairs.push(("filter", filter.to_string()));
                }
            }
            let mut request =
                EngineRequest::get(format!("{base}/search?{}", query_string(&pairs)));
            request.set_header("API-Key", key);
            request
        };
        request.set_header("User-Agent", USER_AGENT);
        Ok(request)
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
            let title = json_str(item, "title");
            if title.is_empty() || !url.starts_with("http") {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(json_str(item, "description"))
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

    fn engine() -> Arc<Marginalia> {
        let mut config = EngineConfig::new("marginalia", "marginalia", "mar", &["general"]);
        config.api_key = Some("KEY".to_string());
        Marginalia::from_config(&config).expect("engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_json_results() {
        let body = r##"{"license":"CC-BY-NC-SA","query":"rust","results":[
          {"url":"https://www.rust-lang.org/","title":"Rust","description":"A language.","quality":1.5},
          {"url":"https://example.com/","title":"Other","description":"More text.","quality":0.5},
          {"url":"https://example.com/","title":"","description":"No title.","quality":0.1}
        ]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust");
                assert_eq!(result.content, "A language.");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => assert_eq!(result.content, "More text."),
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn the_public_endpoint_needs_no_key() {
        let config = EngineConfig::new("marginalia", "marginalia", "mar", &["general"]);
        let bare = Marginalia::from_config(&config).expect("engine");
        let request = bare.build_request(&query("rust programming"), 2).unwrap();
        assert_eq!(
            request.url,
            "https://api.marginalia.nu/public/search/rust%20programming?page=2"
        );
        assert!(request.headers.get("api-key").is_none());
    }

    #[test]
    fn request_carries_api_key_and_paging() {
        let mut request_query = query("rust");
        request_query.safesearch = 2;
        let request = engine().build_request(&request_query, 3).unwrap();
        assert!(
            request
                .url
                .starts_with("https://api2.marginalia-search.com/search?")
        );
        assert!(request.url.contains("page=3"));
        assert!(request.url.contains("count=20"));
        assert!(request.url.contains("nsfw=1"));
        assert!(request.url.contains("query=rust"));
        assert_eq!(
            request
                .headers
                .get("api-key")
                .and_then(|value| value.to_str().ok()),
            Some("KEY")
        );
    }

    #[test]
    fn api_key_is_optional_and_empty_bodies_are_ok() {
        let config = EngineConfig::new("marginalia", "marginalia", "mar", &["general"]);
        let bare = Marginalia::from_config(&config).expect("engine");
        assert!(bare.build_request(&query("rust"), 1).is_ok());
        let response = EngineResponse::new(200, BASE_URL, br##"{"license":"x"}"##.to_vec());
        assert!(
            engine()
                .parse(&response, &query("rust"))
                .unwrap()
                .is_empty()
        );
        let broken = EngineResponse::new(200, BASE_URL, b"not json".to_vec());
        assert!(matches!(
            engine().parse(&broken, &query("rust")),
            Err(EngineError::Parse(_))
        ));
    }
}
