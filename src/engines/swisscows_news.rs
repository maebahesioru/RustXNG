//! Swisscows News engine, ported from searx/engines/swisscows_news.py.
// engine-id: swisscows_news

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["news"],
    shortcut: "swn",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.swisscows.com";
const RESULTS_PER_PAGE: i64 = 20;

/// Swisscows news, through its public news API.
pub struct SwisscowsNews {
    meta: EngineMeta,
}

impl SwisscowsNews {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
    }
}

impl Engine for SwisscowsNews {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let count = self
            .meta
            .opt_i64("results_per_page")
            .filter(|value| *value > 0)
            .unwrap_or(RESULTS_PER_PAGE);
        // Upstream only serves a handful of "language-COUNTRY" news regions.
        let locale = appropriate_locale(&query.locale);
        let language = locale.split('-').next().unwrap_or("de").to_string();
        let freshness = match query.time_range {
            Some(TimeRange::Day) => "Day",
            Some(TimeRange::Week) => "Week",
            Some(TimeRange::Month) => "Month",
            Some(TimeRange::Year) => "Year",
            None => "All",
        };
        let offset = i64::from(pageno.max(1) - 1) * count;
        let pairs: Vec<(&str, String)> = vec![
            ("query", query.query_text()),
            ("itemsCount", count.to_string()),
            ("region", locale),
            ("language", language),
            ("offset", offset.to_string()),
            ("freshness", freshness.to_string()),
            ("sortOrder", "Desc".to_string()),
            ("sortBy", "Created".to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/news/search?{}",
            self.base(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "items").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "uri");
            let title = strip_tags(&json_str(item, "title"));
            if url.is_empty() || title.is_empty() || !is_http(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title).content(json_str(item, "description"));
            let created = json_str(item, "created");
            if !created.is_empty() {
                builder = builder.published_date(created);
            }
            let thumbnail = json_str(item, "og:image");
            if is_http(&thumbnail) {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Maps a SearXNG locale onto one of the regions the news API actually serves.
///
/// Anything else answers HTTP 501 "The service doesn't provide result for the
/// given region.", so unsupported locales fall back to the de-DE edition.
fn appropriate_locale(locale: &str) -> String {
    const SUPPORTED: &[&str] = &["de-DE", "de-AT", "de-CH", "fr-FR", "fr-CH", "it-IT"];
    let normalised = locale.trim().replace('_', "-");
    for region in SUPPORTED {
        if normalised.eq_ignore_ascii_case(region) {
            return (*region).to_string();
        }
    }
    let language = normalised
        .split('-')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match language.as_str() {
        "fr" => "fr-FR".to_string(),
        "it" => "it-IT".to_string(),
        _ => "de-DE".to_string(),
    }
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"items":[
      {"uri":"https://www.example.com/a","title":"First <b>news</b>","description":"Some text.","created":"2024-05-06T10:00:00","og:image":"https://www.example.com/a.jpg"},
      {"uri":"ftp://example.com/x","title":"Bad scheme","description":"x","created":"2024-05-06T10:00:00"},
      {"uri":"https://www.example.com/b","title":"","description":"x","created":"2024-05-06T10:00:00"}
    ]}"##;

    fn engine() -> Arc<SwisscowsNews> {
        SwisscowsNews::from_config(&EngineConfig::new(
            "swisscows news",
            "swisscows_news",
            "swn",
            &["news"],
        ))
        .unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_news_items() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("news")).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(first) = &items[0] else {
            panic!("expected a news result")
        };
        assert_eq!(first.url, "https://www.example.com/a");
        assert_eq!(first.title, "First news");
        assert_eq!(first.content, "Some text.");
        assert_eq!(first.published_date.as_deref(), Some("2024-05-06T10:00:00"));
        assert_eq!(
            first.thumbnail.as_deref(),
            Some("https://www.example.com/a.jpg")
        );
    }

    #[test]
    fn builds_the_paged_request() {
        let request = engine().build_request(&query("rust"), 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://api.swisscows.com/news/search?")
        );
        for fragment in [
            "query=rust",
            "itemsCount=20",
            "region=de-DE",
            "language=de",
            "offset=20",
            "freshness=All",
            "sortOrder=Desc",
            "sortBy=Created",
        ] {
            assert!(
                request.url.contains(fragment),
                "missing {fragment} in {}",
                request.url
            );
        }
    }

    #[test]
    fn forwards_the_time_range_and_locale() {
        let mut request_query = query("rust");
        request_query.locale = "de-DE".to_string();
        request_query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&request_query, 3).unwrap();
        assert!(request.url.contains("region=de-DE&"));
        assert!(request.url.contains("freshness=Week"));
        assert!(request.url.contains("offset=40"));
    }

    #[test]
    fn missing_items_yield_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(engine().parse(&response, &query("x")).unwrap().is_empty());
    }
}
