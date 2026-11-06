//! Ollama engine, ported from searx/engines/ollama.py.
//!
//! The model library search is scraped from the HTML page, like upstream.
// engine-id: ollama

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "repos"],
    shortcut: "ollama",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://ollama.com";
/// The library list is a plain role=list wrapper; the old x-test-* hooks that
/// upstream relied on are gone from the markup.
const RESULTS: &str = "ul[role=\"list\"] > li";
const TITLE: &str = "h2 > span";
const CONTENT: &str = "p.line-clamp-2";
const PUBLISHED: &str = "span[title^=\"Updated \"]";

pub struct Ollama {
    meta: EngineMeta,
}

impl Ollama {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for Ollama {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![("q", query.query_text())];
        Ok(EngineRequest::get(format!(
            "{}/search?{}",
            self.base_url(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base_url();
        let mut results = Vec::new();
        for element in select_elements(&html, RESULTS)? {
            let Some(href) = attr_of(element, "a", "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            let title = text_of(element, TITLE).unwrap_or_default();
            if title.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(text_of(element, CONTENT).unwrap_or_default());
            if let Some(published) = attr_of(element, PUBLISHED, "title").and_then(|value| date(&value))
            {
                builder = builder.published_date(published);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The list shows an absolute stamp in the tooltip ("Updated October 31, 2026")
/// and a relative one as the visible text ("4d").
fn date(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value.strip_prefix("Updated ").unwrap_or(value).trim();
    if let Ok(parsed) = chrono::NaiveDate::parse_from_str(value, "%B %d, %Y") {
        return Some(parsed.format("%Y-%m-%dT00:00:00").to_string());
    }
    let head = value
        .rsplit_once(' ')
        .map(|(head, _zone)| head)
        .unwrap_or(value);
    for candidate in [value, head] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(candidate, "%b %d, %Y %I:%M %p") {
            return Some(parsed.format("%Y-%m-%dT%H:%M:%S").to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Ollama> {
        Ollama::from_config(&EngineConfig::new("ollama", "ollama", "ollama", &["it"])).unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_model_list() {
        let html = r##"<ul role="list" class="overflow-hidden rounded-3xl border-[0.5px] border-line">
          <li class="border-t-[0.5px] border-line first:border-t-0">
            <a href="/meta/llama3.2" class="group grid grid-cols-1">
              <div class="flex min-w-0 items-baseline gap-2.5">
                <h2 class="min-w-0 truncate text-xl font-medium" title="llama3.2"><span>meta/llama3.2</span></h2>
                <span class="flex-none text-[15px] tabular-nums text-muted" title="Updated October 31, 2026">4d</span>
              </div>
              <p class="mt-1 line-clamp-2 text-[15px] leading-6 text-black sm:col-span-2">Meta's Llama 3.2 model</p>
            </a>
          </li>
          <li class="border-t-[0.5px] border-line"><a href="/meta/broken"></a></li>
          <li><span>not a result</span></li>
        </ul>"##;
        let response = EngineResponse::new(200, BASE_URL, html.as_bytes().to_vec());
        let items = engine().parse(&response, &query("llama")).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://ollama.com/meta/llama3.2");
        assert_eq!(result.title, "meta/llama3.2");
        assert_eq!(result.content, "Meta's Llama 3.2 model");
        assert_eq!(
            result.published_date.as_deref(),
            Some("2026-10-31T00:00:00")
        );
    }

    #[test]
    fn builds_the_search_request() {
        let request = engine().build_request(&query("llama 3.2"), 1).unwrap();
        assert_eq!(request.url, "https://ollama.com/search?q=llama%203.2");
        assert!(matches!(request.method, crate::engine::Method::Get));
        let empty = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine().parse(&empty, &query("x")).unwrap().is_empty());
    }
}
