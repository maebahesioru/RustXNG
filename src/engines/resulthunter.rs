//! Resulthunter engine, ported from searx/engines/resulthunter.py.
//!
//! Upstream fetches the search token from the homepage; here it comes from
//! the search_token option (window.__rhSearchToken).
// engine-id: resulthunter

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "reh",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://resulthunter.com";

/// Resulthunter, a Brave backed search front end.
pub struct Resulthunter {
    meta: EngineMeta,
}

impl Resulthunter {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let category = meta.opt_str("resulthunter_categ").unwrap_or("web");
        if !matches!(category, "web" | "images") {
            return Err(format!("resulthunter: invalid category {category:?}"));
        }
        Ok(Arc::new(Self { meta }))
    }

    fn category(&self) -> &str {
        self.meta.opt_str("resulthunter_categ").unwrap_or("web")
    }

    fn base_url(&self) -> String {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for Resulthunter {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let token = self.meta.opt_str("search_token").ok_or_else(|| {
            EngineError::Message("resulthunter needs the search_token option".into())
        })?;
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("search_type", self.category().to_string()),
            ("offset", pageno.saturating_sub(1).to_string()),
            ("search_source", "header".to_string()),
            ("st", token.to_string()),
        ];
        let language = locale_language(&query.locale);
        if language != "all" {
            pairs.push(("search_lang", language));
        }
        let region = locale_region(&query.locale);
        if region != "all" {
            pairs.push(("country", region));
        }
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "pd",
                TimeRange::Week => "pw",
                TimeRange::Month => "pm",
                TimeRange::Year => "py",
            };
            pairs.push(("freshness", value.to_string()));
        }
        let safesearch = match query.safesearch {
            0 => "off",
            1 => "moderate",
            _ => "strict",
        };
        let url = format!("{}/search?{}", self.base_url(), query_string(&pairs));
        let request = EngineRequest::get(url)
            .cookie("safesearch", safesearch)
            .header("Accept", "text/html");
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base_url();
        // A refusal page has no result container at all. Without this check the
        // engine reported "no hits" and the circuit breaker never saw the
        // block, so every later search paid the same refused request.
        if !response.text().contains("organic-results-container") {
            let body = response.text().to_ascii_lowercase();
            if body.contains("captcha")
                || body.contains("access denied")
                || body.contains("forbidden")
                || body.contains("are you a robot")
            {
                return Err(EngineError::AccessDenied);
            }
        }
        let mut results = Vec::new();
        if self.category() == "images" {
            let cards = "div[class*='organic-results-container'] a[class*='group']";
            for card in select_elements(&html, cards)? {
                let Some(href) = attr(card, "href") else {
                    continue;
                };
                let Some(url) = absolute_url(&href, &base) else {
                    continue;
                };
                let title = attr_of(card, "img", "alt").unwrap_or_default();
                if title.is_empty() || !url.starts_with("http") {
                    continue;
                }
                let mut builder = ResultBuilder::new(url, title).template(Template::Images);
                let source = attr_of(card, "img", "src");
                if let Some(source) = source.and_then(|value| absolute_url(&value, &base)) {
                    builder = builder.image(source);
                }
                results.push(builder.build());
            }
            return Ok(results);
        }
        let rows = "div[class*='organic-results-container'] > div > div[class*='group']";
        for row in select_elements(&html, rows)? {
            let Some(href) = child(row, "a").and_then(|link| attr(link, "href")) else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            let title = text_of(row, "a h3").unwrap_or_default();
            if title.is_empty() || !url.starts_with("http") {
                continue;
            }
            let content = text_of(row, "p").unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEB_FIXTURE: &str = r##"<html><body><div class="organic-results-container"><div>
  <div class="group"><a href="https://example.test/one"><h3>First hit</h3></a><p>First description</p></div>
  <div class="group"><a href="/two"><h3>Second hit</h3></a></div>
  <div class="group"><a href="https://example.test/none"><h3></h3></a></div>
</div></div></body></html>"##;

    const IMAGE_FIXTURE: &str = r##"<html><body><div class="organic-results-container"><a class="group" href="/image/1"><img src="/thumb.jpg" alt="First image"></a></div></body></html>"##;

    fn build(category: &str, token: Option<&str>) -> Result<Arc<Resulthunter>, String> {
        let mut config = EngineConfig::new("resulthunter", "resulthunter", "reh", &["general"]);
        config
            .extra
            .insert("resulthunter_categ".into(), category.into());
        if let Some(token) = token {
            config.extra.insert("search_token".into(), token.into());
        }
        Resulthunter::from_config(&config)
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    fn parse(category: &str, body: &[u8]) -> Vec<EngineItem> {
        let engine = build(category, Some("abc")).unwrap();
        let response = EngineResponse::new(200, BASE_URL, body.to_vec());
        engine.parse(&response, &query()).unwrap()
    }

    #[test]
    fn a_refusal_page_is_a_block_not_an_empty_result() {
        let engine = build("web", Some("abc")).unwrap();
        let body = b"<html><body><h1>Access denied</h1><p>Please solve the CAPTCHA.</p></body></html>";
        let response = EngineResponse::new(200, BASE_URL, body.to_vec());
        assert!(matches!(
            engine.parse(&response, &query()),
            Err(EngineError::AccessDenied)
        ));
        // A page that simply has no hits stays an empty success.
        let empty = EngineResponse::new(200, BASE_URL, b"<html><body>nothing</body></html>".to_vec());
        assert!(engine.parse(&empty, &query()).unwrap().is_empty());
    }

    fn result_at(items: &[EngineItem], index: usize) -> &crate::results::SearchResult {
        match &items[index] {
            EngineItem::Result(found) => found,
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn parses_the_web_rows() {
        assert!(build("videos", Some("abc")).is_err());
        let items = parse("web", WEB_FIXTURE.as_bytes());
        assert_eq!(items.len(), 2);
        let first = result_at(&items, 0);
        assert_eq!(first.title, "First hit");
        assert_eq!(first.url, "https://example.test/one");
        assert_eq!(first.content, "First description");
        assert_eq!(result_at(&items, 1).url, "https://resulthunter.com/two");
        assert!(parse("web", b"<html></html>").is_empty());
    }

    #[test]
    fn parses_the_image_cards_and_builds_the_request() {
        let items = parse("images", IMAGE_FIXTURE.as_bytes());
        assert_eq!(items.len(), 1);
        let first = result_at(&items, 0);
        assert_eq!(first.url, "https://resulthunter.com/image/1");
        assert_eq!(first.template, Template::Images);
        let engine = build("web", Some("abc")).unwrap();
        let mut request_query = query();
        request_query.time_range = Some(TimeRange::Week);
        let request = engine.build_request(&request_query, 2).unwrap();
        assert!(request.url.contains("st=abc&"));
        assert!(request.url.contains("offset=1&"));
        assert!(request.url.contains("freshness=pw"));
        let without = build("web", None).unwrap();
        assert!(without.build_request(&query(), 1).is_err());
    }
}
