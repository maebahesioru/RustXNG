//! Goodreads engine, ported from `searx/engines/goodreads.py`.
// engine-id: goodreads

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    // Upstream declares `categories = []`, which places the engine in "general".
    categories: &["general"],
    shortcut: "good",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.goodreads.com";

/// Goodreads.
pub struct Goodreads {
    meta: EngineMeta,
}

impl Goodreads {
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

impl Engine for Goodreads {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("page", pageno.max(1).to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/search?{}",
            self.base_url(),
            query_string(&pairs)
        ))
        // Goodreads answers any `Accept` that mentions text/html with a 302 to the brand
        // new React search UI, which the table parser below cannot read. A wildcard
        // `Accept` keeps the request on the legacy server-rendered list.
        .header("accept", "*/*"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base_url();
        let mut results = Vec::new();
        for element in select_elements(&html, "table tr")? {
            let title = text_of(element, "a.bookTitle").unwrap_or_default();
            let Some(href) = attr_of(element, "a.bookTitle", "href") else {
                continue;
            };
            let url = if href.starts_with("http://") || href.starts_with("https://") {
                href
            } else {
                format!("{base}{href}")
            };
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(text_of(element, "span.uitext").unwrap_or_default())
                    .thumbnail(attr_of(element, "img.bookCover", "src").unwrap_or_default())
                    .author(text_of(element, "a.authorName").unwrap_or_default())
                    .build(),
            );
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

    const FIXTURE: &str = r#"
    <html><body>
      <table>
        <tr itemscope itemtype="http://schema.org/Book">
          <td>
            <a class="bookTitle" href="/book/show/12345.The_Book">
              <span itemprop="name">The Book</span>
            </a>
            <a class="authorName" href="/author/show/1">An Author</a>
            <img class="bookCover" src="https://images.gr-assets.com/books/1234m/12345.jpg">
            <span class="uitext">A description of the book.</span>
          </td>
        </tr>
        <tr><td>not a book</td></tr>
      </table>
    </body></html>"#;

    fn engine() -> Arc<Goodreads> {
        Goodreads::from_config(&EngineConfig::new(
            "goodreads",
            "goodreads",
            "good",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_book_rows() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("book", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "The Book");
                assert_eq!(
                    result.url,
                    "https://www.goodreads.com/book/show/12345.The_Book"
                );
                assert_eq!(result.content, "A description of the book.");
                assert_eq!(result.author.as_deref(), Some("An Author"));
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://images.gr-assets.com/books/1234m/12345.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("the book", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://www.goodreads.com/search?q=the%20book&page=2"
        );
    }
}
