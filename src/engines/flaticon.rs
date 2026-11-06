//! Flaticon engine, ported from searx/engines/flaticon.py.
//!
//! The AJAX endpoint of the icon search is used.
// engine-id: flaticon

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images", "icons"],
    shortcut: "fli",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.flaticon.com";

pub struct Flaticon {
    meta: EngineMeta,
}

impl Flaticon {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// The base URL without its trailing slash.
    fn base(&self) -> String {
        let base = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string());
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for Flaticon {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let text = query.query_text();
        let base = self.base();
        let pairs: Vec<(&str, String)> = vec![("word", text.clone())];
        let url = format!(
            "{base}/ajax/search/{}?{}",
            pageno.max(1),
            query_string(&pairs)
        );
        let referer = format!("{base}/search?word={}", encode(&text));
        // The AJAX endpoint answers 403 unless the request looks like a
        // browser navigation: at least two Sec-Fetch-* hints must be present
        // (one alone is not enough), so all three are sent.
        Ok(EngineRequest::get(url)
            .header("Referer", &referer)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept", "application/json, text/plain, */*")
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-origin"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json.get("items").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "name");
            let url = fix_url(&json_str(item, "slug"));
            if title.is_empty() || !url.starts_with("http") {
                continue;
            }
            let img_src = fix_url(&json_str(item, "png512"));
            let thumbnail = fix_url(&json_str(item, "png"));
            let mut builder = ResultBuilder::new(url, title)
                .content(icon_tags(item))
                .author(json_str(item, "team_name"))
                .template(Template::Images)
                .metadata("img_format", "PNG");
            if !img_src.is_empty() {
                builder = builder.image(img_src);
            }
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream's _fix_url: the JSON escapes the slashes.
fn fix_url(url: &str) -> String {
    url.replace("\\/", "/")
}

/// The non empty tags of an icon, joined like upstream.
fn icon_tags(item: &serde_json::Value) -> String {
    let Some(tags) = json_path(item, "tags").and_then(|value| value.as_array()) else {
        return String::new();
    };
    tags.iter()
        .map(|tag| json_str(tag, "tag"))
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"items": [
      {"name": "Cat", "slug": "https://www.flaticon.com/free-icon/cat_1",
       "png": "https://cdn.example.com/cat.png",
       "png512": "https://cdn.example.com/cat512.png",
       "team_name": "Example Team",
       "tags": [{"tag": "animal"}, {"tag": "pet"}, {"tag": ""}]},
      {"name": "", "slug": "https://www.flaticon.com/free-icon/x_2"}
    ]}"##;

    fn engine() -> Arc<Flaticon> {
        let config = EngineConfig::new("flaticon", "flaticon", "fli", &["icons"]);
        Flaticon::from_config(&config).unwrap()
    }

    #[test]
    fn parses_icon_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.flaticon.com/free-icon/cat_1");
                assert_eq!(result.title, "Cat");
                assert_eq!(result.content, "animal, pet");
                assert_eq!(result.author.as_deref(), Some("Example Team"));
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://cdn.example.com/cat512.png")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://cdn.example.com/cat.png")
                );
                assert_eq!(
                    result.metadata.get("img_format").map(String::as_str),
                    Some("PNG")
                );
                assert_eq!(result.template, Template::Images);
            }
            _ => panic!("expected an icon result"),
        }
    }

    #[test]
    fn builds_the_ajax_request() {
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://www.flaticon.com/ajax/search/2?word=cat"
        );
        let referer = request
            .headers
            .get("referer")
            .and_then(|value| value.to_str().ok());
        assert_eq!(referer, Some("https://www.flaticon.com/search?word=cat"));
    }

    #[test]
    fn unescapes_the_json_slashes() {
        assert_eq!(fix_url("a\\/b"), "a/b");
    }
}
