//! Flickr engine, ported from `searx/engines/flickr_noapi.py`.
//!
//! This is the HTML implementation (without the official API).
//! The result page embeds its state as a `modelExport` JavaScript object; the
//! legend links each photo to its position in the nested model.
// engine-id: flickr_noapi

use std::sync::Arc;

use regex::Regex;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images"],
    shortcut: "fl",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://www.flickr.com/search";
/// From the biggest to the lowest format.
const IMAGE_SIZES: [&str; 11] = ["o", "k", "h", "b", "c", "z", "m", "n", "t", "q", "s"];

/// Flickr, parsed from the HTML search page.
pub struct FlickrNoapi {
    meta: EngineMeta,
}

impl FlickrNoapi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for FlickrNoapi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut url = format!(
            "{SEARCH_URL}?{}&page={}",
            query_string(&[("text", query.query_text())]),
            pageno.max(1)
        );
        if let Some(range) = query.time_range {
            url.push_str(&time_range_suffix(range));
        }
        Ok(EngineRequest::get(url))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let Some(model_export) = model_export(&body) else {
            return Ok(Vec::new());
        };
        let Some(legend) = model_export
            .get("legend")
            .and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        if legend.is_empty() {
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        for index in legend {
            let Some(photo) = photo_at(&model_export, index) else {
                continue;
            };
            if let Some(result) = parse_photo(photo) {
                results.push(result);
            }
        }
        Ok(results)
    }
}

/// The JSON payload of the `modelExport` assignment.
fn model_export(body: &str) -> Option<serde_json::Value> {
    let regex = Regex::new(r"(?m)^\s*modelExport:\s*(\{.*\}),\s*$").ok()?;
    let captures = regex.captures(body)?;
    let payload = captures.get(1)?.as_str();
    serde_json::from_str(payload).ok()
}

/// Upstream's 8 element legend index into `model_export`.
fn photo_at<'a>(
    model: &'a serde_json::Value,
    index: &serde_json::Value,
) -> Option<&'a serde_json::Value> {
    let keys = index.as_array()?;
    if keys.len() != 8 {
        return None;
    }
    let mut current = model.get("main")?;
    for key in keys {
        let segment = match key {
            serde_json::Value::String(segment) => segment.clone(),
            serde_json::Value::Number(segment) => segment.to_string(),
            _ => return None,
        };
        current = match current {
            serde_json::Value::Object(map) => map.get(&segment)?,
            serde_json::Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn parse_photo(photo: &serde_json::Value) -> Option<EngineItem> {
    let sizes = photo.get("sizes").and_then(|sizes| sizes.get("data"));
    let mut size_data = None;
    if let Some(sizes) = sizes {
        for size in IMAGE_SIZES {
            if let Some(data) = sizes.get(size).and_then(|entry| entry.get("data")) {
                size_data = Some(data);
                break;
            }
        }
    }
    let size_data = size_data?;
    let img_src = absolute_url(&json_str(size_data, "url"));
    if !is_http_url(&img_src) {
        return None;
    }
    let resolution = format!(
        "{} x {}",
        json_str(size_data, "width"),
        json_str(size_data, "height")
    );
    let thumbnail = sizes
        .and_then(|sizes| sizes.get("n"))
        .and_then(|entry| entry.get("data"))
        .map(|data| absolute_url(&json_str(data, "url")))
        .filter(|url| !url.is_empty())
        .or_else(|| {
            sizes
                .and_then(|sizes| sizes.get("z"))
                .and_then(|entry| entry.get("data"))
                .map(|data| absolute_url(&json_str(data, "url")))
                .filter(|url| !url.is_empty())
        })
        .unwrap_or_else(|| img_src.clone());
    let photo_id = json_str(photo, "id");
    let url = match photo.get("ownerNsid").and_then(serde_json::Value::as_str) {
        Some(owner) => format!("https://www.flickr.com/photos/{owner}/{photo_id}"),
        None => img_src.clone(),
    };
    if !is_http_url(&url) {
        return None;
    }
    let title = {
        let title = json_str(photo, "title");
        if title.is_empty() {
            photo_id.clone()
        } else {
            title
        }
    };
    if title.is_empty() {
        return None;
    }
    let source = {
        let username = json_str(photo, "username");
        if username.is_empty() {
            String::new()
        } else {
            format!("{username} @ Flickr")
        }
    };
    Some(
        ResultBuilder::new(url, title)
            .image(img_src)
            .thumbnail(thumbnail)
            .author(json_str(photo, "realname"))
            .content(strip_tags(&json_str(photo, "description")))
            .template(Template::Images)
            .metadata("resolution", resolution)
            .metadata("source", source)
            .build(),
    )
}

/// Upstream's `_get_time_range_url` (note: it passes the current time as
/// `min_upload_date` and the shifted time as `max_upload_date`).
fn time_range_suffix(range: TimeRange) -> String {
    let now = chrono::Utc::now().timestamp();
    let delta = match range {
        TimeRange::Day => 86_400,
        TimeRange::Week => 604_800,
        TimeRange::Month => 2_419_200,
        TimeRange::Year => 31_449_600,
    };
    format!("&min_upload_date={now}&max_upload_date={}", now - delta)
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Flickr serves its asset URLs protocol relative, so every image on the page
/// arrives as a bare double slash path that the http checks below would drop.
fn absolute_url(url: &str) -> String {
    match url.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHOTO: &str = r#"{"id": "12345678", "ownerNsid": "1234@N01", "realname": "Jane Doe", "username": "jane", "title": "A photo", "description": "A <b>photo</b>", "sizes": {"data": {"n": {"data": {"url": "https://live.staticflickr.com/n.jpg", "width": 320, "height": 240}}, "z": {"data": {"url": "https://live.staticflickr.com/z.jpg", "width": 640, "height": 480}}, "o": {"data": {"url": "https://live.staticflickr.com/o.jpg", "width": 2048, "height": 1536}}}}}"#;

    fn fixture() -> String {
        let photo: serde_json::Value = serde_json::from_str(PHOTO).unwrap();
        let model = serde_json::json!({
            "legend": [["0", "0", "a", "b", "c", "d", "0", "e"]],
            "main": {"0": [{"a": {"b": {"c": {"d": [{"e": photo}]}}}}]}
        });
        format!(
            "<html><head><script>\n  modelExport: {},\n</script></head><body></body></html>",
            model
        )
    }

    fn engine() -> Arc<FlickrNoapi> {
        FlickrNoapi::from_config(&EngineConfig::new(
            "flickr",
            "flickr_noapi",
            "fl",
            &["images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_model_export() {
        let body = fixture();
        let response = EngineResponse::new(200, SEARCH_URL, body.as_bytes().to_vec());
        let query = Query::parse("photo", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A photo");
                assert_eq!(
                    result.url,
                    "https://www.flickr.com/photos/1234@N01/12345678"
                );
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://live.staticflickr.com/o.jpg")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://live.staticflickr.com/n.jpg")
                );
                assert_eq!(result.content, "A photo");
                assert_eq!(
                    result.metadata.get("resolution").map(String::as_str),
                    Some("2048 x 1536")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn protocol_relative_assets_are_absolutised() {
        // Flickr serves every asset as "//live.staticflickr.com/...", so the
        // http check used to drop the whole legend.
        let relative = PHOTO.replace("https://live.staticflickr.com/", "//live.staticflickr.com/");
        let photo: serde_json::Value = serde_json::from_str(&relative).unwrap();
        let model = serde_json::json!({
            "legend": [["0", "0", "a", "b", "c", "d", "0", "e"]],
            "main": {"0": [{"a": {"b": {"c": {"d": [{"e": photo}]}}}}]}
        });
        let body = format!(
            "<html><head><script>\n  modelExport: {},\n</script></head><body></body></html>",
            model
        );
        let response = EngineResponse::new(200, SEARCH_URL, body.as_bytes().to_vec());
        let query = Query::parse("photo", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(
            result.img_src.as_deref(),
            Some("https://live.staticflickr.com/o.jpg")
        );
        assert_eq!(
            result.thumbnail.as_deref(),
            Some("https://live.staticflickr.com/n.jpg")
        );
    }

    #[test]
    fn pages_without_a_model_export_are_empty() {
        let response = EngineResponse::new(200, SEARCH_URL, b"<html></html>".to_vec());
        let query = Query::parse("photo", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn builds_the_paged_request() {
        let mut query = Query::parse("cat photo", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://www.flickr.com/search?text=cat%20photo&page=2")
        );
        assert!(request.url.contains("&min_upload_date="));
        assert!(request.url.contains("&max_upload_date="));
    }
}
