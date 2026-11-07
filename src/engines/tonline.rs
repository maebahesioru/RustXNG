//! T-Online engine, ported from `searx/engines/tonline.py` (web search).
//!
//! Upstream picks the endpoint from `tonline_categ` (`web`, `images`,
//! `videos`, `news`); this port implements the main `web` variant and reads the
//! category from the `search_type` config key.
// engine-id: tonline

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "ton",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://suche.t-online.de";

/// T-Online search (results are provided by Google).
pub struct TOnline {
    meta: EngineMeta,
    categ: String,
}

impl TOnline {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config.opt_str("search_type").unwrap_or("web").to_string();
        if !matches!(categ.as_str(), "web" | "images" | "videos" | "news") {
            return Err(format!("tonline: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for TOnline {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        // "mandant", "dia" and "ptl" are not needed but reduce captchas.
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("mandant", "toi".to_string()),
            ("dia", "suche".to_string()),
            ("ptl", "std".to_string()),
        ];
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "d",
                TimeRange::Week => "w",
                TimeRange::Month => "m",
                TimeRange::Year => "y",
            };
            pairs.push(("age", value.to_string()));
        }
        let channel = match self.categ.as_str() {
            "images" => Some("flickr"),
            "videos" => Some("yt"),
            _ => None,
        };
        // The result provider has to be repeated during pagination.
        let mut pairs: Vec<(String, String)> = pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect();
        match channel {
            Some(channel) if pageno > 1 => {
                pairs.push(("ch".to_string(), channel.to_string()));
                pairs.push((format!("{channel}_page"), pageno.to_string()));
            }
            _ => pairs.push(("page".to_string(), pageno.max(1).to_string())),
        }
        let query = pairs
            .iter()
            .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/{}?{query}",
            self.categ
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        if self.categ != "web" {
            return Ok(Vec::new());
        }
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        let Ok(items) = select_elements(&html, "div#google_re > div.doc") else {
            return Ok(results);
        };
        for item in items {
            let title = text_of(item, "span[class~='tMMReshl']").unwrap_or_default();
            let Some(href) = attr_of(item, "a", "href") else {
                continue;
            };
            let url = absolute_url(&href, BASE_URL).unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(text_of(item, "div[class~='tMMRest']").unwrap_or_default())
                    .build(),
            );
        }
        if let Ok(suggestions) = select_elements(&html, "div[class^='rsbl'] > a") {
            for suggestion in suggestions {
                let value = text(suggestion);
                if !value.is_empty() {
                    results.push(EngineItem::Suggestion(value));
                }
            }
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<html><body>
      <div id="google_re">
        <div class="doc">
          <a href="https://www.rust-lang.org/">https://www.rust-lang.org/</a>
          <span class="tMMReshl">Rust Programmiersprache</span>
          <div class="tMMRest">Schnell und sicher.</div>
        </div>
      </div>
      <div class="rsbl"><a>rust buch</a></div>
    </body></html>"#;

    fn engine() -> Arc<TOnline> {
        TOnline::from_config(&EngineConfig::new(
            "tonline",
            "tonline",
            "ton",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_web_results_and_suggestions() {
        let response = EngineResponse::new(
            200,
            "https://suche.t-online.de/web",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust Programmiersprache");
                assert_eq!(result.url, "https://www.rust-lang.org/");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Suggestion(suggestion) => assert_eq!(suggestion, "rust buch"),
            _ => panic!("expected a suggestion"),
        }
    }

    #[test]
    fn builds_a_paged_request() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert!(request.url.starts_with("https://suche.t-online.de/web?"));
        assert!(request.url.contains("page=3"));
        assert!(request.url.contains("mandant=toi"));
    }
}
