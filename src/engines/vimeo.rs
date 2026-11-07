//! Vimeo engine.
//!
//! The public API (`api.vimeo.com/search`) rejects every anonymous caller with
//! `8003` and needs a JWT that the site mints per session, so this engine reads
//! the server-rendered search page instead. The page itself is behind a
//! "CAPTCHA Challenge" wall for plain HTTP clients; the browser solver passes it
//! and the rendered DOM carries `a[data-testid="clip-result"]` cards, which is
//! what `parse` reads. `needs_render` recognises the wall so the aggregator
//! sends the URL to the solver with the engine's `solver_dwell`.
// engine-id: vimeo

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "vm",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://vimeo.com";
const ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

/// Vimeo's clip search page.
pub struct Vimeo {
    meta: EngineMeta,
}

impl Vimeo {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Vimeo {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![("q", query.query_text())];
        if pageno > 1 {
            pairs.push(("page", pageno.to_string()));
        }
        Ok(
            EngineRequest::get(format!("{}/search?{}", self.base_url(), query_string(&pairs)))
                .header("Accept", ACCEPT),
        )
    }

    fn needs_render(&self, response: &EngineResponse) -> bool {
        // Plain HTTP clients get a 403 whose body is Vimeo's
        // "CAPTCHA Challenge" page. There is no result markup in it, so the
        // only way to read the hits is to let the browser load the page.
        let body = response.text();
        !body.contains("clip-result") && (response.status == 403 || body.contains("CAPTCHA Challenge"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        // The rendered markup is a Chakra card per clip:
        //   a[data-testid="clip-result"][href="https://vimeo.com/<id>"]
        //     img[src="https://i.vimeocdn.com/video/…"]
        //     p[title="RUST"]                 (title)
        //     p                               (author)
        //     p                               (views • date)
        // The class names are build hashes, so only the test id and the
        // `title` attribute are stable.
        let Ok(cards) = select_elements(&html, "a[data-testid='clip-result']") else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for card in cards {
            let url = attr(card, "href").unwrap_or_default();
            let title = attr_of(card, "p[title]", "title")
                .or_else(|| text_of(card, "p[title]"))
                .unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let paragraphs = children(card, "p");
            let author = paragraphs
                .get(1)
                .map(|element| text(*element))
                .unwrap_or_default();
            let meta_line = paragraphs
                .get(2)
                .map(|element| text(*element))
                .unwrap_or_default();
            let mut builder = ResultBuilder::new(url.clone(), title)
                .template(Template::Videos)
                .author(author)
                .content(meta_line.clone())
                .thumbnail(attr_of(card, "img", "src").unwrap_or_default());
            if let Some((views, date)) = meta_line.split_once('•') {
                builder = builder
                    .metadata("views", views.trim().to_string())
                    .published_date(parse_listed_date(date.trim()));
            }
            let id = video_id(&url);
            if !id.is_empty() {
                builder = builder.metadata(
                    "iframe_src",
                    format!("https://player.vimeo.com/video/{id}"),
                );
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The clip id is the last path segment of the clip link.
fn video_id(url: &str) -> String {
    url::Url::parse(url)
        .map(|url| url.path().trim_matches('/').to_string())
        .unwrap_or_default()
}

/// The card prints the upload date as "September 7, 2015".
fn parse_listed_date(value: &str) -> String {
    chrono::NaiveDate::parse_from_str(value, "%B %e, %Y")
        .map(|date| date.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<html><body>
      <a class="chakra-card" data-testid="clip-result" href="https://vimeo.com/263989277">
        <img src="https://i.vimeocdn.com/video/1.jpg" alt="">
        <div class="chakra-card__body">
          <p class="chakra-text" title="RUST">RUST</p>
          <p class="chakra-text">Ariane Prin</p>
          <p class="chakra-text">13.2K views • September 7, 2015</p>
        </div>
      </a>
      <a class="chakra-card" data-testid="clip-result" href="https://vimeo.com/2">
        <p class="chakra-text" title="Rust Harvest">Rust Harvest</p>
        <p class="chakra-text">Yuhei Kodaka</p>
        <p class="chakra-text">6,772 views • April 10, 2018</p>
      </a>
    </body></html>"#;

    fn engine() -> Arc<Vimeo> {
        Vimeo::from_config(&EngineConfig::new("vimeo", "vimeo", "vm", &["videos"])).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_rendered_clip_cards() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "RUST");
                assert_eq!(result.url, "https://vimeo.com/263989277");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.author.as_deref(), Some("Ariane Prin"));
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://i.vimeocdn.com/video/1.jpg")
                );
                assert_eq!(result.published_date.as_deref(), Some("2015-09-07"));
                assert_eq!(
                    result.metadata.get("views").map(String::as_str),
                    Some("13.2K views")
                );
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://player.vimeo.com/video/263989277")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn the_captcha_wall_asks_for_a_rendered_page() {
        let wall = EngineResponse::new(
            403,
            BASE_URL,
            b"<html><title>Vimeo / CAPTCHA Challenge</title></html>".to_vec(),
        );
        assert!(engine().needs_render(&wall));
        let rendered = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        assert!(!engine().needs_render(&rendered));
    }

    #[test]
    fn request_carries_the_query_and_paging() {
        let request = engine().build_request(&query(), 2).unwrap();
        assert_eq!(request.url, "https://vimeo.com/search?q=rust&page=2");
        assert_eq!(
            request.headers.get("Accept").unwrap().to_str().unwrap(),
            ACCEPT
        );
    }

    #[test]
    fn the_first_page_omits_the_page_parameter() {
        let request = engine().build_request(&query(), 1).unwrap();
        assert_eq!(request.url, "https://vimeo.com/search?q=rust");
    }

    #[test]
    fn a_page_without_cards_is_not_an_error() {
        let response = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
