//! SearchZee engine, ported from searx/engines/searchzee.py.
//!
//! A random szs cookie identifies the session, like upstream's uuid4. The
//! category is selected by searchzee_categ (web or news). Upstream documents
//! the freshness parameter for the news index only.
// engine-id: searchzee

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "sz",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://searchzee.com";

/// SearchZee, an independent web and news search.
pub struct SearchZee {
    meta: EngineMeta,
}

impl SearchZee {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let category = meta.opt_str("searchzee_categ").unwrap_or("web");
        if !matches!(category, "web" | "news") {
            return Err(format!("searchzee: invalid category {category:?}"));
        }
        Ok(Arc::new(Self { meta }))
    }

    fn category(&self) -> &str {
        self.meta.opt_str("searchzee_categ").unwrap_or("web")
    }

    fn base_url(&self) -> String {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for SearchZee {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("type", self.category().to_string()),
            ("offset", pageno.saturating_sub(1).to_string()),
        ];
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "pd",
                TimeRange::Week => "pw",
                TimeRange::Month => "pm",
                TimeRange::Year => "py",
            };
            pairs.push(("freshness", value.to_string()));
        }
        // The API path is not fixed: it is the first twelve hex digits of
        // sha256("sz:" + the UTC date), so it changes every day. The old
        // /api/search path answers with the SPA shell instead of JSON.
        let url = format!(
            "{}/api/s/{}?{}",
            self.base_url(),
            api_token(),
            query_string(&pairs)
        );
        let request = EngineRequest::get(url)
            .cookie("szs", &session_id())
            .header("Accept", "application/json");
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
            let title = strip_tags(&json_str(item, "title"));
            if url.is_empty() || title.is_empty() || !url.starts_with("http") {
                continue;
            }
            let summary = strip_tags(&json_str(item, "summary"));
            let mut builder = ResultBuilder::new(url, title).content(summary);
            let thumbnail = json_str(item, "thumbnail");
            if thumbnail.starts_with("http") {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The daily API path token: sha256("sz:" + YYYY-MM-DD)[..12], in UTC.
fn api_token() -> String {
    let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let digest = Sha256::digest(format!("sz:{day}").as_bytes());
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    hex[..12].to_string()
}

/// A UUID shaped session id; upstream sends a random uuid4 as the szs cookie.
fn session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let digest = Sha256::digest(nanos.to_string().as_bytes());
    let mut id = String::with_capacity(36);
    for (index, byte) in digest.iter().take(16).enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        id.push_str(&format!("{byte:02x}"));
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"results": [
  {"url": "https://example.test/one", "title": "First <b>hit</b>",
   "summary": "A <i>summary</i>", "thumbnail": "https://example.test/one.jpg"},
  {"url": "javascript:void(0)", "title": "Bad", "summary": ""},
  {"url": "https://example.test/two", "title": "Second", "summary": "Plain"}
]}"##;

    fn build(category: &str) -> Result<Arc<SearchZee>, String> {
        let mut config = EngineConfig::new("searchzee", "searchzee", "sz", &["general"]);
        config
            .extra
            .insert("searchzee_categ".into(), category.into());
        SearchZee::from_config(&config)
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    fn parse(category: &str) -> Vec<EngineItem> {
        let engine = build(category).unwrap();
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        engine.parse(&response, &query()).unwrap()
    }

    fn result_at(items: &[EngineItem], index: usize) -> &crate::results::SearchResult {
        match &items[index] {
            EngineItem::Result(found) => found,
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn parses_the_results() {
        assert!(build("images").is_err());
        let items = parse("news");
        assert_eq!(items.len(), 2);
        let first = result_at(&items, 0);
        assert_eq!(first.title, "First hit");
        assert_eq!(first.content, "A summary");
        assert_eq!(
            first.thumbnail.as_deref(),
            Some("https://example.test/one.jpg")
        );
        assert!(result_at(&items, 1).thumbnail.is_none());
        assert_eq!(result_at(&items, 1).content, "Plain");
        let empty = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(
            build("web")
                .unwrap()
                .parse(&empty, &query())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn builds_the_api_request() {
        let engine = build("news").unwrap();
        let mut request_query = query();
        request_query.time_range = Some(TimeRange::Week);
        let request = engine.build_request(&request_query, 2).unwrap();
        assert!(
            request.url.starts_with("https://searchzee.com/api/s/"),
            "{}",
            request.url
        );
        assert_eq!(api_token().len(), 12);
        assert!(request.url.contains("type=news"));
        assert!(request.url.contains("offset=1"));
        assert!(request.url.contains("freshness=pw"));
        let cookies = request.cookies.clone();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].0, "szs");
        assert_eq!(cookies[0].1.len(), 36);
        assert_eq!(cookies[0].1.matches('-').count(), 4);
    }
}
