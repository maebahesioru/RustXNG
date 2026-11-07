//! Google Videos engine, ported from searx/engines/google_videos.py.
// engine-id: google_videos

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos", "web"],
    shortcut: "gov",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.google.com/wml/search";

/// Google video search through the mobile (WML) endpoint.
pub struct GoogleVideos {
    meta: EngineMeta,
}

impl GoogleVideos {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for GoogleVideos {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let language = if language == "all" {
            "en".to_string()
        } else {
            language
        };
        let mut pairs = vec![
            ("q", query.query_text()),
            ("sca_esv", "1".to_string()),
            ("hl", language),
            ("ie", "utf8".to_string()),
            ("oe", "utf8".to_string()),
            ("tbm", "vid".to_string()),
        ];
        let start = pageno.saturating_sub(1).saturating_mul(10);
        if start > 0 {
            pairs.push(("start", start.to_string()));
        }
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "d",
                TimeRange::Week => "w",
                TimeRange::Month => "m",
                TimeRange::Year => "y",
            };
            pairs.push(("tbs", format!("qdr:{value}")));
        }
        if query.safesearch > 0 {
            let safe = if query.safesearch == 1 {
                "medium"
            } else {
                "high"
            };
            pairs.push(("safe", safe.to_string()));
        }
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("User-Agent", "Nokia6230/2.0 (05.50) Profile/MIDP-2.0")
                .header("Accept", "*/*"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        if response.status == 302 || (body.len() < 2000 && body.contains("/sorry/")) {
            return Err(EngineError::Captcha);
        }
        if url::Url::parse(&response.final_url).is_ok_and(|url| {
            url.host_str() == Some("sorry.google.com") || url.path().starts_with("/sorry")
        }) {
            return Err(EngineError::Captcha);
        }
        let body = body.trim_start();
        let body = match body.strip_prefix("<?xml") {
            Some(rest) => rest.split_once("?>").map(|(_, rest)| rest).unwrap_or(rest),
            None => body,
        };
        let html = parse_html(body);
        let mut results = Vec::new();
        for element in select_elements(&html, "div[class*='zMzFAb']")? {
            let Some(title) = text_of(element, "span[class*='CVA68e']") else {
                continue;
            };
            let Some(raw_url) = attr_of(element, "a[class*='fuLhoc']", "href") else {
                continue;
            };
            let target = unwrap_google_url(&raw_url);
            let Some(url) = absolute_url(&target, "https://www.google.com/") else {
                continue;
            };
            if !url.starts_with("http") {
                continue;
            }
            let mut thumbnail = attr_of(element, "img[class*='SygO9d']", "src").unwrap_or_default();
            if thumbnail.contains("/default.jpg") {
                thumbnail = thumbnail
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .replace("/default.jpg", "/hqdefault.jpg");
            }
            let mut builder = ResultBuilder::new(url, title).template(Template::Videos);
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            let spans = children(element, "span[class*='YVIcad']");
            if let Some(length) = spans.into_iter().find_map(|span| duration(&text(span))) {
                builder = builder.metadata("length", length);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream's unwrap_google_url: only the /url?q= redirector is unwrapped.
fn unwrap_google_url(raw: &str) -> String {
    match raw.strip_prefix("/url?q=") {
        Some(rest) => {
            let encoded = rest.split("&sa=U").next().unwrap_or_default();
            percent_encoding::percent_decode_str(encoded)
                .decode_utf8_lossy()
                .to_string()
        }
        None => raw.to_string(),
    }
}

/// Upstream's parse_duration_string, rendered the way a timedelta is.
fn duration(value: &str) -> Option<String> {
    let parts: Vec<&str> = value.trim().split(':').collect();
    let mut seconds = 0i64;
    for part in &parts[parts.len().saturating_sub(3)..] {
        seconds = seconds * 60 + part.trim().parse::<i64>().ok()?;
    }
    let seconds = seconds.max(0);
    let (hours, minutes, seconds) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    Some(if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<GoogleVideos> {
        let config = EngineConfig::new("google videos", "google_videos", "gov", &["videos"]);
        GoogleVideos::from_config(&config).unwrap()
    }

    const FIXTURE: &str = r##"<?xml version="1.0"?>
    <div class="zMzFAb"><a class="fuLhoc" href="/url?q=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3Dabc&amp;sa=U">w</a><span class="CVA68e">A video title</span><img class="SygO9d" src="https://i.ytimg.com/vi/abc/default.jpg?sqp=x"><span class="YVIcad">3:12</span></div>
    <div class="zMzFAb"><a class="fuLhoc" href="https://example.com/b">b</a><span class="CVA68e">Second video</span><span class="YVIcad">no duration</span><span class="YVIcad">1:02:03</span></div><div class="zMzFAb"><span class="CVA68e">No link</span></div>"##;

    #[test]
    fn parses_the_video_blocks() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(first) = &items[0] else {
            panic!("expected a video")
        };
        assert_eq!(first.url, "https://www.youtube.com/watch?v=abc");
        assert_eq!(first.title, "A video title");
        assert_eq!(first.template, Template::Videos);
        assert_eq!(
            first.thumbnail.as_deref(),
            Some("https://i.ytimg.com/vi/abc/hqdefault.jpg")
        );
        assert_eq!(
            first.metadata.get("length").map(String::as_str),
            Some("03:12")
        );
        let EngineItem::Result(second) = &items[1] else {
            panic!("expected a video")
        };
        assert_eq!(
            second.metadata.get("length").map(String::as_str),
            Some("01:02:03")
        );
    }

    #[test]
    fn builds_the_mobile_search_url() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        query.safesearch = 2;
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://www.google.com/wml/search?")
        );
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("hl=en"));
        assert!(request.url.contains("tbm=vid"));
        assert!(request.url.contains("start=10"));
        assert!(request.url.contains("tbs=qdr%3Aw"));
        assert!(request.url.contains("safe=high"));
    }
}
