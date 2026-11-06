//! DuckDuckGo Web engine, ported from `searx/engines/duckduckgo_web.py`.
//!
//! The general DuckDuckGo web search. Upstream scrapes the first-page link and
//! a challenge cookie from duckduckgo.com and then queries the JSON API, but
//! that API (`links.duckduckgo.com/d.js`) now answers every plain HTTP client
//! with a 202 JavaScript-VM challenge. DuckDuckGo also answers a POST to its
//! no-JavaScript endpoint with a 202 "anomaly" interstitial whenever the TLS
//! fingerprint is not a browser's, while the identical query sent as a GET
//! returns the markup results. This engine therefore GETs the endpoint, which
//! shares the caller's quota with the `duckduckgo` engine — the price of
//! having the second one at all.
// engine-id: duckduckgo_web

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "ddgb",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://html.duckduckgo.com/html/";

/// DuckDuckGo general web search.
pub struct DuckDuckGoWeb {
    meta: EngineMeta,
}

impl DuckDuckGoWeb {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for DuckDuckGoWeb {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let text = query.query_text();
        if text.chars().count() >= 500 {
            return Err(EngineError::Message(
                "duckduckgo does not accept queries with 500 or more characters".into(),
            ));
        }
        let region = locale_region(&query.locale);
        let region = if region == "all" {
            "wt-wt".to_string()
        } else {
            region
        };
        // DuckDuckGo fingerprints the transport, not the headers: the same query
        // posted from this stack is answered with a 202 "anomaly" interstitial,
        // while a plain GET returns the HTML results page. Keep it a GET.
        let mut params = vec![
            ("q".to_string(), encode(&text)),
            ("kl".to_string(), encode(&region)),
        ];
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "d",
                TimeRange::Week => "w",
                TimeRange::Month => "m",
                TimeRange::Year => "y",
            };
            params.push(("df".to_string(), value.to_string()));
        }
        params.push((
            "kp".to_string(),
            match query.safesearch {
                0 => "-2",
                1 => "-1",
                _ => "1",
            }
            .to_string(),
        ));
        if pageno > 1 {
            params.push(("s".to_string(), ((pageno - 1) * 10).to_string()));
        }
        let query_string = params
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let request = EngineRequest::get(format!("{BASE_URL}?{query_string}"))
            .header("Referer", "https://duckduckgo.com/")
            .header(
                "Accept-Language",
                &format!(
                    "{0},{0}-{1};q=0.7",
                    locale_language(&query.locale),
                    locale_region(&query.locale).to_uppercase()
                ),
            )
            .header("Origin", "https://duckduckgo.com")
            .header("Sec-Fetch-Dest", "document")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-User", "?1");
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        // Rate limited callers get the "anomaly" interstitial instead of a page.
        if body.contains("anomaly") || body.contains("are you a robot") {
            return Err(EngineError::Captcha);
        }
        let html = parse_html(&body);
        let mut results = Vec::new();
        for element in select_elements(&html, "div.result, div.web-result")? {
            // Sponsored slots are marked result--ad and link through y.js.
            if attr(element, "class")
                .map(|class| class.split_whitespace().any(|token| token == "result--ad"))
                .unwrap_or(false)
            {
                continue;
            }
            let Some(link) = child(element, "a.result__a").or_else(|| child(element, "a[href]"))
            else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            if href.contains("duckduckgo.com/y.js") {
                continue;
            }
            let Some(url) = absolute_url(&href, "https://duckduckgo.com/") else {
                continue;
            };
            let title = text(link);
            if title.is_empty() {
                continue;
            }
            let content = text_of(element, "a.result__snippet")
                .or_else(|| text_of(element, "div.result__snippet"))
                .or_else(|| text_of(element, "div.result__snippet.js-result-snippet"))
                .unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<DuckDuckGoWeb> {
        DuckDuckGoWeb::from_config(&EngineConfig::new(
            "duckduckgo_web",
            "duckduckgo_web",
            "ddgb",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_html_results_page() {
        let html = r#"
        <div id="links">
          <div class="result results_links">
            <h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F">Rust</a></h2>
            <a class="result__snippet">A language empowering everyone.</a>
          </div>
          <div class="result results_links">
            <h2><a class="result__a" href="https://doc.rust-lang.org/">Docs</a></h2>
            <a class="result__snippet">Documentation.</a>
          </div>
        </div>"#;
        let response = EngineResponse::new(200, BASE_URL, html.as_bytes().to_vec());
        let items = engine()
            .parse(
                &response,
                &Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust");
                assert_eq!(result.content, "A language empowering everyone.");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn sponsored_results_are_dropped() {
        let html = r#"
        <div id="links">
          <div class="result result--ad">
            <h2><a class="result__a" href="https://duckduckgo.com/y.js?ad_domain=udemy.com">Sponsored</a></h2>
            <a class="result__snippet">Buy the course.</a>
          </div>
          <div class="result results_links">
            <h2><a class="result__a" href="https://rust-lang.org/">Rust</a></h2>
            <a class="result__snippet">A language empowering everyone.</a>
          </div>
        </div>"#;
        let response = EngineResponse::new(200, BASE_URL, html.as_bytes().to_vec());
        let items = engine()
            .parse(
                &response,
                &Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => assert_eq!(result.url, "https://rust-lang.org/"),
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn captcha_pages_are_detected() {
        let body = "<html><body>Unfortunately, bots use DuckDuckGo too. anomaly</body></html>";
        let response = EngineResponse::new(202, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine().parse(&response, &query),
            Err(EngineError::Captcha)
        ));
    }

    #[test]
    fn build_request_gets_the_html_endpoint() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(request.method, crate::engine::Method::Get);
        assert!(request.url.starts_with(&format!("{BASE_URL}?q=rust")));
        assert!(request.url.contains("kl=wt-wt"));
        assert!(request.form.is_empty());
    }

    #[test]
    fn time_range_and_safesearch_are_forwarded() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        query.safesearch = 2;
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.contains("df=w"));
        assert!(request.url.contains("kp=1"));
        assert!(request.url.contains("s=10"));
    }
}
