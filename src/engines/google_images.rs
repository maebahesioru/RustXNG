//! Google Images engine, ported from searx/engines/google_images.py.
//!
//! Upstream still asks the mobile (WML) endpoint for `tbm=isch`, but that
//! endpoint now answers 403 to every client, and the imgres links it used to
//! return are gone. The desktop endpoint renders one card per image with
//! `data-attrid="images universal"`, the page address in `data-lpage` and the
//! thumbnail in an `<img>`, so that is what this engine reads. The card only
//! exists once the page has run its scripts, hence `needs_render`.
// engine-id: google_images

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images", "web"],
    shortcut: "goi",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.google.com/search";
/// One rendered image card. `data-attrid` is a stable attribute name, unlike
/// the hashed class names next to it.
const CARD: &str = "div[data-attrid='images universal']";

/// Google Images.
pub struct GoogleImages {
    meta: EngineMeta,
}

impl GoogleImages {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for GoogleImages {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let mut pairs: Vec<(&str, String)> = vec![("q", query.query_text())];
        if language != "all" {
            pairs.push(("hl", language.clone()));
        }
        pairs.push(("tbm", "isch".to_string()));
        let start = (pageno.max(1) - 1) * 20;
        if start > 0 {
            pairs.push(("start", start.to_string()));
        }
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "d",
                TimeRange::Week => "w",
                TimeRange::Month => "m",
                TimeRange::Year => "y",
            };
            pairs.push(("tbs", format!("qdr:{value}")));
        }
        if query.safesearch > 0 {
            pairs.push(("safe", "active".to_string()));
        }
        let url = format!("{BASE_URL}?{}", query_string(&pairs));
        Ok(EngineRequest::get(url)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"))
    }

    /// The cards are filled in by the page's own scripts, so a plain fetch
    /// brings back a shell that has nothing to parse.
    fn needs_render(&self, response: &EngineResponse) -> bool {
        let body = response.text();
        !body.contains("images universal") && !body.contains("did not match any documents")
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        if body.contains("detected unusual traffic")
            || (body.len() < 20_000 && body.contains("/sorry/index"))
        {
            return Err(EngineError::Captcha);
        }
        let html = parse_html(&body);
        let mut results = Vec::new();
        for card in select_elements(&html, CARD)? {
            let Some(url) = attr(card, "data-lpage").filter(|value| value.starts_with("http")) else {
                continue;
            };
            let img_src = attr_of(card, "img", "src").unwrap_or_default();
            if img_src.is_empty() {
                continue;
            }
            let mut title = attr_of(card, "img", "alt").unwrap_or_default();
            if title.trim().is_empty() {
                title = host_of(&url);
            }
            if title.is_empty() {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .image(img_src.clone())
                .thumbnail(img_src)
                .template(Template::Images);
            if let Some(source) = attr(card, "data-lpage")
                .as_deref()
                .and_then(|value| value.split("://").nth(1))
                .and_then(|rest| rest.split('/').next())
            {
                builder = builder.metadata("source", source.to_string());
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The host of a URL.
fn host_of(raw: &str) -> String {
    let Some((_, rest)) = raw.split_once("://") else {
        return String::new();
    };
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body>
      <div data-attrid="images universal" data-docid="abc" data-lpage="https://en.wikipedia.org/wiki/Cat">
        <a href="/goto?url=CAES"><img alt="Cat - Wikipedia" src="https://encrypted-tbn0.gstatic.com/images?q=tbn:AAA&amp;s"></a>
      </div>
      <div data-attrid="images universal" data-docid="def">
        <img alt="no page" src="https://encrypted-tbn0.gstatic.com/images?q=tbn:BBB">
      </div>
    </body></html>"##;

    fn engine() -> Arc<GoogleImages> {
        let config = EngineConfig::new("google images", "google_images", "goi", &["images"]);
        GoogleImages::from_config(&config).unwrap()
    }

    #[test]
    fn parses_the_rendered_cards() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://en.wikipedia.org/wiki/Cat");
                assert_eq!(result.title, "Cat - Wikipedia");
                assert!(result
                    .img_src
                    .as_deref()
                    .is_some_and(|src| src.contains("encrypted-tbn0.gstatic.com")));
            }
            other => panic!("expected a result, got {other:?}"),
        }
    }

    #[test]
    fn a_shell_without_cards_asks_for_a_rendered_page() {
        let shell = EngineResponse::new(200, BASE_URL, b"<html><body></body></html>".to_vec());
        assert!(engine().needs_render(&shell));
        let rendered = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        assert!(!engine().needs_render(&rendered));
    }

    #[test]
    fn the_interstitial_is_a_captcha() {
        let page = EngineResponse::new(
            200,
            BASE_URL,
            b"<html><body>Our systems have detected unusual traffic</body></html>".to_vec(),
        );
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine().parse(&page, &query),
            Err(EngineError::Captcha)
        ));
    }

    #[test]
    fn request_carries_the_query_and_paging() {
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.starts_with(BASE_URL));
        assert!(request.url.contains("tbm=isch"));
        assert!(request.url.contains("start=20"));
    }
}
