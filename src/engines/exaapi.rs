//! Exa Search API engine, ported from searx/engines/exaapi.py.
// engine-id: exaapi

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "exa",
    paging: false,
    time_range_support: true,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.exa.ai/search";
const SEARCH_TYPES: &[&str] = &[
    "fast",
    "auto",
    "instant",
    "lite",
    "deep",
    "deep-lite",
    "deep-reasoning",
];
const CONTENT_MODES: &[&str] = &["highlights", "text"];
const DEFAULT_RESULTS: i64 = 10;
const DEFAULT_CHARACTERS: i64 = 500;

/// Exa, a search API for AI agents. Requires an api_key.
pub struct ExaApi {
    meta: EngineMeta,
    api_key: String,
}

fn iso_date(value: &str) -> String {
    let date = chrono::DateTime::parse_from_rfc3339(value);
    date.map(|date| date.to_rfc3339()).unwrap_or_default()
}

impl ExaApi {
    /// The checks of the upstream setup() hook.
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let api_key = config
            .api_key
            .as_deref()
            .or_else(|| config.opt_str("api_key"))
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
            .or_else(|| {
                std::env::var("EXA_API_KEY")
                    .ok()
                    .map(|key| key.trim().to_string())
                    .filter(|key| !key.is_empty())
            })
            .ok_or_else(|| "exaapi requires an api_key or EXA_API_KEY".to_string())?;
        let results = config
            .opt_i64("results_per_page")
            .unwrap_or(DEFAULT_RESULTS);
        let search_type = config.opt_str("search_type").unwrap_or("auto");
        let content_mode = config.opt_str("content_mode").unwrap_or("highlights");
        let characters = config
            .opt_i64("content_max_characters")
            .unwrap_or(DEFAULT_CHARACTERS);
        if !(1..=100).contains(&results)
            || !SEARCH_TYPES.contains(&search_type)
            || !CONTENT_MODES.contains(&content_mode)
            || characters < 1
        {
            return Err(format!(
                "invalid exaapi options: {search_type} {content_mode}"
            ));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            api_key,
        }))
    }
}

impl Engine for ExaApi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let key = &self.api_key;
        let max_characters = self
            .meta
            .opt_i64("content_max_characters")
            .unwrap_or(DEFAULT_CHARACTERS)
            .max(1);
        let contents = match self.meta.opt_str("content_mode") {
            Some("text") => {
                serde_json::json!({"text": {"maxCharacters": max_characters, "stripLinks": true}})
            }
            _ => serde_json::json!({"highlights": {"maxCharacters": max_characters}}),
        };
        let mut body = serde_json::json!({
            "query": query.query_text(),
            "type": self.meta.opt_str("search_type").unwrap_or("auto"),
            "numResults": self.meta.opt_i64("results_per_page").unwrap_or(DEFAULT_RESULTS),
            "contents": contents,
        });
        if let Some(range) = query.time_range {
            let days = match range {
                TimeRange::Day => 1,
                TimeRange::Week => 7,
                TimeRange::Month => 30,
                TimeRange::Year => 365,
            };
            body["startPublishedDate"] = serde_json::Value::String(
                (chrono::Utc::now() - chrono::Duration::days(days))
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            );
        }
        if query.safesearch > 0 {
            body["moderation"] = serde_json::Value::Bool(true);
        }
        let region = locale_region(&query.locale);
        if region != "all" {
            body["userLocation"] = serde_json::Value::String(region.to_uppercase());
        }
        Ok(EngineRequest::post(BASE_URL)
            .header("x-api-key", key)
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
        let text_mode = self.meta.opt_str("content_mode") == Some("text");
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            if !url.starts_with("https://") && !url.starts_with("http://") {
                continue;
            }
            let highlights: Vec<&str> = json_path(item, "highlights")
                .and_then(|value| value.as_array())
                .map(|values| values.iter().filter_map(|value| value.as_str()).collect())
                .unwrap_or_default();
            let excerpt = if text_mode {
                json_str(item, "text")
            } else {
                highlights.join(" ")
            };
            let title = strip_tags(&json_str(item, "title"));
            let title = if title.is_empty() { url.clone() } else { title };
            let mut builder = ResultBuilder::new(url, title)
                .content(strip_tags(&excerpt))
                .author(json_str(item, "author"))
                .published_date(iso_date(&json_str(item, "publishedDate")));
            let image = json_str(item, "image");
            if image.starts_with("https://") || image.starts_with("http://") {
                builder = builder.thumbnail(image);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<ExaApi> {
        let mut config = EngineConfig::new("exaapi", "exaapi", "exa", &["general", "web"]);
        config.api_key = Some("KEY".to_string());
        ExaApi::from_config(&config).expect("exaapi engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn lite_search_uses_native_publication_date_filters_and_explicit_key() {
        let mut config = EngineConfig::new("exaapi", "exaapi", "exa", &["general"]);
        config.api_key = Some("configured-key".into());
        config
            .extra
            .insert("search_type".into(), toml::Value::String("lite".into()));
        let engine = ExaApi::from_config(&config).unwrap();
        assert!(engine.meta().time_range_support);
        let mut query = query("published this week");
        query.time_range = Some(TimeRange::Week);
        let before = chrono::Utc::now() - chrono::Duration::days(7);
        let request = engine.build_request(&query, 1).unwrap();
        assert_eq!(
            request
                .headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("configured-key")
        );
        let body = request.json.unwrap();
        assert_eq!(body["type"], "lite");
        let since =
            chrono::DateTime::parse_from_rfc3339(body["startPublishedDate"].as_str().unwrap())
                .unwrap();
        assert!((since.timestamp() - before.timestamp()).abs() <= 1);
        assert_eq!(body["query"], "published this week");
    }

    #[test]
    fn builds_the_api_body() {
        let mut query = query("rust async");
        query.locale = "en-US".to_string();
        query.safesearch = 1;
        let request = engine().build_request(&query, 1).expect("request");
        let body = request.json.as_ref().expect("json body");
        assert_eq!(body["query"].as_str(), Some("rust async"));
        assert_eq!(body["userLocation"].as_str(), Some("US"));
        assert_eq!(
            body["contents"]["highlights"]["maxCharacters"].as_i64(),
            Some(500)
        );
    }

    #[test]
    fn parses_the_results() {
        let body = r##"{"results": [
          {"url": "https://exa.ai/", "title": "Exa <b>Search</b>", "highlights": ["A <i>search</i> engine.", "For agents."], "image": "https://exa.ai/og.png", "publishedDate": "2024-05-01T10:00:00Z"},
          {"title": "no url"}, {"url": "ftp://example.com/file", "title": "not http"}]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let items = engine().parse(&response, &query("exa")).expect("items");
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://exa.ai/");
                assert_eq!(result.title, "Exa Search");
                assert_eq!(result.content, "A search engine. For agents.");
                assert_eq!(result.thumbnail.as_deref(), Some("https://exa.ai/og.png"));
            }
            _ => panic!("expected a result"),
        }
        let empty = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        let items = engine().parse(&empty, &query("exa"));
        assert!(items.expect("items").is_empty());
    }
}
