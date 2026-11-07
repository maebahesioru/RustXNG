//! Google Play engine, ported from `searx/engines/google_play.py` (apps).
//!
//! Upstream supports the categories `apps` and `movies` through the
//! `play_categ` setting; this port implements the main `apps` variant and reads
//! the category from the `search_type` config key.
// engine-id: google_play

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "apps"],
    shortcut: "gpa",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://play.google.com";

/// Google Play, apps and movies.
pub struct GooglePlay {
    meta: EngineMeta,
    categ: String,
}

impl GooglePlay {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config.opt_str("search_type").unwrap_or("apps").to_string();
        if !matches!(categ.as_str(), "apps" | "movies") {
            return Err(format!("google_play: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }

    /// The search URL of upstream, `{query}` already encoded.
    fn search_url(&self, encoded_query: &str) -> String {
        format!("{BASE_URL}/store/search?q={encoded_query}&c={}", self.categ)
    }
}

impl Engine for GooglePlay {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Ok(
            EngineRequest::get(self.search_url(&encode(&query.query_text())))
                .cookie("CONSENT", "YES+"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        if self.categ == "movies" {
            return Ok(parse_movies(&html));
        }
        Ok(parse_apps(&html, &self.categ))
    }
}

/// `response_apps` of upstream.
fn parse_apps(html: &scraper::Html, categ: &str) -> Vec<EngineItem> {
    let mut results = Vec::new();
    // Google Play serves a "not available" page with this container.
    if select_one(html, "div.v6DsQb").ok().flatten().is_some() {
        return results;
    }
    let search_url = format!("{BASE_URL}/store/search?q=&c={categ}");
    if let Ok(Some(spot)) = select_one(html, "div.ipRz4") {
        let url = attr_of(spot, "a.Qfxief", "href")
            .and_then(|href| absolute_url(&href, &search_url))
            .unwrap_or_default();
        let title = text_of(spot, "div.vWM94c").unwrap_or_default();
        if !title.is_empty() && is_http_url(&url) {
            results.push(
                ResultBuilder::new(url, title)
                    .content(text_of(spot, "div.LbQbAe").unwrap_or_default())
                    .image(attr_of(spot, "img.T75of", "src").unwrap_or_default())
                    .build(),
            );
        }
    }
    if let Ok(entries) = select_elements(html, "c-wiz[jsrenderer='RBsfwb'] div[role='listitem']") {
        for entry in entries {
            let url = attr_of(entry, "a", "href")
                .and_then(|href| absolute_url(&href, &search_url))
                .unwrap_or_default();
            let title = text_of(entry, "span.DdYX5").unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let image = attr_of(entry, "img.T75of", "src").unwrap_or_default();
            results.push(
                ResultBuilder::new(url, title)
                    .content(text_of(entry, "span.wMUdtb").unwrap_or_default())
                    .image(image)
                    .build(),
            );
        }
    }
    if let Ok(suggestions) =
        select_elements(html, "c-wiz[jsrenderer='qyd4Kb'] div[class~='ULeU3b']")
    {
        for suggestion in suggestions {
            let value = text_of(suggestion, "div[class~='Epkrse']").unwrap_or_default();
            if !value.is_empty() {
                results.push(EngineItem::Suggestion(value));
            }
        }
    }
    results
}

/// `response_movies` of upstream.
fn parse_movies(html: &scraper::Html) -> Vec<EngineItem> {
    let mut results = Vec::new();
    let Ok(sections) = select_elements(html, "c-wiz section header") else {
        return results;
    };
    for header in sections {
        let Some(section) = header.parent().and_then(scraper::ElementRef::wrap) else {
            continue;
        };
        let section_name = text(header);
        let Ok(links) = select_elements_at(section, "a") else {
            continue;
        };
        for link in links {
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let url = absolute_url(&format!("{BASE_URL}{href}"), BASE_URL).unwrap_or_default();
            let divs: Vec<scraper::ElementRef<'_>> = children(link, "div");
            let title = divs
                .get(1)
                .and_then(|div| text_of(*div, "div[title]"))
                .unwrap_or_default();
            let metadata = divs
                .get(1)
                .and_then(|div| text_of(*div, "div[class]"))
                .unwrap_or_default();
            let thumbnail = divs
                .first()
                .and_then(|div| attr_of(*div, "img", "src"))
                .unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(section_name.clone())
                    .thumbnail(thumbnail)
                    .template(Template::Videos)
                    .metadata("metadata", metadata)
                    .build(),
            );
        }
    }
    results
}

/// `select_elements` for an element subtree.
fn select_elements_at<'a>(
    root: scraper::ElementRef<'a>,
    css: &str,
) -> Result<Vec<scraper::ElementRef<'a>>, EngineError> {
    let selector = scraper::Selector::parse(css)
        .map_err(|error| EngineError::Parse(format!("bad selector {css:?}: {error:?}")))?;
    Ok(root.select(&selector).collect())
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<html><body>
      <c-wiz jsrenderer="RBsfwb">
        <div role="listitem">
          <a href="/store/apps/details?id=com.example.rust">
            <span class="DdYX5">Rust Playground</span>
            <span class="wMUdtb">Learn Rust anywhere</span>
            <img class="T75of stzEZd" src="https://play-lh.googleusercontent.com/rust.png">
          </a>
        </div>
      </c-wiz>
      <c-wiz jsrenderer="qyd4Kb"><div class="ULeU3b neq64b">
        <div class="Epkrse ">rust book</div>
      </div></c-wiz>
    </body></html>"#;

    fn engine() -> Arc<GooglePlay> {
        GooglePlay::from_config(&EngineConfig::new(
            "google_play",
            "google_play",
            "gpa",
            &["it", "apps"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_app_results_and_suggestions() {
        let response = EngineResponse::new(
            200,
            "https://play.google.com/store/search",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust Playground");
                assert_eq!(
                    result.url,
                    "https://play.google.com/store/apps/details?id=com.example.rust"
                );
                assert_eq!(result.content, "Learn Rust anywhere");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Suggestion(suggestion) => assert_eq!(suggestion, "rust book"),
            _ => panic!("expected a suggestion"),
        }
    }

    #[test]
    fn builds_the_apps_search_url() {
        let query = Query::parse(
            "rust playground",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        let request = engine().build_request(&query, 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://play.google.com/store/search?q=")
        );
        assert!(request.url.contains("rust%20playground"));
        assert!(request.url.ends_with("&c=apps"));
        assert!(
            request
                .cookies
                .iter()
                .any(|(name, value)| name == "CONSENT" && value == "YES+")
        );
    }
}
