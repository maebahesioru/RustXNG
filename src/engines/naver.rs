//! Naver engine, ported from `searx/engines/naver.py` (web search).
//!
//! Upstream supports the categories `general`, `images`, `news` and `videos`
//! through the `naver_category` setting; this port implements the main
//! `general` variant and reads the category from the `search_type` config key.
// engine-id: naver

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "nv",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://search.naver.com";
const TITLE: &str = "span[class~='sds-comps-text-type-headline1']";

/// Naver, the Korean search engine.
pub struct Naver {
    meta: EngineMeta,
    categ: String,
}

impl Naver {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config
            .opt_str("search_type")
            .unwrap_or("general")
            .to_string();
        if !matches!(categ.as_str(), "general" | "images" | "news" | "videos") {
            return Err(format!("naver: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }

    /// Naver cannot set the number of results per page; the step per category is
    /// the upstream `naver_category_dict` value.
    fn page_step(&self) -> i64 {
        match self.categ.as_str() {
            "images" => 50,
            "news" => 10,
            "videos" => 48,
            _ => 15,
        }
    }

    fn where_value(&self) -> &'static str {
        match self.categ.as_str() {
            "images" => "image",
            "news" => "news",
            "videos" => "video",
            _ => "web",
        }
    }
}

impl Engine for Naver {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let start = ((pageno.max(1) - 1) as i64) * self.page_step() + 1;
        let mut pairs: Vec<(&str, String)> = vec![
            ("query", query.query_text()),
            ("start", start.to_string()),
            ("where", self.where_value().to_string()),
        ];
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "1d",
                TimeRange::Week => "1w",
                TimeRange::Month => "1m",
                TimeRange::Year => "1y",
            };
            pairs.push(("nso", format!("p:{value}")));
        }
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/search.naver?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        if self.categ != "general" {
            return Ok(results);
        }
        let Ok(items) = select_elements(&html, "div[class~='fds-web-normal-doc-root']") else {
            return Ok(results);
        };
        for item in items {
            let title = text_of(item, TITLE).unwrap_or_default();
            // A block can start with the site profile link (for example
            // `https://github.com/`); the document link is the anchor around the title.
            let link = children(item, "a[href]")
                .into_iter()
                .find(|link| child(*link, TITLE).is_some())
                .or_else(|| child(item, "a[href]"));
            let Some(href) = link.and_then(|link| attr(link, "href")) else {
                continue;
            };
            if !href.starts_with("http") || href.contains("keep.naver.com") {
                continue;
            }
            if title.is_empty() {
                continue;
            }
            let content =
                text_of(item, "span[class~='sds-comps-text-type-body1']").unwrap_or_default();
            let thumbnail = attr_of(
                item,
                "div[class~='sds-comps-image']:not(.sds-comps-image-circle) img",
                "src",
            );
            let mut builder = ResultBuilder::new(href, title).content(content);
            if let Some(thumbnail) = thumbnail {
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

    const FIXTURE: &str = r#"<html><body>
      <div class="fds-web-normal-doc-root">
        <div class="sds-comps-image"><img src="https://img.naver.net/a.png"></div>
        <span class="sds-comps-text sds-comps-text-type-headline1">Rust 언어</span>
        <a href="https://www.rust-lang.org/">https://www.rust-lang.org/</a>
        <span class="sds-comps-text-type-body1">시스템 프로그래밍 언어</span>
      </div>
    </body></html>"#;

    fn engine() -> Arc<Naver> {
        Naver::from_config(&EngineConfig::new("naver", "naver", "nv", &["general"])).unwrap()
    }

    #[test]
    fn parses_general_results() {
        let response = EngineResponse::new(
            200,
            "https://search.naver.com/search.naver",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust 언어");
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.content, "시스템 프로그래밍 언어");
            }
            _ => panic!("expected a result"),
        }
    }

    // Shape of a live block where the site profile link comes before the document link.
    const PROFILE_FIRST: &str = r#"<html><body>
      <div class="fds-web-normal-doc-root">
        <a class="fds-anchor" href="https://github.com/"><span>GitHub</span></a>
        <a class="fds-anchor" href="https://github.com/"><span>github.com</span></a>
        <a class="fds-anchor" href="https://github.com/CapSoftware/Cap/issues/345">
          <span class="sds-comps-text sds-comps-text-type-headline1">error[E0502] · Issue #345</span>
        </a>
        <a href="https://keep.naver.com/">keep</a>
        <span class="sds-comps-text-type-body1">The source code is not building</span>
      </div>
    </body></html>"#;

    #[test]
    fn uses_the_title_link_instead_of_the_site_profile_link() {
        let response = EngineResponse::new(
            200,
            "https://search.naver.com/search.naver",
            PROFILE_FIRST.as_bytes().to_vec(),
        );
        let query = Query::parse("E0502", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://github.com/CapSoftware/Cap/issues/345");
                assert_eq!(result.title, "error[E0502] · Issue #345");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_a_paged_request_with_the_category_step() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert!(
            request
                .url
                .starts_with("https://search.naver.com/search.naver?")
        );
        assert!(request.url.contains("start=31"));
        assert!(request.url.contains("where=web"));
    }
}
