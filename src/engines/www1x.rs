//! 1x engine, ported from searx/engines/www1x.py.
// engine-id: www1x

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["stock images"],
    shortcut: "1x",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 3.0,
};

const BASE_URL: &str = "https://1x.com";
const GALLERY_URL: &str = "https://gallery.1x.com/";
const DATA_RE: &str = "(?s)<data[^>]*>(.*?)</data>";

/// 1x stock photography.
pub struct Www1x {
    meta: EngineMeta,
}

impl Www1x {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for Www1x {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let search = match self.meta.search_url.as_deref() {
            Some(url) => url.to_string(),
            None => format!(
                "{}/backend/search.php?{{query}}",
                base.trim_end_matches('/')
            ),
        };
        let encoded = query_string(&[("q", query.query_text())]);
        Ok(EngineRequest::get(search.replace("{query}", &encoded)))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let pattern = regex::Regex::new(DATA_RE)
            .map_err(|error| EngineError::Parse(format!("invalid data pattern: {error}")))?;
        let Some(captures) = pattern.captures(&body) else {
            return Ok(Vec::new());
        };
        let Some(payload) = captures.get(1) else {
            return Ok(Vec::new());
        };
        // The XML payload keeps the result markup as escaped text, wrapped in a
        // CDATA section that would otherwise be swallowed as a bogus comment.
        let payload = payload
            .as_str()
            .trim()
            .trim_start_matches("<![CDATA[")
            .trim_end_matches("]]>");
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let html = parse_html(&decode_entities(payload));
        let mut results = Vec::new();
        for link in select_elements(&html, "a")? {
            let Some(url) = attr(link, "href").and_then(|href| absolute_url(&href, base)) else {
                continue;
            };
            // The gallery links carry an icon, not a caption, so the title has
            // to fall back to the image alt text and finally to the URL itself.
            let title = {
                let caption = text(link);
                if !caption.is_empty() {
                    caption
                } else {
                    child(link, "img")
                        .and_then(|image| attr(image, "alt"))
                        .filter(|alt| !alt.is_empty())
                        .unwrap_or_else(|| fallback_title(&url))
                }
            };
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let thumbnail = child(link, "img")
                .and_then(|image| attr(image, "src"))
                .and_then(|src| absolute_url(&src.replace(base, ""), GALLERY_URL));
            let mut builder = ResultBuilder::new(url, title).template(Template::Images);
            if let Some(thumbnail) = thumbnail {
                builder = builder.thumbnail(thumbnail.clone()).image(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// A readable title for a link that has no caption of its own.
fn fallback_title(url: &str) -> String {
    let path = url.split("://").nth(1).unwrap_or(url);
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match segments.len() {
        0 | 1 => path.to_string(),
        n => format!("{} {}", segments[n - 2], segments[n - 1]),
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn engine() -> Arc<Www1x> {
        Www1x::from_config(&EngineConfig::new("1x", "www1x", "1x", &["stock images"])).unwrap()
    }

    #[test]
    fn parses_the_escaped_xml_payload() {
        let body = r##"<?xml version="1.0" encoding="UTF-8"?><response><data>&lt;a href="/photo/123"&gt;&lt;img src="https://1x.com/media/123.jpg"&gt;A landscape&lt;/a&gt;&lt;a href="/photo/456"&gt;B portrait&lt;/a&gt;</data></response>"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://1x.com/photo/123");
                assert_eq!(result.title, "A landscape");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://gallery.1x.com/media/123.jpg")
                );
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://gallery.1x.com/media/123.jpg")
                );
            }
            _ => panic!("expected an image result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://1x.com/photo/456");
                assert_eq!(result.title, "B portrait");
                assert!(result.thumbnail.is_none());
            }
            _ => panic!("expected an image result"),
        }
    }

    #[test]
    fn cdata_payload_without_captions_is_parsed() {
        let body = r##"<?xml version="1.0" encoding="UTF-8"?><response><data><![CDATA[<a href="/photo/203885"><img src="https://1x.com/images/a-sq.jpg"><span><i class="fas fa-image"></i></span></a><a href="/photo/40103"><img alt="Harbour at dusk" src="https://1x.com/images/b-sq.jpg"></a>]]></data></response>"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://1x.com/photo/203885");
                assert_eq!(result.title, "photo 203885");
                assert_eq!(result.template, Template::Images);
            }
            _ => panic!("expected an image result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://1x.com/photo/40103");
                assert_eq!(result.title, "Harbour at dusk");
            }
            _ => panic!("expected an image result"),
        }
    }

    #[test]
    fn builds_the_search_url() {
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(request.url, "https://1x.com/backend/search.php?q=cat");
    }
}
