//! Google Custom Search Engine, ported from searx/engines/google_cse.py.
// engine-id: google_cse

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "goc",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const ENDPOINT: &str = "https://cse.google.com/cse/element/v1";
const DEFAULT_CX: &str = "partner-pub-8993703457585266:4862972284";
const DEFAULT_PAGE_SIZE: i64 = 20;

/// Google CSE, the JSONP element endpoint of a custom search engine.
///
/// The token (upstream fetches it from cse.js) is configured; the image
/// category is not ported.
pub struct GoogleCse {
    meta: EngineMeta,
}

impl GoogleCse {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        if !config.opt_str("google_categ").unwrap_or("").is_empty() {
            return Err("google_cse: only the web category is ported".to_string());
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn is_http(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Upstream web_item: a web result of the element endpoint.
fn web_item(item: &serde_json::Value) -> Option<EngineItem> {
    let url = json_str(item, "unescapedUrl");
    let title = json_str(item, "titleNoFormatting");
    if !is_http(&url) || title.is_empty() {
        return None;
    }
    let content = json_str(item, "contentNoFormatting");
    let mut builder = ResultBuilder::new(url, title).content(content);
    let thumbnail = json_str(item, "richSnippet.cseThumbnail.src");
    if is_http(&thumbnail) {
        builder = builder.thumbnail(thumbnail);
    }
    Some(builder.build())
}

impl Engine for GoogleCse {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let token = self.meta.opt_str("cse_tok").map(str::trim);
        let token = token
            .filter(|token| !token.is_empty())
            .ok_or_else(|| EngineError::Message("google_cse needs a cse_tok".into()))?;
        let page_size = self.meta.opt_i64("page_size").filter(|size| *size > 0);
        let page_size = page_size.unwrap_or(DEFAULT_PAGE_SIZE);
        let language = locale_language(&query.locale);
        let language = if language == "all" {
            "en".to_string()
        } else {
            language
        };
        let safe = match query.safesearch {
            0 => "off",
            1 => "medium",
            _ => "high",
        };
        let cx = self.meta.opt_str("cx").unwrap_or(DEFAULT_CX);
        let mut pairs: Vec<(&str, String)> = vec![
            ("rsz", "filtered_cse".to_string()),
            ("num", page_size.to_string()),
            ("hl", language),
            ("cx", cx.to_string()),
            ("q", query.query_text()),
            ("safe", safe.to_string()),
            ("cse_tok", token.to_string()),
            ("callback", "_".to_string()),
            ("rurl", String::new()),
        ];
        if let Some(seconds) = TimeRange::seconds(query.time_range) {
            let end = chrono::Utc::now();
            let start = end - chrono::Duration::seconds(seconds);
            let sort = format!("date:r:{}:{}", start.format("%Y%m%d"), end.format("%Y%m%d"));
            pairs.push(("sort", sort));
        }
        let region = locale_region(&query.locale);
        if region != "all" {
            pairs.push(("gl", region.to_uppercase()));
        }
        if pageno > 1 {
            pairs.push(("start", (i64::from(pageno - 1) * page_size).to_string()));
        }
        let url = format!("{ENDPOINT}?{}", query_string(&pairs));
        Ok(EngineRequest::get(url).header("Referer", "https://cse.google.com/"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        // The element endpoint answers with a JSONP callback, not plain JSON.
        let body = response.text();
        let jsonp = body.find('{').zip(body.rfind('}'));
        let Some(jsonp) = jsonp.and_then(|(first, last)| body.get(first..=last)) else {
            return Ok(Vec::new());
        };
        let json: serde_json::Value = serde_json::from_str(jsonp)
            .map_err(|error| EngineError::Parse(format!("invalid google cse response: {error}")))?;
        if json.get("error").is_some() {
            if json_path(&json, "error.code").and_then(|value| value.as_i64()) == Some(429) {
                return Err(EngineError::TooManyRequests);
            }
            let message = json_str(&json, "error.message");
            return Err(EngineError::Message(format!("google cse: {message}")));
        }
        let Some(items) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            if let Some(result) = web_item(item) {
                results.push(result);
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<GoogleCse> {
        let mut config = EngineConfig::new("google cse", "google_cse", "goc", &["general"]);
        config
            .extra
            .insert("cse_tok".to_string(), toml::Value::String("tok".into()));
        GoogleCse::from_config(&config).expect("google_cse engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_element_request() {
        let mut config = EngineConfig::new("google cse", "google_cse", "goc", &["general"]);
        config.extra.insert(
            "google_categ".to_string(),
            toml::Value::String("image".into()),
        );
        assert!(GoogleCse::from_config(&config).is_err());
        let mut search = query("rust lang");
        search.locale = "ja-JP".to_string();
        search.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&search, 2).expect("request");
        assert!(request.url.contains("q=rust%20lang"));
        assert!(request.url.contains("cse_tok=tok"));
        assert!(request.url.contains("hl=ja"));
        assert!(request.url.contains("gl=JP"));
        assert!(request.url.contains("start=20"));
        assert!(request.url.contains("safe=off"));
        let plain = EngineConfig::new("google cse", "google_cse", "goc", &["general"]);
        let engine = GoogleCse::from_config(&plain).expect("engine");
        assert!(engine.build_request(&query("rust"), 1).is_err());
    }

    #[test]
    fn parses_the_jsonp_answer() {
        let body = br##"_({"results": [{"unescapedUrl": "https://www.rust-lang.org/", "titleNoFormatting": "Rust", "contentNoFormatting": "A language.", "richSnippet": {"cseThumbnail": {"src": "https://rust-lang.org/l.png"}}}, {"titleNoFormatting": "no url"}]});"##;
        let response = EngineResponse::new(200, ENDPOINT, body.to_vec());
        let items = engine().parse(&response, &query("rust")).expect("items");
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust");
                assert!(result.content.starts_with("A language"));
            }
            _ => panic!("expected a result"),
        }
        let body = br##"_({"error": {"code": 429, "message": "quota"}});"##;
        let response = EngineResponse::new(200, ENDPOINT, body.to_vec());
        assert!(matches!(
            engine().parse(&response, &query("rust")),
            Err(EngineError::TooManyRequests)
        ));
        let response = EngineResponse::new(200, ENDPOINT, b"not json".to_vec());
        assert!(
            engine()
                .parse(&response, &query("rust"))
                .ok()
                .unwrap()
                .is_empty()
        );
    }
}
