//! Fireball engine, ported from `searx/engines/fireball.py` (web search).
//!
//! Upstream supports the categories `web`, `news` and `videos` through the
//! `fireball_category` setting; this port implements the main `web` variant and
//! reads the category from the `search_type` config key. All categories are
//! returned by the same request.
//!
//! Upstream obtains a `fireball` settings cookie from `POST /settings` (cached
//! for a month). The frozen `build_request` interface cannot perform that extra
//! round trip, so the cookie is read from the `fireball_cookie` config option.
// engine-id: fireball

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "fb",
    paging: false,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://fireball.com";

/// Fireball, a German privacy focused search engine.
pub struct Fireball {
    meta: EngineMeta,
    categ: String,
}

impl Fireball {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config.opt_str("search_type").unwrap_or("web").to_string();
        if !matches!(categ.as_str(), "web" | "news" | "videos") {
            return Err(format!("fireball: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for Fireball {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        // `/getResults/` answers 405 now; the results live in the HTML of the
        // search page itself (div.card.my-1 with a window.open() handler).
        let args = [
            ("f", self.categ.clone()),
            ("q", query.query_text()),
        ];
        let query = query_string(&args);
        let mut request = EngineRequest::get(format!("{BASE_URL}/search?{query}"))
            .header("Referer", &format!("{BASE_URL}/"))
            .header("Accept", "text/html,application/xhtml+xml");
        if let Some(cookie) = self.meta.opt_str("fireball_cookie") {
            request = request.cookie("fireball", cookie);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        // The search page is server rendered HTML: every hit is a
        // `div.card.my-1` whose clickable `div[onclick]` carries the real URL
        // inside `window.open('…')`. The `/getResults/` JSON endpoint this
        // engine used to call answers 405.
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        for card in select_elements(&html, "div#webresults div.card")? {
            let Some(url) = child(card, "div[onclick]")
                .and_then(|node| attr(node, "onclick"))
                .and_then(|script| window_open_target(&script))
            else {
                continue;
            };
            let Some(title) = text_of(card, "h4") else {
                continue;
            };
            if !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title);
            if let Some(content) = text_of(card, "p.mb-0") {
                builder = builder.content(content);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The target of the first `window.open('…')` call in an inline handler.
fn window_open_target(script: &str) -> Option<String> {
    let start = script.find("window.open(")? + "window.open(".len();
    let rest = script[start..].trim_start();
    let quote = rest.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let rest = &rest[quote.len_utf8()..];
    let end = rest.find(quote)?;
    let url = rest[..end].trim();
    (!url.is_empty()).then(|| url.to_string())
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<html><body><div id="webresults">
      <div class="card my-1" style="border: 0px;"><div class="card-body">
        <div onclick="window.open('https://example.com/rust', '_blank');return false;">
          <h4 class="mb-1 text-truncate">Rust Programming Language</h4>
          <div class="font-13 text-muted mb-1 text-truncate">https://example.com/rust</div>
          <p class="mb-0 mt-2">Rust is blazingly fast and memory-efficient.</p>
        </div>
      </div></div>
      <div class="card my-1"><div class="card-body">
        <div onclick="window.open('https://example.org/other', '_blank');return false;">
          <h4>Second hit</h4>
          <p class="mb-0 mt-2">Another description.</p>
        </div>
      </div></div>
    </div></body></html>"#;

    fn engine() -> Arc<Fireball> {
        Fireball::from_config(&EngineConfig::new(
            "fireball",
            "fireball",
            "fb",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_server_rendered_cards() {
        let response = EngineResponse::new(
            200,
            "https://fireball.com/search?f=web&q=rust",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust Programming Language");
                assert_eq!(result.url, "https://example.com/rust");
                assert_eq!(
                    Some(result.content.as_str()),
                    Some("Rust is blazingly fast and memory-efficient.")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => assert_eq!(result.url, "https://example.org/other"),
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn cards_without_a_window_open_target_are_skipped() {
        let body = r#"<div id="webresults"><div class="card my-1">
            <h4>No link here</h4></div></div>"#;
        let response = EngineResponse::new(200, "https://fireball.com/search", body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn builds_the_request_with_a_referer() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.starts_with("https://fireball.com/search?"));
        assert!(request.url.ends_with("q=rust"));
        assert_eq!(
            request
                .headers
                .get("referer")
                .map(|value| value.to_str().unwrap()),
            Some("https://fireball.com/")
        );
    }
}
