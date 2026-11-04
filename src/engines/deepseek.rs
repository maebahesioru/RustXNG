//! DeepSeek Search API engine, using DeepSeek's in-house web search.
//!
//! The endpoint returns ranked results with a short snippet and a longer
//! model-oriented summary. It has no page offset, so the engine does not
//! advertise paging; period and language filters are passed as query
//! operators.
// engine-id: deepseek

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_COUNT: u32 = 10;
/// The Search API accepts 1 to 128 Unicode characters per query.
const MAX_QUERY_CHARS: usize = 128;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "ds",
    paging: false,
    time_range_support: true,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 10.0,
};

/// DeepSeek Search. Requires an API key; billed per successful request.
pub struct DeepSeek {
    meta: EngineMeta,
    api_key: String,
    count: u32,
    need_content: bool,
}

impl DeepSeek {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let api_key = config
            .api_key
            .as_deref()
            .or_else(|| config.opt_str("api_key"))
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
            .or_else(|| {
                std::env::var("DEEPSEEK_API_KEY")
                    .ok()
                    .map(|key| key.trim().to_string())
                    .filter(|key| !key.is_empty())
            })
            .ok_or_else(|| "deepseek requires an api_key or DEEPSEEK_API_KEY".to_string())?;
        let count = config.opt_i64("count").unwrap_or(DEFAULT_COUNT as i64);
        if !(1..=50).contains(&count) {
            return Err("deepseek count must be between 1 and 50".to_string());
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            api_key,
            count: count as u32,
            need_content: config.opt_bool("need_content").unwrap_or(false),
        }))
    }

    fn endpoint(&self) -> String {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL)
            .trim()
            .trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/tools/search")
        } else {
            format!("{base}/v1/tools/search")
        }
    }

    /// Add period and language operators without exceeding the API's query
    /// limit. The operators are kept even when the base query is long.
    fn search_query(query: &Query) -> String {
        let base_query = query.query_text();
        let base_query = base_query.trim();
        if base_query.is_empty() {
            return String::new();
        }
        let mut suffix = String::new();
        if let Some(range) = query.time_range {
            let days = match range {
                TimeRange::Day => 1,
                TimeRange::Week => 7,
                TimeRange::Month => 30,
                TimeRange::Year => 365,
            };
            let date = (chrono::Utc::now() - chrono::Duration::days(days)).format("%Y-%m-%d");
            suffix.push_str(&format!(" after:{date}"));
        }
        let language = locale_language(&query.locale);
        if language != "all" {
            suffix.push_str(&format!(" lang:{language}"));
        }
        let budget = MAX_QUERY_CHARS.saturating_sub(suffix.chars().count());
        let base: String = base_query.chars().take(budget).collect();
        format!("{}{suffix}", base.trim()).trim().to_string()
    }
}

impl Engine for DeepSeek {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let text = Self::search_query(query);
        if text.is_empty() {
            return Err(EngineError::Message(
                "deepseek needs a non-empty query".into(),
            ));
        }
        let body = serde_json::json!({
            "query": text,
            "count": self.count,
            "need_content": self.need_content,
        });
        Ok(EngineRequest::post(self.endpoint())
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json_body(body))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        if let Some(error) = json.get("error") {
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown error");
            return Err(EngineError::Message(format!(
                "DeepSeek Search error: {message}"
            )));
        }
        let Some(items) = json.get("results").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            if !url.starts_with("http://") && !url.starts_with("https://") {
                continue;
            }
            let title = strip_tags(&json_str(item, "title"));
            let title = if title.is_empty() { url.clone() } else { title };
            // The summary is written for models; the content is the full page
            // and only present when need_content was requested.
            let content = {
                let summary = json_str(item, "summary");
                let snippet = json_str(item, "snippet");
                if !summary.trim().is_empty() {
                    summary
                } else if !snippet.trim().is_empty() {
                    snippet
                } else {
                    json_str(item, "content")
                }
            };
            let mut builder = ResultBuilder::new(url, title).content(strip_tags(&content));
            let date = json_str(item, "date");
            if !date.trim().is_empty() {
                builder = builder.published_date(normalize_date(&date));
            }
            let site = json_str(item, "site_name");
            if !site.trim().is_empty() {
                builder = builder.metadata("site_name", site);
            }
            if let Some(authority) = item.get("authority").and_then(serde_json::Value::as_i64) {
                builder = builder.metadata("authority", authority.to_string());
            }
            if let Some(score) = item.get("rerank_score").and_then(serde_json::Value::as_f64) {
                builder = builder.metadata("rerank_score", format!("{score:.2}"));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn normalize_date(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|date| date.to_rfc3339())
        .unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> EngineConfig {
        let mut config = EngineConfig::new("deepseek", "deepseek", "ds", &["general", "web"]);
        config.api_key = Some("KEY".to_string());
        config
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_search_request() {
        let engine = DeepSeek::from_config(&config()).expect("deepseek engine");
        let mut query = query("rust async");
        query.locale = "ja-JP".to_string();
        query.time_range = Some(TimeRange::Week);
        let request = engine.build_request(&query, 1).expect("request");
        assert_eq!(request.headers.get("authorization").unwrap(), "Bearer KEY");
        let body = request.json.expect("json body");
        let text = body["query"].as_str().unwrap();
        assert!(text.starts_with("rust async"));
        assert!(text.contains("lang:ja"));
        assert!(text.contains("after:"));
        assert_eq!(body["count"].as_u64(), Some(10));
        assert_eq!(body["need_content"].as_bool(), Some(false));
    }

    #[test]
    fn parses_results_and_prefers_the_summary() {
        let body = r#"{"results": [
          {"title": "Rust <b>lang</b>", "url": "https://www.rust-lang.org/",
           "snippet": "short", "summary": "A language for reliable software.",
           "content": "Full page.", "site_name": "Rust", "date": "2026-01-02T03:04:05Z",
           "authority": 4, "rerank_score": 2.4},
          {"title": "no url"}, {"title": "not http", "url": "ftp://example.com/x"}
        ]}"#;
        let response =
            EngineResponse::new(200, "https://api.deepseek.com", body.as_bytes().to_vec());
        let engine = DeepSeek::from_config(&config()).unwrap();
        let items = engine.parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust lang");
                assert_eq!(result.content, "A language for reliable software.");
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2026-01-02T03:04:05+00:00")
                );
                assert_eq!(result.metadata.get("site_name").unwrap(), "Rust");
                assert_eq!(result.metadata.get("authority").unwrap(), "4");
                assert_eq!(result.metadata.get("rerank_score").unwrap(), "2.40");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn requires_an_api_key() {
        let mut config = EngineConfig::new("deepseek", "deepseek", "ds", &["general"]);
        config.api_key = Some("  ".to_string());
        // The environment variable is absent in normal test runs; if a developer
        // exports it, the engine should build instead.
        if std::env::var("DEEPSEEK_API_KEY").is_err() {
            assert!(DeepSeek::from_config(&config).is_err());
        }
    }

    #[test]
    fn rejects_an_out_of_range_count() {
        let mut config = config();
        config
            .extra
            .insert("count".into(), toml::Value::Integer(51));
        assert!(DeepSeek::from_config(&config).is_err());
    }

    #[test]
    fn rejects_an_empty_query() {
        let engine = DeepSeek::from_config(&config()).unwrap();
        assert!(engine.build_request(&query(""), 1).is_err());
    }

    #[test]
    fn keeps_the_query_inside_the_api_limit() {
        let long = "x".repeat(200);
        let mut query = query(&long);
        query.locale = "ja".to_string();
        query.time_range = Some(TimeRange::Year);
        let text = DeepSeek::search_query(&query);
        assert!(text.chars().count() <= MAX_QUERY_CHARS);
        assert!(text.contains("lang:ja"));
        assert!(text.contains("after:"));
    }
}
