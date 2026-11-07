//! YouTube engine, ported from searx/engines/youtube_noapi.py.
//!
//! The search page embeds the whole result list in `ytInitialData`, so no API
//! key is needed: one GET returns twenty videos with title, channel, length and
//! thumbnail. Paging is off because the second page needs a continuation token
//! that only the YouTube client can mint.
// engine-id: youtube_noapi

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos", "music"],
    shortcut: "yt",
    paging: false,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 8.0,
};

const BASE_URL: &str = "https://www.youtube.com";
/// YouTube answers a Firefox user agent with a transport-level reset when the
/// TLS handshake does not match Firefox, so this engine pins a Chrome signature
/// instead of the shared rotating pool.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const WATCH_URL: &str = "https://www.youtube.com/watch?v=";
const EMBED_URL: &str = "https://www.youtube-nocookie.com/embed/";
const THUMB_URL: &str = "https://i.ytimg.com/vi/";

/// The YouTube video search.
pub struct YoutubeNoapi {
    meta: EngineMeta,
}

impl YoutubeNoapi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for YoutubeNoapi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let text = query.query_text();
        let mut url = format!(
            "{BASE_URL}/results?search_query={}",
            percent_encoding::utf8_percent_encode(&text, percent_encoding::NON_ALPHANUMERIC)
        );
        // The filter codes are the ones the web app uses in its own URLs.
        let filter = match query.time_range {
            Some(TimeRange::Day) => Some("EgIIAg%253D%253D"),
            Some(TimeRange::Week) => Some("EgIIAw%253D%253D"),
            Some(TimeRange::Month) => Some("EgIIBA%253D%253D"),
            Some(TimeRange::Year) => Some("EgIIBQ%253D%253D"),
            None => None,
        };
        if let Some(filter) = filter {
            url.push_str("&sp=");
            url.push_str(filter);
        }
        Ok(EngineRequest::get(url)
            // Without the consent cookie YouTube answers a consent interstitial.
            .cookie("CONSENT", "YES+")
            .header("User-Agent", USER_AGENT)
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Accept", "text/html,application/xhtml+xml"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        if body.contains("consent.youtube.com") || body.contains("Before you continue") {
            return Err(EngineError::AccessDenied);
        }
        let Some(payload) = initial_data(&body) else {
            return Ok(Vec::new());
        };
        let Ok(document) = serde_json::from_str::<serde_json::Value>(payload) else {
            return Ok(Vec::new());
        };
        let Some(sections) = json_path(&document, "contents")
            .and_then(|value| value.get("twoColumnSearchResultsRenderer"))
            .and_then(|value| value.get("primaryContents"))
            .and_then(|value| value.get("sectionListRenderer"))
            .and_then(|value| value.get("contents"))
            .and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for section in sections {
            let Some(entries) = section
                .get("itemSectionRenderer")
                .and_then(|value| value.get("contents"))
                .and_then(|value| value.as_array())
            else {
                continue;
            };
            for entry in entries {
                let Some(video) = entry.get("videoRenderer") else {
                    continue;
                };
                let Some(id) = video
                    .get("videoId")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let title = runs_text(video.get("title"));
                if title.is_empty() {
                    continue;
                }
                let mut builder = ResultBuilder::new(format!("{WATCH_URL}{id}"), title)
                    .template(Template::Videos)
                    .thumbnail(format!("{THUMB_URL}{id}/hqdefault.jpg"))
                    .metadata("iframe_src", format!("{EMBED_URL}{id}"));
                let content = runs_text(video.get("descriptionSnippet"));
                if !content.is_empty() {
                    builder = builder.content(content);
                }
                let author = runs_text(video.get("ownerText"));
                if !author.is_empty() {
                    builder = builder.metadata("author", author);
                }
                let length = video
                    .get("lengthText")
                    .map(|value| runs_text(Some(value)))
                    .unwrap_or_default();
                if !length.is_empty() {
                    builder = builder.metadata("length", length);
                }
                results.push(builder.build());
            }
        }
        Ok(results)
    }
}

/// The `ytInitialData` object literal, scanned by brace matching because the
/// page also embeds other scripts whose braces would cut a naive search short.
fn initial_data(body: &str) -> Option<&str> {
    const MARKER: &str = "ytInitialData = ";
    let start = body.find(MARKER)? + MARKER.len();
    let rest = &body[start..];
    let open = rest.find('{')?;
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for index in open..bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[open..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The text of a `{"runs": [{"text": "..."}]}` node.
fn runs_text(value: Option<&serde_json::Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if let Some(runs) = value.get("runs").and_then(|value| value.as_array()) {
        let joined = runs
            .iter()
            .filter_map(|run| run.get("text").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()
            .join("");
        return clean(&joined);
    }
    value
        .get("simpleText")
        .and_then(|value| value.as_str())
        .map(clean)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><script>var ytInitialData = {"contents":{"twoColumnSearchResultsRenderer":{"primaryContents":{"sectionListRenderer":{"contents":[{"itemSectionRenderer":{"contents":[{"videoRenderer":{"videoId":"abc123","title":{"runs":[{"text":"Rust in 100 seconds"}]},"descriptionSnippet":{"runs":[{"text":"A fast "},{"text":"tour"}]},"ownerText":{"runs":[{"text":"Fireship"}]},"lengthText":{"simpleText":"2:31"}}},{"videoRenderer":{"videoId":"","title":{"runs":[{"text":"dropped"}]}}}]}},{"continuationItemRenderer":{}}]}}}}};</script></html>"##;

    fn engine() -> Arc<YoutubeNoapi> {
        YoutubeNoapi::from_config(&EngineConfig::new(
            "youtube_noapi",
            "youtube_noapi",
            "yt",
            &["videos"],
        ))
        .unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_embedded_result_list() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust in 100 seconds");
                assert_eq!(result.url, "https://www.youtube.com/watch?v=abc123");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.content, "A fast tour");
                assert_eq!(result.metadata["author"], "Fireship");
                assert_eq!(result.metadata["length"], "2:31");
                assert_eq!(
                    result.metadata["iframe_src"],
                    "https://www.youtube-nocookie.com/embed/abc123"
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn braces_inside_strings_do_not_end_the_payload() {
        let body = r#"<script>var ytInitialData = {"a":"} not the end {","b":{"c":1}};</script>"#;
        assert_eq!(initial_data(body), Some(r#"{"a":"} not the end {","b":{"c":1}}"#));
    }

    #[test]
    fn the_consent_wall_is_a_block() {
        let response = EngineResponse::new(
            200,
            BASE_URL,
            b"<html><title>Before you continue</title></html>".to_vec(),
        );
        assert!(matches!(
            engine().parse(&response, &query()),
            Err(EngineError::AccessDenied)
        ));
    }

    #[test]
    fn the_request_carries_the_query_and_the_consent_cookie() {
        let request = engine().build_request(&query(), 1).unwrap();
        assert!(request.url.contains("search_query=rust"));
        assert_eq!(
            request.cookies,
            vec![("CONSENT".to_string(), "YES+".to_string())]
        );
    }
}
