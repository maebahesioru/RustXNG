//! SearchRockit engine, ported from searx/engines/searchrockit.py.
//!
//! The web and news lists share their markup, images are rendered as cards.

// engine-id: searchrockit

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "sr",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://searchrockit.com";

/// SearchRockit, a Google backed search front end.
pub struct Searchrockit {
    meta: EngineMeta,
}

impl Searchrockit {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let category = meta.opt_str("searchrockit_categ").unwrap_or("web");
        if !matches!(category, "web" | "images" | "news") {
            return Err(format!("invalid search category: {category}"));
        }
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for Searchrockit {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let base = base.trim_end_matches('/');
        Ok(EngineRequest::get(format!(
            "{}/results/{}?{}",
            base,
            self.meta.opt_str("searchrockit_categ").unwrap_or("web"),
            query_string(&[("q", query.query_text()), ("p", pageno.to_string())])
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let base = base.trim_end_matches('/');
        let mut results = Vec::new();
        if self.meta.opt_str("searchrockit_categ") == Some("images") {
            for card in select_elements(&html, "div[class*='image-grid'] a[class*='image-card']")? {
                let title = text_of(card, "div[class*='image-title']");
                let (Some(href), Some(title)) = (attr(card, "href"), title) else {
                    continue;
                };
                let Some(url) = http_url(&href, base) else {
                    continue;
                };
                let mut builder = ResultBuilder::new(url, title).template(Template::Images);
                if let Some(src) = attr_of(card, "img", "src") {
                    if let Some(src) = absolute_url(&src, base) {
                        builder = builder.thumbnail(src);
                    }
                }
                if let Some(image) = attr(card, "data-full-url") {
                    if let Some(image) = absolute_url(&image, base) {
                        builder = builder.image(image);
                    }
                }
                results.push(builder.build());
            }
            return Ok(results);
        }
        for row in select_elements(
            &html,
            "div[class*='results-list'] div[class*='result-item']",
        )? {
            let Some(link) = child(row, "a[class*='result-item--title']") else {
                continue;
            };
            let (Some(href), title) = (attr(link, "href"), text(link)) else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            let Some(url) = http_url(&href, base) else {
                continue;
            };
            let content = child(row, "a[class*='result-item--desc']")
                .map(text)
                .unwrap_or_default();
            let published = text_of(row, "span[class*='result-item--publishedAt']");
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .published_date(published.unwrap_or_default());
            if let Some(src) = attr_of(row, "a[class*='result-item--thumb'] img", "src") {
                if let Some(src) = absolute_url(&src, base) {
                    builder = builder.thumbnail(src);
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn http_url(raw: &str, base: &str) -> Option<String> {
    absolute_url(raw, base).filter(|url| url.starts_with("http"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEB_FIXTURE: &str = r##"<html><body><div class="results-list">
  <div class="result-item">
    <a class="result-item--title" href="https://example.test/one">First hit</a>
    <a class="result-item--desc">First description</a>
    <a class="result-item--thumb"><img src="https://example.test/one.png"></a>
    <span class="result-item--publishedAt">2024-10-24</span>
  </div>
  <div class="result-item"><a class="result-item--title" href="/relative/two">Second hit</a>
    <a class="result-item--desc">Second description</a></div>
  <div class="result-item"><a class="result-item--title" href="/empty"></a></div></div></body></html>"##;

    const IMAGE_FIXTURE: &str = r##"<html><body><div class="image-grid">
  <a class="image-card" href="/image/1" data-full-url="https://cdn.example.test/full.jpg">
  <img src="https://cdn.example.test/thumb.jpg"><div class="image-title">First image</div></a></div></body></html>"##;

    fn config(category: &str) -> EngineConfig {
        let mut config = EngineConfig::new("searchrockit", "searchrockit", "sr", &["general"]);
        config.extra.insert(
            "searchrockit_categ".into(),
            toml::Value::String(category.to_string()),
        );
        config
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_web_rows() {
        assert!(Searchrockit::from_config(&config("videos")).is_err());
        let engine = Searchrockit::from_config(&config("web")).unwrap();
        let response = EngineResponse::new(200, BASE_URL, WEB_FIXTURE.as_bytes().to_vec());
        let items = engine.parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "First hit");
                assert_eq!(result.url, "https://example.test/one");
                assert_eq!(result.content, "First description");
                assert_eq!(result.published_date.as_deref(), Some("2024-10-24"));
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://example.test/one.png")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://searchrockit.com/relative/two");
            }
            _ => panic!("expected a result"),
        }
        let empty = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine.parse(&empty, &query()).unwrap().is_empty());
    }

    #[test]
    fn parses_the_images_and_builds_the_url() {
        let engine = Searchrockit::from_config(&config("images")).unwrap();
        let response = EngineResponse::new(200, BASE_URL, IMAGE_FIXTURE.as_bytes().to_vec());
        let items = engine.parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "First image");
                assert_eq!(result.url, "https://searchrockit.com/image/1");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://cdn.example.test/full.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
        let request = engine.build_request(&query(), 2).unwrap();
        assert_eq!(
            request.url,
            "https://searchrockit.com/results/images?q=rust&p=2"
        );
    }
}
