//! S1search rampjs engine, ported from searx/engines/s1search_rampjs.py.
//!
//! Handles the mirrors that embed the payload in the __RAMPJS__ variable.

// engine-id: s1search_rampjs

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "s1j",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://search.answers.com";

/// System1's JavaScript based search front end.
pub struct S1SearchRampjs {
    meta: EngineMeta,
}

impl S1SearchRampjs {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for S1SearchRampjs {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        Ok(EngineRequest::get(format!(
            "{}/?{}",
            self.base_url(),
            query_string(&[("q", query.query_text()), ("page", pageno.to_string())])
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let Some(data) = payload(&response.text()) else {
            return Ok(Vec::new());
        };
        let Some(regions) = json_path(&data, "search.regions").and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let Some(mainline) = regions
            .iter()
            .find(|region| json_str(region, "name") == "mainline")
        else {
            return Ok(Vec::new());
        };
        let Some(groups) = json_path(mainline, "groups").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut items = Vec::new();
        for group in groups {
            let entries = match json_path(group, "results").and_then(|value| value.as_array()) {
                Some(entries) => entries,
                None => continue,
            };
            match json_str(group, "result_type").as_str() {
                "web" => {
                    for entry in entries {
                        let mut url = json_str(entry, "url");
                        if url.is_empty() {
                            url = json_str(entry, "clickUrl");
                        }
                        let title = strip_tags(&json_str(entry, "title"));
                        if title.is_empty() || !is_http_url(&url) {
                            continue;
                        }
                        let content = strip_tags(&json_str(entry, "description"));
                        items.push(ResultBuilder::new(url, title).content(content).build());
                    }
                }
                "aylf" => {
                    for entry in entries {
                        let term = strip_tags(&json_str(entry, "term"));
                        if !term.is_empty() {
                            items.push(EngineItem::Suggestion(term));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(items)
    }
}

/// The `{...}` object literal that starts at `start`, matched by braces.
fn object_at(text: &str, start: usize) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + index + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The JSON value of the payload embedded in the page.
fn payload(body: &str) -> Option<serde_json::Value> {
    let marker = body.find("__RAMPJS__")?;
    let rest = &body[marker..];
    let start = rest.find('{')?;
    let slice = object_at(rest, start)?;
    let document = match serde_json::from_str::<serde_json::Value>(slice) {
        Ok(document) => document,
        Err(_) => {
            // The live payload is a JavaScript object literal whose outer keys
            // are unquoted (`{ response: {...} }`), which JSON rejects: descend
            // into the first value object and parse that instead.
            let colon = slice.find(':')?;
            let inner = slice[colon + 1..].find('{').map(|offset| offset + colon + 1)?;
            serde_json::from_str::<serde_json::Value>(object_at(slice, inner)?).ok()?
        }
    };
    Some(document.get("response").cloned().unwrap_or(document))
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body><script>
window.__RAMPJS__ = { "response": {"search": {"regions": [
  {"name": "mainline", "groups": [
    {"result_type": "web", "results": [
      {"url": "https://example.test/one", "title": "First <b>result</b>",
       "description": "Some <em>text</em>"},
      {"clickUrl": "https://example.test/two", "title": "Second result", "description": ""},
      {"url": "/relative", "title": "", "description": "dropped"}
    ]},
    {"result_type": "aylf", "results": [{"term": "related thing"}]},
    {"result_type": "unknown", "results": [{"url": "https://example.test/no"}]}
  ]}
]}} };
</script></body></html>"##;

    fn engine() -> Arc<S1SearchRampjs> {
        let config = EngineConfig::new("s1search", "s1search_rampjs", "s1j", &["general"]);
        S1SearchRampjs::from_config(&config).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_embedded_payload() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 3);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "First result");
                assert_eq!(result.url, "https://example.test/one");
                assert_eq!(result.content, "Some text");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => assert_eq!(result.url, "https://example.test/two"),
            _ => panic!("expected a result"),
        }
        match &items[2] {
            EngineItem::Suggestion(term) => assert_eq!(term, "related thing"),
            _ => panic!("expected a suggestion"),
        }
        let empty = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine().parse(&empty, &query()).unwrap().is_empty());
    }

    #[test]
    fn parses_the_unquoted_object_literal_payload() {
        // The live page embeds a JavaScript object literal whose outer key is
        // not quoted, which plain JSON parsing rejects.
        const LIVE: &str = r##"<html><script>
window.__RAMPJS__ = { response: {"search": {"regions": [
  {"name": "mainline", "groups": [
    {"result_type": "web", "results": [
      {"url": "https://example.test/live", "title": "Live <b>hit</b>", "description": "Body"}
    ]}
  ]}
]}} };
</script></html>"##;
        let response = EngineResponse::new(200, BASE_URL, LIVE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.test/live");
                assert_eq!(result.title, "Live hit");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let request = engine().build_request(&query(), 2).unwrap();
        assert_eq!(request.url, "https://search.answers.com/?q=rust&page=2");
    }
}
