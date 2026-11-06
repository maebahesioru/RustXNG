//! Shopify stock photos engine, ported from searx/engines/shopify_stock.py.
//!
//! The royalty free stock photo search of shopify.com. Upstream derives the
//! full quality image link from the photo page URL, this port does the same.
// engine-id: shopify_stock

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["stock images"],
    shortcut: "shs",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.shopify.com";

/// Shopify stock photos.
pub struct ShopifyStock {
    meta: EngineMeta,
}

impl ShopifyStock {
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

impl Engine for ShopifyStock {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("page", pageno.max(1).to_string()),
        ];
        let url = format!(
            "{}/stock-photos/photos/search?{}",
            self.base_url(),
            query_string(&pairs)
        );
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
        for element in select_elements(&html, "div.photo-card")? {
            let href = attr_of(element, "a.photo-tile__image-wrapper", "href").unwrap_or_default();
            let url = absolute_url(&href, &base)
                .map(|url| strip_query(&url))
                .unwrap_or_default();
            let title = text_of(element, "p.photo-tile__title").unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url.clone(), title)
                .template(Template::Images)
                .image(download_url(&url));
            let thumbnail = attr_of(element, "img.photo-card__image", "src")
                .or_else(|| {
                    attr_of(element, "img.photo-card__image", "data-srcset")
                        .and_then(|value| first_srcset_url(&value))
                })
                .unwrap_or_default();
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The download link of the full quality image, upstream _get_download_url.
fn download_url(url: &str) -> String {
    format!("{}/download?quality=premium", strip_query(url))
}

/// The same address without its query string.
fn strip_query(url: &str) -> String {
    match url.find('?') {
        Some(index) => url[..index].to_string(),
        None => url.to_string(),
    }
}

/// The first candidate of a srcset attribute.
fn first_srcset_url(value: &str) -> Option<String> {
    value
        .split(',')
        .next()?
        .split_whitespace()
        .next()
        .map(str::to_string)
        .filter(|url| is_http_url(url))
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body>
      <div class="grid__item">
        <div class="photo-card tile--with-overlay" data-photo-id="123">
          <div class="photo-tile">
            <a class="photo-tile__image-wrapper" href="/stock-photos/photos/happy-dog?q=dog">
              <img class="photo-card__image"
                   src="https://burst.shopifycdn.com/photos/happy-dog.jpg?width=1000"
                   data-srcset="https://burst.shopifycdn.com/photos/happy-dog.jpg?width=500 500w">
            </a>
            <p class="photo-tile__title">Happy dog</p>
          </div>
        </div>
      </div>
      <div class="grid__item">
        <div class="photo-card">
          <div class="photo-tile">
            <a class="photo-tile__image-wrapper" href="/stock-photos/photos/no-title">
              <img class="photo-card__image"
                   data-srcset="https://burst.shopifycdn.com/photos/no-title.jpg?width=500 500w">
            </a>
            <p class="photo-tile__title"></p>
          </div>
        </div>
      </div>
    </body></html>"##;

    const SRCSET_ONLY: &str = r##"<html><body>
      <div class="grid__item">
        <div class="photo-card">
          <div class="photo-tile">
            <a class="photo-tile__image-wrapper" href="/stock-photos/photos/laptop">
              <img class="photo-card__image"
                   data-srcset="https://burst.shopifycdn.com/photos/laptop.jpg?width=500 500w, https://burst.shopifycdn.com/photos/laptop.jpg?width=1000 1000w">
            </a>
            <p class="photo-tile__title">Laptop With Shopify Sticker</p>
          </div>
        </div>
      </div>
    </body></html>"##;

    fn engine() -> Arc<ShopifyStock> {
        let config = EngineConfig::new("shopify stock", "shopify_stock", "shs", &["stock images"]);
        ShopifyStock::from_config(&config).unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_photo_tiles() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("dog")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Happy dog");
                assert_eq!(
                    result.url,
                    "https://www.shopify.com/stock-photos/photos/happy-dog"
                );
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some(
                        "https://www.shopify.com/stock-photos/photos/happy-dog/download?quality=premium"
                    )
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://burst.shopifycdn.com/photos/happy-dog.jpg?width=1000")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn falls_back_to_the_srcset() {
        let response = EngineResponse::new(200, BASE_URL, SRCSET_ONLY.as_bytes().to_vec());
        let items = engine().parse(&response, &query("laptop")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Laptop With Shopify Sticker");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://burst.shopifycdn.com/photos/laptop.jpg?width=500")
                );
                assert_eq!(
                    result.img_src.as_deref(),
                    Some(
                        "https://www.shopify.com/stock-photos/photos/laptop/download?quality=premium"
                    )
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let request = engine().build_request(&query("happy dog"), 2).unwrap();
        assert_eq!(
            request.url,
            "https://www.shopify.com/stock-photos/photos/search?q=happy%20dog&page=2"
        );
    }
}
