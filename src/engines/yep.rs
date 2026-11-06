//! Yep engine, ported from searx/engines/yep.py and kept in step with the web app.
//!
//! The historical JSON endpoint (api.yep.com/search) answers 404 now. The site
//! itself is a SvelteKit app that fetches /web/v1/search with
//! accept: application/json; because that endpoint sits behind Cloudflare the
//! engine leans on the browser solver, and the solved response arrives as a
//! rendered JSON document wrapped in a <pre> block, which parse unwraps.
// engine-id: yep

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "yep",
    paging: false,
    time_range_support: false,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    // Cloudflare plus a browser solve; a 4 second budget only ever yields
    // "solver exceeded the engine budget".
    timeout: 25.0,
};

const BASE_URL: &str = "https://yep.com";
const SEARCH_PATH: &str = "/web/v1/search";
const RESULTS_PER_PAGE: i64 = 20;

/// Yep.
pub struct Yep {
    meta: EngineMeta,
}

impl Yep {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for Yep {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
            .to_string();
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("limit", RESULTS_PER_PAGE.to_string()),
        ];
        // The web app sends language=<ui filter> plus lang=<two letter code>.
        // The locale is only a hint, so it is forwarded as lang and never as a
        // hard language filter.
        let language = self
            .meta
            .opt_str("language")
            .map(str::to_string)
            .unwrap_or_else(|| locale_language(&query.locale));
        if language.len() == 2 && language.bytes().all(|byte| byte.is_ascii_lowercase()) {
            pairs.push(("lang", language));
        }
        Ok(
            EngineRequest::get(format!("{base}{SEARCH_PATH}?{}", query_string(&pairs)))
                .header("accept", "application/json")
                .header("referer", "https://yep.com/web")
                .header("origin", BASE_URL),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        if body.contains("Just a moment") || body.contains("cf-challenge") {
            return Err(EngineError::Captcha);
        }
        let payload = json_payload(&body)
            .ok_or_else(|| EngineError::Parse("yep returned no JSON payload".to_string()))?;
        let json: serde_json::Value = serde_json::from_str(payload)
            .map_err(|error| EngineError::Parse(format!("invalid JSON response: {error}")))?;
        let Some(items) = json.get("results").and_then(|value| value.as_array()) else {
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
                    .content(strip_tags(&json_str(item, "description")))
                    .build(),
            );
        }
        Ok(results)
    }
}

/// The JSON document of a response, which the solver hands back inside a
/// rendered HTML page.
fn json_payload(body: &str) -> Option<&str> {
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    if end <= start {
        return None;
    }
    body.get(start..=end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    fn engine() -> Arc<Yep> {
        Yep::from_config(&EngineConfig::new("yep", "yep", "yep", &["general"])).expect("engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_json_results() {
        let body = r##"{"results":[
          {"url":"https://www.rust-lang.org/","title":"Rust","description":"A <b>language</b>","site_name":null},
          {"url":"https://example.com/","title":"","description":"No title."}
        ],"returned_count":2}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust");
                assert_eq!(result.content, "A language");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn unwraps_the_json_the_solver_renders() {
        let body = concat!(
            "<html><head><meta charset=\"utf-8\"></head><body><pre>",
            "{\"results\":[{\"url\":\"https://rust-lang.org/\",\"title\":\"Rust\"}]}",
            "</pre></body></html>"
        );
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        assert_eq!(engine().parse(&response, &query("rust")).unwrap().len(), 1);
    }

    #[test]
    fn request_targets_the_web_api() {
        let mut request_query = query("rust");
        request_query.locale = "de-DE".to_string();
        let request = engine().build_request(&request_query, 1).unwrap();
        assert!(request.url.starts_with("https://yep.com/web/v1/search?"));
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("limit=20"));
        assert!(request.url.contains("lang=de"));
        assert_eq!(
            request
                .headers
                .get("accept")
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
    }

    #[test]
    fn a_cloudflare_challenge_is_a_captcha() {
        let body = "<html><head><title>Just a moment...</title></head></html>";
        let response = EngineResponse::new(403, BASE_URL, body.as_bytes().to_vec());
        assert!(matches!(
            engine().parse(&response, &query("rust")),
            Err(EngineError::Captcha)
        ));
    }

    #[test]
    fn broken_bodies_are_rejected() {
        let response = EngineResponse::new(200, BASE_URL, b"not json".to_vec());
        assert!(matches!(
            engine().parse(&response, &query("rust")),
            Err(EngineError::Parse(_))
        ));
    }
}
