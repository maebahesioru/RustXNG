//! Google News engine, ported from `searx/engines/google_news.py`.
//!
//! Deviation (documented in docs/PORTING.md): upstream scrapes the Google News
//! HTML page, this port uses the official RSS feed of the same service, which
//! needs no JavaScript and carries the publication date and the source.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["news"],
    shortcut: "gnews",
    paging: false,
    time_range_support: true,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://news.google.com/rss/search";

/// Google News.
pub struct GoogleNews {
    meta: EngineMeta,
}

impl GoogleNews {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for GoogleNews {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let language = if language == "all" {
            "en".to_string()
        } else {
            language
        };
        let region = locale_region(&query.locale);
        let region = if region == "all" {
            "US".to_string()
        } else {
            region.to_uppercase()
        };

        // Google News understands when:1d / when:7d ... as part of the query.
        let mut text = query.query_text();
        if let Some(range) = query.time_range {
            let window = match range {
                TimeRange::Day => Some("1d"),
                TimeRange::Week => Some("7d"),
                TimeRange::Month => Some("30d"),
                TimeRange::Year => Some("365d"),
            };
            if let Some(window) = window {
                text = format!("{text} when:{window}");
            }
        }
        let pairs: Vec<(&str, String)> = vec![
            ("q", text),
            ("hl", format!("{language}-{region}")),
            ("gl", region.clone()),
            ("ceid", format!("{region}:{language}")),
        ];
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/rss+xml, application/xml"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let items = parse_rss(&response.text())?;
        let mut results = Vec::new();
        for item in items {
            if item.title.is_empty() || item.link.is_empty() {
                continue;
            }
            let mut content = strip_tags(&item.description);
            if let Some(source) = item.source.as_deref() {
                if !source.is_empty() {
                    content = if content.is_empty() {
                        source.to_string()
                    } else {
                        format!("{source} — {content}")
                    };
                }
            }
            results.push(
                ResultBuilder::new(item.link, item.title)
                    .content(content)
                    .published_date(item.pub_date.unwrap_or_default())
                    .template(Template::Default)
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r##"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <item>
    <title>Rust 1.99 released - Example News</title>
    <link>https://news.google.com/rss/articles/abc?oc=5</link>
    <pubDate>Sat, 17 Oct 2026 02:19:43 GMT</pubDate>
    <description>&lt;a href="https://news.google.com/rss/articles/abc"&gt;Rust 1.99 released&lt;/a&gt;&nbsp;&nbsp;&lt;font color="#6f6f6f"&gt;Example News&lt;/font&gt;</description>
    <source url="https://example.com">Example News</source>
  </item>
</channel></rss>"##;

    fn engine() -> Arc<GoogleNews> {
        GoogleNews::from_config(&EngineConfig::new(
            "google news",
            "google_news",
            "gnews",
            &["news"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_rss_feed() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust 1.99 released - Example News");
                assert!(
                    result
                        .url
                        .starts_with("https://news.google.com/rss/articles/")
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("Sat, 17 Oct 2026 02:19:43 GMT")
                );
                assert!(result.content.contains("Example News"));
                assert!(result.content.contains("Rust 1.99 released"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn time_range_becomes_a_when_filter() {
        let mut query = Query::parse("rust", &EngineRegistry::default(), &[]);
        assert!(
            !engine()
                .build_request(&query, 1)
                .unwrap()
                .url
                .contains("when")
        );
        query.time_range = Some(TimeRange::Day);
        let url = engine().build_request(&query, 1).unwrap().url;
        assert!(url.contains("when%3A1d"));
        assert!(url.contains("hl=en-US"));
    }
}
