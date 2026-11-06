//! Anna's Archive engine, ported from `searx/engines/annas_archive.py`.
// engine-id: annas_archive

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files", "books"],
    shortcut: "aab",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

/// Anna's Archive shadow-library metasearch.
pub struct AnnasArchive {
    meta: EngineMeta,
}

impl AnnasArchive {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        if meta.base_url.is_none() {
            return Err("missing required config `base_url`".into());
        }
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for AnnasArchive {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or_default();
        let lang = locale_language(&query.locale);
        let lang = if lang == "all" { String::new() } else { lang };
        let mut pairs: Vec<(&str, String)> = Vec::new();
        if !lang.is_empty() {
            pairs.push(("lang", lang));
        }
        if let Some(content) = self.meta.opt_str("aa_content") {
            if !content.is_empty() {
                pairs.push(("content", content.to_string()));
            }
        }
        if let Some(ext) = self.meta.opt_str("aa_ext") {
            if !ext.is_empty() {
                pairs.push(("ext", ext.to_string()));
            }
        }
        if let Some(sort) = self.meta.opt_str("aa_sort") {
            if !sort.is_empty() {
                pairs.push(("sort", sort.to_string()));
            }
        }
        pairs.push(("q", query.query_text()));
        pairs.push(("page", pageno.to_string()));
        Ok(EngineRequest::get(format!(
            "{base}/search?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.meta.base_url.as_deref().unwrap_or_default();
        // The site ships two layouts: the legacy `js-aarecord-list-outer` list and
        // the current Tailwind card list ("bg-white rounded-lg shadow p-4 mb-4").
        let mut nodes = select_elements(&html, "div.bg-white.rounded-lg.shadow.p-4.mb-4")?;
        if nodes.is_empty() {
            nodes = select_elements(&html, "main div.js-aarecord-list-outer div.flex")?;
        }
        let mut results = Vec::new();
        for item in nodes {
            let Some(href) = attr_of(item, "h3 a", "href").or_else(|| attr_of(item, "a", "href"))
            else {
                continue;
            };
            let title = text_of(item, "h3")
                .or_else(|| text_of(item, "span.js-vim-focus"))
                .or_else(|| text_of(item, "a.js-vim-focus"))
                .or_else(|| text_of(item, "a"))
                .unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let url = if href.starts_with("http") {
                href
            } else {
                format!("{base}{href}")
            };
            let mut builder = ResultBuilder::new(url, title);
            if let Some(content) = text_of(item, "p.text-sm.mt-2")
                .or_else(|| text_of(item, "div.relative div.line-clamp"))
            {
                builder = builder.content(content);
            }
            if let Some(thumb) = attr_of(item, "img", "src") {
                let thumb = if thumb.starts_with("http") {
                    thumb
                } else {
                    format!("{base}{thumb}")
                };
                builder = builder.thumbnail(thumb);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::EngineConfig;

    fn config() -> EngineConfig {
        let mut config =
            EngineConfig::new("annas_archive", "annas_archive", "aab", &["files", "books"]);
        config.base_url = Some("https://annas-archive.gl".to_string());
        config
    }

    #[test]
    fn requires_base_url() {
        let bare = EngineConfig::new("annas_archive", "annas_archive", "aab", &["files", "books"]);
        assert!(AnnasArchive::from_config(&bare).is_err());
    }

    #[test]
    fn parses_the_tailwind_card_list() {
        let html = r#"
        <html><body>
          <div class="bg-white rounded-lg shadow p-4 mb-4">
            <div class="flex gap-[18px] items-start">
              <a class="custom-a shrink-0" href="https://annas-archive.is/books/44321303-rust-programming">
                <img src="https://images.example.test/cover.jpg">
              </a>
              <div class="min-w-0 flex-1 pt-[2px]">
                <h3 class="font-bold text-lg leading-tight">
                  <a class="custom-a" href="https://annas-archive.is/books/44321303-rust-programming">Rust Programming</a>
                </h3>
                <p class="text-sm mt-2">Key Features : For application-level programming</p>
              </div>
            </div>
          </div>
        </body></html>"#;
        let response =
            EngineResponse::new(200, "https://annas-archive.is/", html.as_bytes().to_vec());
        let engine = AnnasArchive::from_config(&config()).unwrap();
        let query = Query::parse("book", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine.parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.url,
                    "https://annas-archive.is/books/44321303-rust-programming"
                );
                assert_eq!(result.title, "Rust Programming");
                assert_eq!(
                    result.content,
                    "Key Features : For application-level programming"
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn parses_the_result_list() {
        let html = r#"
        <html><main>
          <div class="js-aarecord-list-outer">
            <div class="flex">
              <a href="/md5/abc"><span class="js-vim-focus">Book Title</span></a>
              <div class="relative"><div class="line-clamp">A description</div></div>
              <img src="/cover.jpg">
            </div>
          </div>
        </main></html>"#;
        let response =
            EngineResponse::new(200, "https://annas-archive.gl/", html.as_bytes().to_vec());
        let engine = AnnasArchive::from_config(&config()).unwrap();
        let query = Query::parse("book", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine.parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://annas-archive.gl/md5/abc");
                assert_eq!(result.title, "Book Title");
                assert_eq!(result.content, "A description");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://annas-archive.gl/cover.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }
}
