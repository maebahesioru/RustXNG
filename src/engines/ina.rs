//! INA engine, ported from searx/engines/ina.py.
//!
//! The AJAX endpoint of the French audiovisual archive answers with an HTML
//! fragment, so the results are extracted with CSS selectors.
// engine-id: ina

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "in",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 6.0,
};

const BASE_URL: &str = "https://www.ina.fr";
/// Upstream page_size (one page per request).
const PAGE_SIZE: u32 = 12;

/// INA, the French national audiovisual institute.
pub struct Ina {
    meta: EngineMeta,
}

impl Ina {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Ina {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        // The AJAX endpoint is zero-based: offset=0 is the first page. The
        // previous implementation started at PAGE_SIZE and therefore dropped
        // the twelve best hits of every search.
        let offset = pageno.saturating_sub(1) * PAGE_SIZE;
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("espace", "1".to_string()),
            ("sort", "pertinence".to_string()),
            ("order", "desc".to_string()),
            ("offset", offset.to_string()),
            ("modified", "size".to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/ajax/recherche?{}",
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
        for row in select_elements(&html, "div#searchHits > div")? {
            let Some(href) = child(row, "a").and_then(|link| attr(link, "href")) else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            if !is_http_url(&url) {
                continue;
            }
            let title = text_of(row, "div[class*='title-bloc-small']").unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let published = text_of(row, "div[class*='dateAgenda']").unwrap_or_default();
            let subtitle = text_of(row, "div[class*='sous-titre-fonction']").unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title)
                .content(format!("{published} {subtitle}"))
                .template(Template::Videos);
            if let Some(source) = attr_of(row, "img", "data-src") {
                if let Some(source) = absolute_url(&source, &base) {
                    builder = builder.thumbnail(source);
                }
            }
            results.push(builder.build());
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

    const FIXTURE: &str = r##"<html><body><div id="searchHits">
  <div><a href="/videos/une">lien</a>
    <div class="title-bloc-small">Titre un</div>
    <div class="dateAgenda">12/03/2024</div>
    <div class="sous-titre-fonction">Sous titre</div>
    <img data-src="https://www.ina.fr/img/une.jpg">
  </div>
  <div><a href="/videos/deux">lien</a>
    <div class="title-bloc-small">Titre deux</div>
  </div>
  <div><div class="title-bloc-small">Sans lien</div></div>
</div></body></html>"##;

    fn engine() -> Arc<Ina> {
        Ina::from_config(&EngineConfig::new("ina", "ina", "in", &["videos"])).unwrap()
    }

    fn query() -> Query {
        Query::parse(
            "documentaire",
            &crate::engine::EngineRegistry::default(),
            &[],
        )
    }

    #[test]
    fn parses_the_result_rows() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.ina.fr/videos/une");
                assert_eq!(result.title, "Titre un");
                assert_eq!(result.content, "12/03/2024 Sous titre");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://www.ina.fr/img/une.jpg")
                );
                assert_eq!(result.template, Template::Videos);
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.ina.fr/videos/deux");
                assert!(result.thumbnail.is_none());
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn a_page_without_the_container_is_empty() {
        let response = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }

    #[test]
    fn builds_the_ajax_url() {
        let request = engine().build_request(&query(), 2).unwrap();
        assert_eq!(
            request.url,
            "https://www.ina.fr/ajax/recherche?q=documentaire&espace=1&sort=pertinence&order=desc&offset=12&modified=size"
        );
    }

    #[test]
    fn the_first_page_starts_at_offset_zero() {
        let request = engine().build_request(&query(), 1).unwrap();
        assert!(
            request.url.ends_with("&offset=0&modified=size"),
            "{}",
            request.url
        );
    }
}
