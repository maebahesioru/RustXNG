//! S1search engine, ported from searx/engines/s1search.py.
//!
//! The HTML variant served by the s1search.co mirrors. Upstream keys its
//! pagination on the verification token of the previous page; that token cache
//! is not ported, so only the first page can be requested.

// engine-id: s1search

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "s1",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// System1's HTML search front end.
pub struct S1Search {
    meta: EngineMeta,
}

impl S1Search {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        match meta.base_url.as_deref().map(str::trim) {
            Some(url) if !url.is_empty() => Ok(Arc::new(Self { meta })),
            _ => Err("s1search requires a base_url".to_string()),
        }
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for S1Search {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        if pageno > 1 {
            return Err(EngineError::Message(
                "s1search needs the verification token of the previous page".into(),
            ));
        }
        Ok(EngineRequest::get(format!(
            "{}/serp?{}",
            self.base_url(),
            query_string(&[("q", query.query_text()), ("page", pageno.to_string()),])
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base_url();
        let mut items = Vec::new();
        for block in select_elements(
            &html,
            "div.aylf-yahoo-bottom > div, div.aylf-yahoo-sidebar > div",
        )? {
            let suggestion = text(block);
            if !suggestion.is_empty() {
                items.push(EngineItem::Suggestion(suggestion));
            }
        }
        for result in select_elements(
            &html,
            "div[class*='web-yahoo'] > div[class*='__result'], div[class*='web-google'] > div[class*='__result']",
        )? {
            let Some(link) = child(result, "a[class*='title']") else {
                continue;
            };
            let (Some(href), title) = (attr(link, "href"), text(link)) else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            if !is_http_url(&url) {
                continue;
            }
            let content = child(result, "span[class*='description'], span:not([class])")
                .map(text)
                .unwrap_or_default();
            items.push(ResultBuilder::new(url, title).content(content).build());
        }
        Ok(items)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://search.gmx.net";

    const FIXTURE: &str = r##"<html><body>
<div class="aylf-yahoo-bottom"><div>related search one</div></div>
<div class="web-google">
  <div class="web-google__result">
    <a class="title" href="/click/one">First result</a>
    <span class="description">Description of the first</span>
  </div>
  <div class="web-google__result">
    <a class="title" href="https://example.test/two">Second result</a>
    <span class="description">Description of the second</span>
  </div>
  <div class="web-google__result"><a class="title" href="/click/three"></a></div>
</div>
</body></html>"##;

    fn config() -> EngineConfig {
        let mut config = EngineConfig::new("s1search", "s1search", "s1", &["general"]);
        config.base_url = Some(BASE.to_string());
        config
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn requires_a_base_url() {
        let config = EngineConfig::new("s1search", "s1search", "s1", &["general"]);
        assert!(S1Search::from_config(&config).is_err());
        assert!(S1Search::from_config(&self::config()).is_ok());
    }

    #[test]
    fn parses_suggestions_and_results() {
        let engine = S1Search::from_config(&config()).unwrap();
        let response = EngineResponse::new(200, BASE, FIXTURE.as_bytes().to_vec());
        let items = engine.parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 3);
        match &items[0] {
            EngineItem::Suggestion(suggestion) => assert_eq!(suggestion, "related search one"),
            _ => panic!("expected a suggestion"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "First result");
                assert_eq!(result.url, "https://search.gmx.net/click/one");
                assert_eq!(result.content, "Description of the first");
            }
            _ => panic!("expected a result"),
        }
        match &items[2] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.test/two");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_first_page_only() {
        let engine = S1Search::from_config(&config()).unwrap();
        let request = engine.build_request(&query(), 1).unwrap();
        assert_eq!(request.url, "https://search.gmx.net/serp?q=rust&page=1");
        assert!(engine.build_request(&query(), 2).is_err());
    }

    #[test]
    fn missing_container_yields_no_results() {
        let engine = S1Search::from_config(&config()).unwrap();
        let response = EngineResponse::new(200, BASE, b"<html></html>".to_vec());
        assert!(engine.parse(&response, &query()).unwrap().is_empty());
    }
}
