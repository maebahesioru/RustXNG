//! Mozhi engine, ported from searx/engines/mozhi.py.
//!
//! Mozhi is an alternative front-end for several translation services. The
//! upstream instance list cannot be randomised here, so the configured
//! instance (the official one by default) is used.

// engine-id: mozhi

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "translate"],
    shortcut: "mz",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://mozhi.aryak.me";
const MOZHI_ENGINE: &str = "google";

/// Mozhi Translate.
pub struct Mozhi {
    meta: EngineMeta,
}

impl Mozhi {
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

fn engine_name(meta: &EngineMeta) -> String {
    meta.opt_str("mozhi_engine")
        .unwrap_or(MOZHI_ENGINE)
        .to_string()
}

fn is_language_code(value: &str) -> bool {
    (2..=3).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphabetic())
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Split the from-to prefix off the query, like upstream's dictionary processor.
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

/// Mozhi answers with this text when it cannot transliterate a direction.
fn transliteration_unsupported(value: &str) -> bool {
    value.starts_with("Direction '") && value.contains("is not supported")
}

impl Engine for Mozhi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let (from, to, text) = direction(query);
        let pairs = vec![
            ("from", from),
            ("to", to),
            ("text", text),
            ("engine", engine_name(&self.meta)),
        ];
        let url = format!(
            "{}/api/translate?{}",
            base_url(&self.meta),
            query_string(&pairs)
        );
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let (from, to, text) = direction(query);
        let json = response.json()?;
        let translation = json_str(&json, "translated-text");
        if translation.is_empty() || text.is_empty() {
            return Ok(Vec::new());
        }
        let base = base_url(&self.meta);
        if !is_http(&base) {
            return Ok(Vec::new());
        }
        let mut content = vec![translation];
        let transliteration = json_str(&json, "target_transliteration");
        if !transliteration.is_empty() && !transliteration_unsupported(&transliteration) {
            content.push(transliteration);
        }
        if let Some(words) = json_path(&json, "word_choices").and_then(|value| value.as_array()) {
            for word in words {
                let definition = json_str(word, "definition");
                if !definition.is_empty() {
                    content.push(definition);
                }
                if let Some(examples) =
                    json_path(word, "examples_target").and_then(|v| v.as_array())
                {
                    for example in examples {
                        if let Some(example) = example.as_str() {
                            let example = example.replace(['<', '>'], "");
                            let example = example.trim_start_matches(['-', ' ']).trim().to_string();
                            if !example.is_empty() {
                                content.push(example);
                            }
                        }
                    }
                }
            }
        }
        let mut builder = ResultBuilder::new(base, text)
            .content(content.join(" "))
            .metadata("engine", engine_name(&self.meta))
            .metadata("from", from)
            .metadata("to", to);
        let synonyms: Vec<String> = json_path(&json, "source_synonyms")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if !synonyms.is_empty() {
            builder = builder.metadata("synonyms", synonyms.join(", "));
        }
        Ok(vec![builder.build()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{
      "translated-text": "Hallo",
      "target_transliteration": "Guten Tag",
      "word_choices": [{"definition": "a greeting", "examples_target": ["- Hello there"]}],
      "source_synonyms": ["hi", "hey"]
    }"##;

    fn engine() -> Arc<Mozhi> {
        Mozhi::from_config(&EngineConfig::new("mozhi", "mozhi", "mz", &["general"])).unwrap()
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
                assert_eq!(result.url, "https://mozhi.aryak.me");
                assert_eq!(result.content, "Hallo Guten Tag a greeting Hello there");
                assert_eq!(
                    result.metadata.get("engine").map(String::as_str),
                    Some("google")
                );
                assert_eq!(
                    result.metadata.get("synonyms").map(String::as_str),
                    Some("hi, hey")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_api_url() {
        let request = engine().build_request(&query("en-de hello"), 1).unwrap();
        assert_eq!(
            request.url,
            "https://mozhi.aryak.me/api/translate?from=en&to=de&text=hello&engine=google"
        );
    }

    #[test]
    fn missing_translation_is_empty() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        let items = engine().parse(&response, &query("en-de hello")).unwrap();
        assert!(items.is_empty());
    }
}
