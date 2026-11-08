//! Google Custom Search Engine, ported from searx/engines/google_cse.py.
// engine-id: google_cse

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "goc",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

/// The rendered search page of a custom search engine.
///
/// The JSONP element endpoint (`/cse/element/v1`) used to answer with a
/// callback that carried every result. It now answers 403 for every caller we
/// could reach — with a fresh `cse_tok` from `cse.js`, with the cookies of a
/// warmed session, through FlareSolverr and through every exit we have. The
/// search page itself still renders the results server side, so this engine
/// reads that page instead and lets the browser solver wait for it.
const BASE_URL: &str = "https://cse.google.com/cse";
const DEFAULT_CX: &str = "partner-pub-8993703457585266:4862972284";
const DEFAULT_PAGE_SIZE: i64 = 10;

/// Google CSE, read from the rendered result page.
pub struct GoogleCse {
    meta: EngineMeta,
}

impl GoogleCse {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        if !config.opt_str("google_categ").unwrap_or("").is_empty() {
            return Err("google_cse: only the web category is ported".to_string());
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn page_size(&self) -> i64 {
        self.meta
            .opt_i64("page_size")
            .filter(|size| *size > 0)
            .unwrap_or(DEFAULT_PAGE_SIZE)
    }
}

/// The title of a result carries `<b>` around the matched words.
fn clean_title(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Engine for GoogleCse {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let language = if language == "all" {
            "en".to_string()
        } else {
            language
        };
        let cx = self.meta.opt_str("cx").unwrap_or(DEFAULT_CX);
        let mut pairs: Vec<(&str, String)> = vec![
            ("cx", cx.to_string()),
            ("q", query.query_text()),
            ("hl", language),
        ];
        let region = locale_region(&query.locale);
        if region != "all" {
            pairs.push(("gl", region.to_uppercase()));
        }
        if let Some(seconds) = TimeRange::seconds(query.time_range) {
            let end = chrono::Utc::now();
            let start = end - chrono::Duration::seconds(seconds);
            let sort = format!("date:r:{}:{}", start.format("%Y%m%d"), end.format("%Y%m%d"));
            pairs.push(("sort", sort));
        }
        if pageno > 1 {
            let start = (i64::from(pageno - 1) * self.page_size()).to_string();
            pairs.push(("start", start));
        }
        let url = format!("{BASE_URL}?{}", query_string(&pairs));
        Ok(EngineRequest::get(url).header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ))
    }

    /// The page is a shell until the widget script has run, so the solver has
    /// to render it before the results are in the DOM.
    fn needs_render(&self, response: &EngineResponse) -> bool {
        let body = response.text();
        !body.contains("gsc-webResult gsc-result") && !body.contains("did not match any documents")
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        if body.contains("did not match any documents") {
            return Ok(Vec::new());
        }
        if body.len() < 20_000 && body.contains("/sorry/") {
            return Err(EngineError::AccessDenied);
        }
        let html = parse_html(&body);
        let cards = select_elements(&html, "div.gsc-webResult.gsc-result")?;
        let mut results = Vec::new();
        for card in cards {
            let Some(anchor) = child(card, "a.gs-title") else {
                continue;
            };
            let Some(url) = attr(anchor, "href").and_then(|href| absolute_url(&href, BASE_URL))
            else {
                continue;
            };
            let title = clean_title(&text(anchor));
            if title.is_empty() {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title);
            if let Some(content) = text_of(card, "div.gs-snippet") {
                builder = builder.content(content);
            }
            if let Some(thumbnail) = attr_of(card, "img", "src") {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<GoogleCse> {
        let config = EngineConfig::new("google cse", "google_cse", "goc", &["general"]);
        GoogleCse::from_config(&config).expect("google_cse engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_rendered_page_request() {
        let mut config = EngineConfig::new("google cse", "google_cse", "goc", &["general"]);
        config.extra.insert(
            "google_categ".to_string(),
            toml::Value::String("image".into()),
        );
        assert!(GoogleCse::from_config(&config).is_err());
        let mut search = query("rust lang");
        search.locale = "ja-JP".to_string();
        search.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&search, 2).expect("request");
        assert!(request.url.starts_with("https://cse.google.com/cse?"));
        assert!(request.url.contains("q=rust%20lang"));
        assert!(request.url.contains("hl=ja"));
        assert!(request.url.contains("gl=JP"));
        assert!(request.url.contains("start=10"));
        let first = engine().build_request(&query("rust"), 1).expect("request");
        assert!(!first.url.contains("start="));
    }

    #[test]
    fn a_shell_without_results_asks_for_a_rendered_page() {
        let shell = EngineResponse::new(200, BASE_URL, b"<html><body>loading</body></html>".to_vec());
        assert!(engine().needs_render(&shell));
        let rendered = EngineResponse::new(
            200,
            BASE_URL,
            br#"<div class="gsc-webResult gsc-result">x</div>"#.to_vec(),
        );
        assert!(!engine().needs_render(&rendered));
    }

    #[test]
    fn parses_the_rendered_cards() {
        let body = br#"<div class="gsc-webResult gsc-result"><div class="gs-webResult gs-result">
            <a class="gs-title" href="https://www.rust-lang.org/"><b>Rust</b> Programming Language</a>
            <div class="gs-snippet">A language empowering everyone.</div>
            <img src="https://www.google.com/images/rust.png">
        </div></div>
        <div class="gsc-webResult gsc-result"><div class="gs-webResult gs-result">
            <a class="gs-title" href="https://example.com/">Example</a>
        </div></div>"#;
        let response = EngineResponse::new(200, BASE_URL, body.to_vec());
        let items = engine().parse(&response, &query("rust")).expect("items");
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust Programming Language");
                assert!(result.content.starts_with("A language"));
                assert!(result
                    .thumbnail
                    .as_deref()
                    .is_some_and(|src| src.ends_with("rust.png")));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn an_empty_page_is_not_an_error() {
        let body = b"<div>did not match any documents</div>";
        let response = EngineResponse::new(200, BASE_URL, body.to_vec());
        assert!(engine()
            .parse(&response, &query("rust"))
            .expect("items")
            .is_empty());
    }
}
