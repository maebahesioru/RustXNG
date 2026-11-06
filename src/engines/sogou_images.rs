//! Sogou-Images engine, ported from searx/engines/sogou_images.py.
// engine-id: sogou_images

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images"],
    shortcut: "sogoui",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://pic.sogou.com";
const STATE_MARKER: &str = "window.__INITIAL_STATE__";

/// Sogou image search.
pub struct SogouImages {
    meta: EngineMeta,
}

impl SogouImages {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for SogouImages {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let start = pageno.saturating_sub(1) * 48;
        let pairs = vec![("query", query.query_text()), ("start", start.to_string())];
        Ok(EngineRequest::get(format!(
            "{}/pics?{}",
            base.trim_end_matches('/'),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let Some(payload) = extract_state(&body) else {
            return Ok(Vec::new());
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) else {
            return Ok(Vec::new());
        };
        let Some(items) =
            json_path(&json, "searchList.searchList").and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            let title = json_str(item, "title");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let picture = json_str(item, "picUrl");
            let mut builder = ResultBuilder::new(url, title)
                .content(json_str(item, "content_major"))
                .template(Template::Images)
                .metadata("source", json_str(item, "ch_site_name"));
            if !picture.is_empty() {
                builder = builder.thumbnail(picture.clone()).image(picture);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Pulls the JSON object assigned to `window.__INITIAL_STATE__` out of the page.
///
/// The blob is not terminated by `};` (sogou writes `}</script>`), so a regex
/// cannot delimit it: walk the braces instead, skipping over string literals.
fn extract_state(body: &str) -> Option<&str> {
    let marker = body.find(STATE_MARKER)?;
    let rest = &body[marker + STATE_MARKER.len()..];
    let open = rest.find('{')?;
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes.iter().enumerate().skip(open) {
        let byte = *byte;
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return rest.get(open..=offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn engine() -> Arc<SogouImages> {
        SogouImages::from_config(&EngineConfig::new(
            "sogou images",
            "sogou_images",
            "sogoui",
            &["images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_initial_state() {
        let body = r##"<html><script>window.__INITIAL_STATE__ = {"searchList":{"searchList":[{"url":"https://example.com/photo","picUrl":"https://img.example.com/cat.jpg","title":"Cat photo","content_major":"A cat","ch_site_name":"example.com"}]}};</script></html>"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/photo");
                assert_eq!(result.title, "Cat photo");
                assert_eq!(result.content, "A cat");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://img.example.com/cat.jpg")
                );
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://img.example.com/cat.jpg")
                );
                assert_eq!(
                    result.metadata.get("source").map(String::as_str),
                    Some("example.com")
                );
            }
            _ => panic!("expected an image result"),
        }
    }

    #[test]
    fn parses_a_state_blob_without_a_trailing_semicolon() {
        // sogou writes "</script>" straight after the closing brace, so the
        // blob cannot be delimited by a regex looking for "};".
        let body = r##"<script>window.__INITIAL_STATE__={"route":{"name":"searchList"},"searchList":{"searchList":[{"url":"http://www.duitang.com/people/mblog/1/detail/","picUrl":"https://img.example.com/a.jpg","title":"cat","content_major":"","ch_site_name":"堆糖"}]}}</script><script src="//x.js"></script>"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn braces_inside_strings_do_not_end_the_blob() {
        let body = r##"<script>window.__INITIAL_STATE__={"searchList":{"searchList":[{"url":"https://example.com/a","title":"a } b","content_major":"{x}"}]}}</script>"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn missing_state_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(request.url, "https://pic.sogou.com/pics?query=cat&start=48");
    }
}
