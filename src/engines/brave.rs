//! Brave Search engine, ported from `searx/engines/brave.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "brave",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 5.0,
};

const BASE_URL: &str = "https://search.brave.com/search";

/// Brave Search.
pub struct Brave {
    meta: EngineMeta,
}

impl Brave {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Brave {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let region = locale_region(&query.locale);
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("source", "web".to_string()),
            ("spellcheck", "1".to_string()),
        ];
        if pageno > 1 {
            pairs.push(("offset", (pageno - 1).to_string()));
        }
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "pd",
                TimeRange::Week => "pw",
                TimeRange::Month => "pm",
                TimeRange::Year => "py",
            };
            pairs.push(("tf", value.to_string()));
        }
        let safesearch = match query.safesearch {
            0 => "off",
            1 => "moderate",
            _ => "strict",
        };
        let country = if region == "all" {
            "us".to_string()
        } else {
            region
        };
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .cookie("safesearch", safesearch)
                .cookie("useLocation", "0")
                .cookie("summarizer", "0")
                .cookie("country", &country)
                .cookie("ui_lang", &locale_language(&query.locale))
                .header("Accept-Encoding", "gzip, deflate")
                .header("Referer", "https://search.brave.com/"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        // Brave's own result page carries localisation strings such as
        // "Switch to traditional CAPTCHA", so the word alone is not a block
        // page; only an empty result set makes it one.
        let looks_blocked = body.contains("captcha") && body.contains("Brave");
        let html = parse_html(&body);
        let mut results = Vec::new();
        for element in select_elements(&html, "div.snippet, div.snippet-cta")? {
            let Some(link) = child(element, "a.h").or_else(|| child(element, "a[href]")) else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, "https://search.brave.com/") else {
                continue;
            };
            let title = text_of(element, "div.title")
                .or_else(|| text_of(element, "span.title"))
                .unwrap_or_else(|| text(link));
            if title.is_empty() {
                continue;
            }
            let content = text_of(element, "div.snippet-description")
                .or_else(|| text_of(element, "div.snippet-content"))
                .unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        if results.is_empty() && looks_blocked {
            return Err(EngineError::Captcha);
        }
        Ok(results)
    }
}
