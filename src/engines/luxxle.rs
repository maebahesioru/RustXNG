//! Luxxle engine, ported from `searx/engines/luxxle.py` (web search).
//!
//! Upstream picks the endpoint from `luxxle_categ` (`search`, `images`,
//! `videos`, `news`); this port implements the main `search` variant and reads
//! the category from the `search_type` config key.
//!
//! Upstream scrapes short-lived telemetry data (`ip`, `timeOf`,
//! `authorization`, `preferencesCookie`) from Luxxle's search page before every
//! request. The frozen `build_request` interface cannot perform that extra
//! round trip, so those values are read from the same-named config options.
// engine-id: luxxle

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "lx",
    paging: false,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://luxxle.com";

/// Luxxle, an American search engine.
pub struct Luxxle {
    meta: EngineMeta,
    categ: String,
}

impl Luxxle {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config
            .opt_str("search_type")
            .unwrap_or("search")
            .to_string();
        if !matches!(categ.as_str(), "search" | "images" | "videos" | "news") {
            return Err(format!("luxxle: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for Luxxle {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        // `/load_search.php` is the JSON endpoint the page calls from script and
        // it rejects every request that does not carry Luxxle's own telemetry
        // (`ip`, `timeOf`, `authorization`) — it answers 403 for all of them.
        // The server-rendered `/luxsearch?q=` page carries the same hits and is
        // reachable once the Cloudflare challenge is passed.
        if self.categ != "search" {
            return Err(EngineError::Message(
                "luxxle: only the web search page is implemented".to_string(),
            ));
        }
        let pairs: Vec<(&str, String)> = vec![("q", query.query_text())];
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/luxsearch?{}",
            query_string(&pairs)
        )))
    }

    fn needs_render(&self, response: &EngineResponse) -> bool {
        // The page ships the whole search UI and then fills #mainResults from
        // `/load_search.php`. Until that script has run the body is a shell:
        // it names the endpoint but carries no result container.
        let body = response.text();
        body.contains("load_search.php") && !body.contains("resultsContainer")
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        if self.categ != "search" {
            return Ok(Vec::new());
        }
        let html = parse_html(&response.text());
        // The current result markup is
        //   div#mainResults > div.resultsContainer > div.searchResultContent
        //     a.urlnamelink[href="/redirect?url=…"]  (title)
        //     div.urlSnippet                          (snippet)
        // The previous selectors (`div[class~='urlAddressLink'] a`,
        // `div[class~='urlname']`) matched the *displayed* URL widget, which
        // no longer carries the anchor, so every item was dropped.
        let Ok(items) = select_elements(&html, "div#mainResults div.resultsContainer") else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let Some(anchor) = child(item, "a.urlnamelink")
                .or_else(|| child(item, "a[href*='/redirect?url=']"))
            else {
                continue;
            };
            let url = unwrap_redirect(&attr(anchor, "href").unwrap_or_default());
            let title = text(anchor);
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(text_of(item, "div[class~='urlSnippet']").unwrap_or_default())
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Luxxle wraps outgoing links in `/redirect?url=<url>`.
fn unwrap_redirect(url: &str) -> String {
    match url.find("?url=") {
        Some(index) => percent_decode(&url[index + 5..]),
        None => url.to_string(),
    }
}

fn percent_decode(value: &str) -> String {
    let value = value.replace('+', " ");
    percent_encoding::percent_decode_str(&value)
        .decode_utf8_lossy()
        .to_string()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<html><body><div id="mainResults">
      <div class="resultsContainer">
        <div class="searchResultContent">
          <a class="urlnamelink" href="/redirect?url=https%3A%2F%2Fexample.com%2Frust">Rust Programming</a>
          <div class="urlSnippet">A fast systems language.</div>
        </div>
      </div>
    </div></body></html>"#;

    fn engine() -> Arc<Luxxle> {
        Luxxle::from_config(&EngineConfig::new("luxxle", "luxxle", "lx", &["general"])).unwrap()
    }

    #[test]
    fn parses_search_results_and_unwraps_redirects() {
        let response = EngineResponse::new(
            200,
            "https://luxxle.com/search",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust Programming");
                assert_eq!(result.url, "https://example.com/rust");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_a_get_request_for_the_rendered_search_page() {
        // The load_search.php endpoint answers 403 to every client that does
        // not carry Luxxle\u{27}s own telemetry, so the engine reads the page the
        // browser renders instead.
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(request.method, crate::engine::Method::Get);
        assert_eq!(request.url, "https://luxxle.com/luxsearch?q=rust");
    }

    #[test]
    fn a_shell_without_results_is_empty_not_an_error() {
        // The unrendered page is a 200 shell: no result container, no items.
        let response = EngineResponse::new(
            200,
            "https://luxxle.com/luxsearch?q=rust",
            b"<html><body><div id=\"mainResults\"></div></body></html>".to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }
}
