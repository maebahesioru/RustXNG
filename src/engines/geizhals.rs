//! Geizhals engine, ported from searx/engines/geizhals.py.
//!
//! A sort:<name> token in the query switches the order, like upstream.
// engine-id: geizhals

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["shopping"],
    shortcut: "geiz",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://geizhals.de";

/// Geizhals price comparison.
pub struct Geizhals {
    meta: EngineMeta,
}

impl Geizhals {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Geizhals {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let (text, sort) = split_sort(&query.query_text());
        let sort = match sort.as_deref() {
            Some("price") | Some("asc") => Some("p"),
            Some("desc") => Some("-p"),
            _ => None,
        };
        let mut pairs: Vec<(&str, String)> = vec![
            ("fs", text),
            ("pg", pageno.max(1).to_string()),
            ("toggle_all", "1".to_string()),
        ];
        if let Some(sort) = sort {
            pairs.push(("sort", sort.to_string()));
        }
        let url = format!("{}/?{}", self.base_url(), query_string(&pairs));
        Ok(EngineRequest::get(url))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base_url();
        let mut results = Vec::new();
        for element in select_elements(&html, "article.galleryview__item, article.listview__item")?
        {
            // The listing renders the gallery layout by default and falls back
            // to the list layout for some categories, so both are read here.
            let title = text_of(element, "h3.galleryview__name")
                .or_else(|| text_of(element, "h3.listview__name"))
                .unwrap_or_default();
            let href = attr_of(element, "a.galleryview__name-link", "href")
                .or_else(|| attr_of(element, "a.listview__name-link", "href"))
                .unwrap_or_default();
            let url = if is_http_url(&href) {
                href
            } else {
                format!("{}/{}", base, href.trim_start_matches('/'))
            };
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut specs = Vec::new();
            for spec in children(element, "div.specs-grid__item") {
                let term = text_of(spec, "dt").unwrap_or_default();
                let value = text_of(spec, "dd").unwrap_or_default();
                if !(term.is_empty() && value.is_empty()) {
                    specs.push(format!("{term}: {value}"));
                }
            }
            let metadata: Vec<String> = [
                "span.stars-rating-label",
                "div.stars-rating-label",
                "div.listview__offercount",
                "div.galleryview__offercount",
            ]
            .iter()
            .filter_map(|css| text_of(element, css))
            .collect();
            let mut builder = ResultBuilder::new(url, title)
                .content(specs.join(" | "))
                .metadata("metadata", metadata.join(", "));
            let thumbnail = attr_of(element, "img.galleryview__image", "src")
                .or_else(|| attr_of(element, "img.listview__image", "src"))
                .unwrap_or_default();
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            let price = text_of(element, "a.galleryview__price-link")
                .or_else(|| text_of(element, "a.listview__price-link"))
                .unwrap_or_default();
            let words: Vec<&str> = price.split_whitespace().collect();
            if words.len() > 1 {
                builder = builder.metadata("price", format!("Bestes Angebot: {}€", words[1]));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Split a sort:<name> token off the query, like upstream's SORT_RE.
fn split_sort(text: &str) -> (String, Option<String>) {
    let Some(index) = text.find("sort:") else {
        return (text.to_string(), None);
    };
    let rest = &text[index + 5..];
    let length = rest
        .char_indices()
        .find(|(_, character)| !(character.is_alphanumeric() || *character == '_'))
        .map(|(offset, _)| offset)
        .unwrap_or(rest.len());
    if length == 0 {
        return (text.to_string(), None);
    }
    let sort = rest[..length].to_string();
    let mut remaining = String::with_capacity(text.len());
    remaining.push_str(&text[..index]);
    remaining.push_str(&rest[length..]);
    (clean(&remaining), Some(sort))
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<html><body>
      <article class="listview__item">
        <a class="listview__name-link" href="/a/123/testprodukt">
          <img class="listview__image" src="https://cdn.geizhals.de/test.jpg">
          <h3 class="listview__name">Testprodukt</h3>
        </a>
        <div class="specs-grid__item"><dt>Hersteller</dt><dd>Test GmbH</dd></div>
        <div class="specs-grid__item"><dt>EAN</dt><dd>12345</dd></div>
        <div class="stars-rating-label">4.5 von 5</div>
        <div class="listview__offercount">12 Angebote</div>
        <a class="listview__price-link">ab 99,00 €</a>
      </article>
      <article class="listview__item">
        <a class="listview__name-link" href="/a/999/leer"><h3 class="listview__name"></h3></a>
      </article>
    </body></html>"#;

    fn engine() -> Arc<Geizhals> {
        let config = EngineConfig::new("geizhals", "geizhals", "geiz", &["shopping"]);
        Geizhals::from_config(&config).unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_listview_items() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("testprodukt")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                let metadata = |key: &str| result.metadata.get(key).cloned().unwrap_or_default();
                assert_eq!(result.title, "Testprodukt");
                assert_eq!(result.url, "https://geizhals.de/a/123/testprodukt");
                assert_eq!(result.content, "Hersteller: Test GmbH | EAN: 12345");
                assert_eq!(metadata("metadata"), "4.5 von 5, 12 Angebote");
                assert_eq!(metadata("price"), "Bestes Angebot: 99,00€");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://cdn.geizhals.de/test.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn parses_galleryview_items() {
        let fixture = r#"<html><body>
          <article class="galleryview__item card">
            <div class="galleryview__image-wrapper">
              <a href="/a/323578/logitech.html" class="galleryview__image-link">
                <img src="https://gzhls.at/pix/0e/6b/x-m.webp" class="galleryview__image">
              </a>
            </div>
            <h3 class="galleryview__name">
              <a href="/a/323578/logitech.html" class="galleryview__name-link">Logitech PC Headset 960</a>
            </h3>
            <div class="galleryview__rating-wrapper">
              <a href="/a/323578/logitech.html#ratings" class="galleryview__rating-link">
                <span class="stars-rating-label">4.6</span>
              </a>
            </div>
            <div class="galleryview__offercount">
              <a href="/a/323578/logitech.html#offerlist" class="galleryview__offercount-link">86 Angebote</a>
            </div>
            <a href="/a/323578/logitech.html#offerlist" class="galleryview__price-link">
              <span class="price">&euro; 14,14</span>
            </a>
          </article>
        </body></html>"#;
        let response = EngineResponse::new(200, BASE_URL, fixture.as_bytes().to_vec());
        let items = engine().parse(&response, &query("kopfhoerer")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Logitech PC Headset 960");
                assert_eq!(result.url, "https://geizhals.de/a/323578/logitech.html");
                let metadata = |key: &str| result.metadata.get(key).cloned().unwrap_or_default();
                assert_eq!(metadata("metadata"), "4.6, 86 Angebote");
                assert_eq!(metadata("price"), "Bestes Angebot: 14,14€");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://gzhls.at/pix/0e/6b/x-m.webp")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_url() {
        let request = engine().build_request(&query("grafikkarte"), 1).unwrap();
        assert_eq!(
            request.url,
            "https://geizhals.de/?fs=grafikkarte&pg=1&toggle_all=1"
        );
        let request = engine()
            .build_request(&query("sort:price grafikkarte"), 2)
            .unwrap();
        assert_eq!(
            request.url,
            "https://geizhals.de/?fs=grafikkarte&pg=2&toggle_all=1&sort=p"
        );
        let request = engine()
            .build_request(&query("grafikkarte sort:desc"), 1)
            .unwrap();
        assert!(request.url.ends_with("&sort=-p"));
    }
}
