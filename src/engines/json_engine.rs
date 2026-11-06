//! The declarative `json_engine`, ported from `searx/engines/json_engine.py`.
//!
//! Upstream (`settings.yml`: `engine: json_engine`) describes a JSON API engine
//! with a search URL template plus query paths for the result fields. The path
//! syntax is the upstream one: slash separated keys, `$` descends into a list,
//! and the first match is used.

use std::sync::Arc;

use serde_json::Value;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// A declaratively configured JSON engine.
pub struct JsonEngine {
    meta: EngineMeta,
    search_url: String,
    method: String,
    request_body: Option<String>,
    results_query: String,
    url_query: String,
    title_query: String,
    content_query: String,
    thumbnail_query: String,
    url_prefix: String,
    thumbnail_prefix: String,
    content_html_to_text: bool,
    title_html_to_text: bool,
    page_size: u32,
    first_page_num: u32,
    send_page_num_on_first_page: bool,
    time_range_url: String,
    time_range_map: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    cookies: Vec<(String, String)>,
    language_all: String,
}

impl JsonEngine {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let search_url = config
            .search_url
            .clone()
            .or_else(|| config.opt_str("search_url").map(str::to_string))
            .ok_or_else(|| format!("json_engine {:?} needs a search_url", config.name))?;
        let required = |key: &str| -> Result<String, String> {
            config
                .opt_str(key)
                .map(str::to_string)
                .ok_or_else(|| format!("json_engine {:?} needs {key}", config.name))
        };
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            search_url,
            method: config.opt_str("method").unwrap_or("GET").to_string(),
            request_body: config.opt_str("request_body").map(str::to_string),
            results_query: config
                .opt_str("results_query")
                .unwrap_or_default()
                .to_string(),
            url_query: required("url_query")?,
            title_query: required("title_query")?,
            content_query: config
                .opt_str("content_query")
                .unwrap_or_default()
                .to_string(),
            thumbnail_query: config
                .opt_str("thumbnail_query")
                .unwrap_or_default()
                .to_string(),
            url_prefix: config.opt_str("url_prefix").unwrap_or_default().to_string(),
            thumbnail_prefix: config
                .opt_str("thumbnail_prefix")
                .unwrap_or_default()
                .to_string(),
            content_html_to_text: config.opt_bool("content_html_to_text").unwrap_or(false),
            title_html_to_text: config.opt_bool("title_html_to_text").unwrap_or(false),
            page_size: config
                .opt_i64("page_size")
                .map(|value| value.max(1) as u32)
                .unwrap_or(1),
            first_page_num: config
                .opt_i64("first_page_num")
                .map(|value| value.max(0) as u32)
                .unwrap_or(1),
            send_page_num_on_first_page: config
                .opt_bool("send_page_num_on_first_page")
                .unwrap_or(false),
            time_range_url: config
                .opt_str("time_range_url")
                .unwrap_or_default()
                .to_string(),
            time_range_map: string_pairs(config.extra.get("time_range_map")),
            headers: string_pairs(config.extra.get("headers")),
            cookies: string_pairs(config.extra.get("cookies")),
            language_all: config.opt_str("language_all").unwrap_or("en").to_string(),
        }))
    }

    fn template(&self, query: &Query, pageno: u32) -> String {
        let language = locale_language(&query.locale);
        let language = if language == "all" {
            self.language_all.clone()
        } else {
            language.chars().take(2).collect::<String>()
        };
        let page_value = if pageno <= 1 && !self.send_page_num_on_first_page {
            String::new()
        } else {
            ((pageno.saturating_sub(1)) * self.page_size + self.first_page_num).to_string()
        };
        let time_range = query
            .time_range
            .map(|range| range.as_str())
            .and_then(|range| {
                self.time_range_map
                    .iter()
                    .find(|(key, _)| key == range)
                    .map(|(_, value)| value.clone())
            })
            .unwrap_or_default();
        let time_range_output = if time_range.is_empty() {
            String::new()
        } else {
            self.time_range_url.replace("{time_range_val}", &time_range)
        };
        self.search_url
            .replace("{query}", &encode(&query.query_text()))
            .replace("{lang}", &language)
            .replace("{language}", &language)
            .replace("{pageno}", &page_value)
            .replace("{time_range}", &time_range_output)
    }
}

impl Engine for JsonEngine {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let url = self.template(query, pageno);
        let post = self.request_body.is_some() || self.method.eq_ignore_ascii_case("POST");
        let mut request = if post {
            EngineRequest::post(url)
        } else {
            EngineRequest::get(url)
        };
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        for (name, value) in &self.cookies {
            request = request.cookie(name, value);
        }
        if let Some(body) = &self.request_body {
            let body = body.replace("{query}", &encode(&query.query_text()));
            request = request
                .header("Content-Type", "application/x-www-form-urlencoded")
                .form(
                    body.split('&')
                        .filter_map(|pair| pair.split_once('='))
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect(),
                );
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        // The results query usually points at an array of records; a single
        // array value is unwrapped so each record becomes one result.
        let containers: Vec<&Value> = json_query(&json, &self.results_query)
            .into_iter()
            .flat_map(|value| match value {
                Value::Array(items) => items.iter().collect::<Vec<_>>(),
                other => vec![other],
            })
            .collect();
        let mut results = Vec::new();
        for container in containers {
            let Some(url) = first_value(container, &self.url_query).map(to_string) else {
                continue;
            };
            let Some(title) = first_value(container, &self.title_query).map(to_string) else {
                continue;
            };
            if url.is_empty() || title.is_empty() {
                continue;
            }
            let title = if self.title_html_to_text {
                strip_tags(&title)
            } else {
                title
            };
            let content = first_value(container, &self.content_query)
                .map(to_string)
                .map(|content| {
                    if self.content_html_to_text {
                        strip_tags(&content)
                    } else {
                        content
                    }
                })
                .unwrap_or_default();
            let url = format!("{}{}", self.url_prefix, url);
            let mut builder = ResultBuilder::new(url, title).content(content);
            if !self.thumbnail_query.is_empty() {
                if let Some(thumbnail) =
                    first_value(container, &self.thumbnail_query).map(to_string)
                {
                    if !thumbnail.is_empty() {
                        builder =
                            builder.thumbnail(format!("{}{}", self.thumbnail_prefix, thumbnail));
                    }
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Resolve an upstream style JSON query: `results`, `data/list`, `x/y/$`.
pub fn json_query<'a>(value: &'a Value, query: &str) -> Vec<&'a Value> {
    let path: Vec<&str> = query.split('/').filter(|part| !part.is_empty()).collect();
    resolve(value, &path)
}

fn resolve<'a>(value: &'a Value, path: &[&str]) -> Vec<&'a Value> {
    if path.is_empty() {
        return vec![value];
    }
    match value {
        Value::Object(map) => {
            if path[0] == "$" {
                // An object keyed by id (Habr's publicationRefs) is a column of
                // records just like an array, so the wildcard visits every value.
                map.values()
                    .flat_map(|child| resolve(child, &path[1..]))
                    .collect()
            } else {
                match map.get(path[0]) {
                    Some(child) => resolve(child, &path[1..]),
                    None => Vec::new(),
                }
            }
        }
        Value::Array(items) => {
            if path[0] == "$" {
                items
                    .iter()
                    .flat_map(|item| resolve(item, &path[1..]))
                    .collect()
            } else {
                items.iter().flat_map(|item| resolve(item, path)).collect()
            }
        }
        _ => Vec::new(),
    }
}

fn first_value<'a>(value: &'a Value, query: &str) -> Option<&'a Value> {
    json_query(value, query).into_iter().next()
}

fn to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::new(),
        Value::Array(items) => items.iter().map(to_string).collect::<Vec<_>>().join(" "),
        Value::Object(map) => map
            .get("$")
            .map(to_string)
            .unwrap_or_else(|| map.values().map(to_string).collect::<Vec<_>>().join(" ")),
    }
}

fn string_pairs(value: Option<&toml::Value>) -> Vec<(String, String)> {
    match value {
        Some(toml::Value::Table(table)) => table
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r#"{"documents": [
      {"mdn_url": "/en-US/docs/Web/API/Fetch_API", "title": "Fetch API", "summary": "The Fetch API."},
      {"mdn_url": "/en-US/docs/Web/JavaScript", "title": "JavaScript", "summary": "A language."}
    ]}"#;

    fn engine() -> Arc<JsonEngine> {
        let mut config = EngineConfig::new("mdn", "json_engine", "mdn", &["it"]);
        config.search_url =
            Some("https://developer.mozilla.org/api/v1/search?q={query}&page={pageno}".into());
        config.extra.insert(
            "results_query".into(),
            toml::Value::String("documents".into()),
        );
        config
            .extra
            .insert("url_query".into(), toml::Value::String("mdn_url".into()));
        config
            .extra
            .insert("title_query".into(), toml::Value::String("title".into()));
        config.extra.insert(
            "content_query".into(),
            toml::Value::String("summary".into()),
        );
        config.extra.insert(
            "url_prefix".into(),
            toml::Value::String("https://developer.mozilla.org".into()),
        );
        config
            .extra
            .insert("page_size".into(), toml::Value::Integer(10));
        JsonEngine::from_config(&config).unwrap()
    }

    #[test]
    fn parses_the_configured_paths() {
        let response = EngineResponse::new(200, "https://example.com", FIXTURE.as_bytes().to_vec());
        let query = Query::parse("fetch", &EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.url,
                    "https://developer.mozilla.org/en-US/docs/Web/API/Fetch_API"
                );
                assert_eq!(result.title, "Fetch API");
                assert_eq!(result.content, "The Fetch API.");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn page_numbers_follow_the_upstream_formula() {
        let query = Query::parse("fetch", &EngineRegistry::default(), &[]);
        let first = engine().build_request(&query, 1).unwrap().url;
        // Upstream sends an empty page number on the first page.
        assert!(first.ends_with("page="), "{first}");
        // (pageno - 1) * page_size + first_page_num, upstream's formula.
        let second = engine().build_request(&query, 2).unwrap().url;
        assert!(second.contains("page=11"), "{second}");
    }

    /// Habr keys its records by id, so the wildcard has to walk object values
    /// as well as array elements.
    #[test]
    fn dollar_descends_into_id_keyed_objects() {
        let value: Value = serde_json::json!({
            "publicationRefs": {
                "9001": {"titleHtml": "first", "id": "9001"},
                "9002": {"titleHtml": "second", "id": "9002"}
            }
        });
        let found = json_query(&value, "publicationRefs/$/titleHtml");
        assert_eq!(found.len(), 2);
        let titles: Vec<String> = found.iter().map(|value| to_string(value)).collect();
        assert!(titles.contains(&"first".to_string()), "{titles:?}");
        assert!(titles.contains(&"second".to_string()), "{titles:?}");
        // A named key inside an object still resolves to exactly one value.
        let one = json_query(&value, "publicationRefs/9002/id");
        assert_eq!(one.len(), 1);
        assert_eq!(to_string(one[0]), "9002");
    }

    #[test]
    fn dollar_descends_into_lists() {
        let value: Value = serde_json::json!({"a": {"b": [{"url": "x"}, {"url": "y"}]}});
        let found = json_query(&value, "a/b/$/url");
        assert_eq!(found.len(), 2);
        assert_eq!(to_string(found[0]), "x");
    }
}
