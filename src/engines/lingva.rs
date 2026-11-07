//! Lingva engine, ported from searx/engines/lingva.py.
//!
//! Lingva is an alternative front-end for Google Translate. Upstream is an
//! online dictionary engine: the query starts with a from-to language pair
//! such as en-de. Its translation result becomes a regular result here.

// engine-id: lingva

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "translate"],
    shortcut: "lv",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 6.0,
};

const BASE_URL: &str = "https://lingva.ml";

/// Lingva Translate.
pub struct Lingva {
    meta: EngineMeta,
}

impl Lingva {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn base_url(meta: &EngineMeta) -> String {
    meta.base_url
        .as_deref()
        .unwrap_or(BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn is_language_code(value: &str) -> bool {
    (2..=3).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphabetic())
}

/// Split the from-to prefix off the query, like upstream's dictionary
/// processor. Without a prefix the source language is detected and the text
/// is translated into the requested engine language.
fn direction(query: &Query) -> (String, String, String) {
    let text = query.query_text();
    let mut parts = text.splitn(2, ' ');
    let first = parts.next().unwrap_or_default();
    let rest = parts.next().unwrap_or_default().trim();
    if !rest.is_empty() {
        let mut codes = first.split('-');
        if let (Some(from), Some(to), None) = (codes.next(), codes.next(), codes.next()) {
            if is_language_code(from) && is_language_code(to) {
                return (from.to_lowercase(), to.to_lowercase(), rest.to_string());
            }
        }
    }
    let language = locale_language(&query.engine_language());
    let to = if language == "all" {
        "en".to_string()
    } else {
        language
    };
    ("auto".to_string(), to, text)
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

impl Engine for Lingva {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let (from, to, text) = direction(query);
        Ok(EngineRequest::get(format!(
            "{}/api/v1/{}/{}/{}",
            base_url(&self.meta),
            encode(&from),
            encode(&to),
            encode(&text)
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let (from, to, text) = direction(query);
        let json = response.json()?;
        let translation = json_str(&json, "translation");
        if translation.is_empty() || text.is_empty() {
            return Ok(Vec::new());
        }
        let base = base_url(&self.meta);
        let url = format!(
            "{}/{}/{}/{}",
            base,
            encode(&from),
            encode(&to),
            encode(&text)
        );
        if !is_http(&url) {
            return Ok(Vec::new());
        }
        let mut content = vec![translation];
        if let Some(definitions) = json_path(&json, "info.definitions").and_then(|v| v.as_array()) {
            for definition in definitions {
                if let Some(items) = json_path(definition, "list").and_then(|v| v.as_array()) {
                    for item in items {
                        let value = json_str(item, "definition");
                        if !value.is_empty() && !content.contains(&value) {
                            content.push(value);
                        }
                        let example = json_str(item, "example");
                        if !example.is_empty() {
                            content.push(format!("Example: {example}"));
                        }
                    }
                }
            }
        }
        if let Some(extra) = json_path(&json, "info.extraTranslations").and_then(|v| v.as_array()) {
            for group in extra {
                if let Some(words) = json_path(group, "list").and_then(|v| v.as_array()) {
                    for word in words {
                        let value = json_str(word, "word");
                        if !value.is_empty() {
                            content.push(value);
                        }
                    }
                }
            }
        }
        Ok(vec![
            ResultBuilder::new(url, text)
                .content(content.join(" "))
                .metadata("from", from)
                .metadata("to", to)
                .build(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{
      "translation": "Hallo",
      "info": {
        "definitions": [{"list": [{"definition": "a greeting", "example": "Hello there"}]}],
        "extraTranslations": [{"list": [{"word": "Guten Tag"}]}]
      }
    }"##;

    fn engine() -> Arc<Lingva> {
        Lingva::from_config(&EngineConfig::new("lingva", "lingva", "lv", &["general"])).unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_a_translation() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("en-de hello")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "hello");
                assert_eq!(result.url, "https://lingva.ml/en/de/hello");
                assert_eq!(
                    result.content,
                    "Hallo a greeting Example: Hello there Guten Tag"
                );
                assert_eq!(result.metadata.get("from").map(String::as_str), Some("en"));
                assert_eq!(result.metadata.get("to").map(String::as_str), Some("de"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_api_url() {
        let request = engine()
            .build_request(&query("en-de hello world"), 1)
            .unwrap();
        assert_eq!(request.url, "https://lingva.ml/api/v1/en/de/hello%20world");
    }

    #[test]
    fn missing_translation_is_empty() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        let items = engine().parse(&response, &query("en-de hello")).unwrap();
        assert!(items.is_empty());
    }
}
