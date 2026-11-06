//! Z-Library engine, ported from searx/engines/zlibrary.py.
//!
//! Z-Library mirrors change often, so base_url must be configured.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

// engine-id: zlibrary

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files", "books"],
    shortcut: "",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 5.0,
};

/// A Z-Library mirror.
pub struct Zlibrary {
    meta: EngineMeta,
    mirror: String,
    /// Current mirrors (z-lib.id and friends) take the term as "/s/?q=term";
    /// the legacy layout used a path segment ("/s/term"). Setting
    /// "query_style = true" in the engine block selects the query-string form.
    query_style: bool,
}

impl Zlibrary {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let mirror = meta.base_url.clone().unwrap_or_default();
        if mirror.is_empty() {
            return Err("zlibrary requires a base_url (a working mirror)".into());
        }
        let query_style = config
            .extra
            .get("query_style")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        Ok(Arc::new(Self {
            meta,
            mirror,
            query_style,
        }))
    }
}

impl Engine for Zlibrary {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mirror = self.mirror.trim_end_matches('/');
        let page = pageno.to_string();
        if self.query_style {
            return Ok(EngineRequest::get(format!(
                "{mirror}/s/?{}",
                query_string(&[("q", query.query_text()), ("page", page)])
            )));
        }
        Ok(EngineRequest::get(format!(
            "{mirror}/s/{}?{}",
            encode(&query.query_text()),
            query_string(&[("page", page)])
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        if let Ok(Some(title)) = select_one(&html, "title") {
            if text(title).to_lowercase().contains("seized") {
                return Err(EngineError::AccessDenied);
            }
        }
        let mirror = self.mirror.trim_end_matches('/');
        let mut results = Vec::new();
        for item in select_elements(&html, "div[id='searchResultBox'] div.resItemBox")? {
            let href = attr_of(item, "a[href^='/book/']", "href").unwrap_or_default();
            if href.is_empty() {
                continue;
            }
            let url = absolute_url(&href, mirror).unwrap_or_default();
            let title = text_of(item, "[itemprop='name']").unwrap_or_default();
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let mut content: Vec<String> = Vec::new();
            let language =
                text_of(item, "div.property_language div.property_value").unwrap_or_default();
            if !language.is_empty() {
                content.push(format!("Language: {language}"));
            }
            let year = text_of(item, "div.property_year div.property_value").unwrap_or_default();
            if !year.is_empty() {
                content.push(format!("Year: {year}"));
            }
            let file_type =
                text_of(item, "div.property__file div.property_value").unwrap_or_default();
            if !file_type.is_empty() {
                content.push(format!("File: {file_type}"));
            }
            let mut builder = ResultBuilder::new(url, title).content(content.join(" | "));
            if !year.is_empty() {
                builder = builder.published_date(year);
            }
            let thumbnail = attr_of(item, "img.cover", "data-src").unwrap_or_default();
            if is_http_url(&thumbnail) {
                builder = builder.thumbnail(thumbnail);
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

    fn config() -> EngineConfig {
        let mut config = EngineConfig::new("zlibrary", "zlibrary", "", &["files", "books"]);
        config.base_url = Some("https://example-mirror.test".to_string());
        config
    }

    fn engine() -> Arc<Zlibrary> {
        Zlibrary::from_config(&config()).expect("engine")
    }

    #[test]
    fn requires_a_mirror() {
        let bare = EngineConfig::new("zlibrary", "zlibrary", "", &["files", "books"]);
        assert!(Zlibrary::from_config(&bare).is_err());
    }

    #[test]
    fn searches_the_mirror() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(request.url, "https://example-mirror.test/s/rust?page=2");
    }

    #[test]
    fn searches_with_the_query_string_layout() {
        let mut config = config();
        config
            .extra
            .insert("query_style".into(), toml::Value::Boolean(true));
        let engine = Zlibrary::from_config(&config).expect("engine");
        let query = Query::parse("rust lang", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://example-mirror.test/s/?q=rust%20lang&page=2"
        );
    }

    #[test]
    fn parses_the_book_rows() {
        let html = r##"<html><body><div id="searchResultBox">
            <div class="resItemBox">
              <a href="/book/123/abc">link</a>
              <h3 itemprop="name">The Rust Book</h3>
              <div class="property_year"><div class="property_value">2023</div></div>
              <div class="property_language"><div class="property_value">English</div></div>
              <img class="cover" data-src="https://cdn.test/cover.jpg">
            </div>
          </div></body></html>"##;
        let response =
            EngineResponse::new(200, "https://example-mirror.test", html.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example-mirror.test/book/123/abc");
                assert_eq!(result.title, "The Rust Book");
                assert!(result.content.contains("Language: English"));
            }
            _ => panic!("expected a result"),
        }
    }
}
