//! Google Images engine, ported from searx/engines/google_images.py.
//!
//! Like upstream's google_request(use_locales=False): tbm=isch on the mobile
//! endpoint, parsed for the imgres links.
// engine-id: google_images

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images", "web"],
    shortcut: "goi",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

/// The mobile endpoint used by every Google engine (google.py).
const BASE_URL: &str = "https://www.google.com/wml/search";
const THUMBNAIL_URL: &str = "https://encrypted-tbn0.gstatic.com/images?q=tbn:";
/// The mobile endpoint only serves images to old handsets (google.py).
const USER_AGENT: &str =
    "Nokia7610/2.0 (5.0509.0) SymbianOS/7.0s Series60/2.1 Profile/MIDP-2.0 Configuration/CLDC-1.0";

/// Google Images.
pub struct GoogleImages {
    meta: EngineMeta,
}

impl GoogleImages {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for GoogleImages {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let mut pairs: Vec<(&str, String)> =
            vec![("q", query.query_text()), ("sca_esv", "1".to_string())];
        // google_request(use_locales=False) drops the lr and cr parameters.
        if language != "all" {
            pairs.push(("hl", language.clone()));
        }
        pairs.push(("ie", "utf8".to_string()));
        pairs.push(("oe", "utf8".to_string()));
        pairs.push(("tbm", "isch".to_string()));
        let start = (pageno.max(1) - 1) * 10;
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
        // filter_mapping = {0: images, 1: active, 2: active}; zero is not sent.
        if query.safesearch > 0 {
            pairs.push(("safe", "active".to_string()));
        }
        let url = format!("{BASE_URL}?{}", query_string(&pairs));
        Ok(EngineRequest::get(url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "*/*"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        for link in select_elements(&html, "a[href*=\"/imgres?\"]")? {
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let img_src = query_param(&href, "imgurl");
            let url = query_param(&href, "imgrefurl");
            if img_src.is_empty() || url.is_empty() || !url.starts_with("http") {
                continue;
            }
            // The title is the file name of the image, the host of the page is
            // the fallback (upstream uses urlparse).
            let mut title = file_name(&img_src);
            if title.is_empty() {
                title = host_of(&url);
            }
            if title.is_empty() {
                continue;
            }
            let width = query_param(&href, "w");
            let height = query_param(&href, "h");
            let tbnid = query_param(&href, "tbnid");
            let mut builder = ResultBuilder::new(url, title)
                .image(img_src)
                .template(Template::Images);
            if !tbnid.is_empty() {
                builder = builder.thumbnail(format!("{THUMBNAIL_URL}{tbnid}"));
            }
            if !width.is_empty() && !height.is_empty() {
                builder = builder.metadata("resolution", format!("{width} x {height}"));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The decoded value of one query parameter of an href.
fn query_param(href: &str, name: &str) -> String {
    let Some(index) = href.find('?') else {
        return String::new();
    };
    let query = &href[index + 1..];
    let query = query.split('#').next().unwrap_or(query);
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or_default();
        let value = parts.next().unwrap_or_default();
        if key == name {
            return percent_encoding::percent_decode_str(value)
                .decode_utf8_lossy()
                .to_string();
        }
    }
    String::new()
}

/// The last path segment of a URL, percent-decoded.
fn file_name(raw: &str) -> String {
    let path = raw.split(['?', '#']).next().unwrap_or(raw);
    let segment = path.rsplit('/').next().unwrap_or_default();
    percent_encoding::percent_decode_str(segment)
        .decode_utf8_lossy()
        .to_string()
}

/// The host of a URL.
fn host_of(raw: &str) -> String {
    let Some((_, rest)) = raw.split_once("://") else {
        return String::new();
    };
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body>
      <a href="/imgres?imgurl=https%3A%2F%2Fexample.com%2Fphotos%2Fcat.jpg&amp;imgrefurl=https%3A%2F%2Fexample.com%2Fanimals&amp;w=800&amp;h=600&amp;tbnid=tb42"><img src="thumb.jpg" alt=""></a>
      <a href="/imgres?imgurl=https%3A%2F%2Fexample.com%2Fno-page.jpg">no page</a>
    </body></html>"##;

    fn engine() -> Arc<GoogleImages> {
        let config = EngineConfig::new("google images", "google_images", "goi", &["images"]);
        GoogleImages::from_config(&config).unwrap()
    }

    #[test]
    fn parses_imgres_links() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/animals");
                assert_eq!(result.title, "cat.jpg");
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://example.com/photos/cat.jpg")
                );
                let thumbnail = "https://encrypted-tbn0.gstatic.com/images?q=tbn:tb42";
                assert_eq!(result.thumbnail.as_deref(), Some(thumbnail));
                let resolution = result.metadata.get("resolution").map(String::as_str);
                assert_eq!(resolution, Some("800 x 600"));
                assert_eq!(result.template, Template::Images);
            }
            _ => panic!("expected an image result"),
        }
    }

    #[test]
    fn builds_the_mobile_request() {
        let mut query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        query.locale = "ja-JP".to_string();
        query.time_range = Some(TimeRange::Week);
        query.safesearch = 2;
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://www.google.com/wml/search?q=cat")
        );
        assert!(request.url.contains("hl=ja"));
        assert!(request.url.contains("tbm=isch"));
        assert!(request.url.contains("start=10"));
        assert!(request.url.contains("tbs=qdr%3Aw"));
        assert!(request.url.contains("safe=active"));
        let accept = request
            .headers
            .get("accept")
            .and_then(|value| value.to_str().ok());
        assert_eq!(accept, Some("*/*"));
    }
}
