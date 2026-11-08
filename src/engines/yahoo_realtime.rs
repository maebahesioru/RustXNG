//! Yahoo! JAPAN realtime search (X/Twitter posts), ported from the public
//! endpoint the realtime page itself calls.
//!
//! Nitter and the other X front ends are all offline (nitter.net answers
//! "nitter.net is offline", xcancel.com is 451, the rest are 403 or DNS dead),
//! so this is the only working route to X content from a search engine.
// engine-id: yahoo_realtime

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["social media"],
    shortcut: "yjrt",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 8.0,
};

const BASE_URL: &str = "https://search.yahoo.co.jp/realtime";
const PAGE_SIZE: i64 = 20;

/// The realtime search endpoint behind https://search.yahoo.co.jp/realtime/.
pub struct YahooRealtime {
    meta: EngineMeta,
}

impl YahooRealtime {
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

impl Engine for YahooRealtime {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("p", query.query_text()),
            (
                "results",
                self.meta
                    .opt_i64("page_size")
                    .unwrap_or(PAGE_SIZE)
                    .to_string(),
            ),
        ];
        Ok(EngineRequest::get(format!(
            "{}/api/v1/pagination?{}",
            self.base_url(),
            query_string(&pairs)
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(entries) = json_path(&json, "timeline.entry").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in entries {
            let id = json_str(entry, "id");
            let screen_name = json_str(entry, "screenName");
            let text = json_str(entry, "displayTextBody");
            if id.is_empty() || screen_name.is_empty() {
                continue;
            }
            // The API's own url carries utm_* tracking parameters; the canonical
            // permalink is the one X uses.
            let url = format!("https://x.com/{screen_name}/status/{id}");
            let name = json_str(entry, "name");
            let title = if name.is_empty() {
                format!("@{screen_name}")
            } else {
                format!("{name} (@{screen_name})")
            };
            let mut builder = ResultBuilder::new(url, title).content(text);
            let thumbnail = json_str(entry, "profileImage");
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            if let Some(seconds) = json_path(entry, "createdAt").and_then(|v| v.as_i64()) {
                if let Some(stamp) = chrono::DateTime::from_timestamp(seconds, 0) {
                    builder = builder.published_date(stamp.format("%Y-%m-%d").to_string());
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r#"{"timeline":{"head":{"totalResultsAvailable":735,"totalResultsReturned":2},
      "entry":[
        {"id":"2119420205613125829","url":"https://x.com/kuraraLL/status/2119420205613125829?utm_source=yjrealtime",
         "screenName":"kuraraLL","name":"kuraraLL","displayTextBody":"RUST の箱鯖","createdAt":1794144128,
         "profileImage":"https://rts-pctr.c.yimg.jp/abc"},
        {"id":"2119420205613125830","screenName":"someone","name":"","displayTextBody":"no name"}
      ]}}"#;

    fn engine() -> Arc<YahooRealtime> {
        let config = EngineConfig::new("yahoo_realtime", "yahoo_realtime", "yjrt", &["social media"]);
        YahooRealtime::from_config(&config).unwrap()
    }

    #[test]
    fn builds_the_pagination_request() {
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.starts_with(
            "https://search.yahoo.co.jp/realtime/api/v1/pagination?p=rust&results=20"
        ));
    }

    #[test]
    fn parses_entries_into_x_permalinks() {
        let response = EngineResponse::new(200, "https://search.yahoo.co.jp/", FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(first) = &items[0] else {
            panic!("expected a result");
        };
        assert_eq!(first.url, "https://x.com/kuraraLL/status/2119420205613125829");
        assert_eq!(first.title, "kuraraLL (@kuraraLL)");
        assert_eq!(first.content, "RUST の箱鯖");
        assert_eq!(first.published_date.as_deref(), Some("2026-11-08"));
        let EngineItem::Result(second) = &items[1] else {
            panic!("expected a result");
        };
        assert_eq!(second.title, "@someone");
    }

    #[test]
    fn an_empty_timeline_is_not_an_error() {
        let response = EngineResponse::new(
            200,
            "https://search.yahoo.co.jp/",
            br#"{"timeline":{"entry":[]}}"#.to_vec(),
        );
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }
}
