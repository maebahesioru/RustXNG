//! DuckDuckGo Extra engine, ported from `searx/engines/duckduckgo_extra.py`.
//!
//! Images, videos or news depending on the `ddg_category` engine option.
//! DuckDuckGo guards its JSON endpoints with a `vqd` token that is handed out
//! in the search page for the very query, so the engine fetches that page
//! first and reads the token from it (the aggregator follows a
//! [`Engine::followup_request`] with whatever time the engine has left).
//!
//! The images endpoint only answers the token when it is sent as a form POST:
//! the same parameters as a GET come back as
//! `{"challenge":{"cc":"sre",...},"response_type":"botnet_challenge"}`.
// engine-id: duckduckgo_extra

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images", "videos", "news"],
    shortcut: "",
    paging: true,
    time_range_support: false,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://duckduckgo.com";

/// category -> (json path, page step)
const PATH_MAP: &[(&str, &str, u32)] = &[
    ("images", "i.js", 100),
    ("videos", "v.js", 60),
    ("news", "news.js", 30),
];

/// DuckDuckGo images/videos/news.
pub struct DuckDuckGoExtra {
    meta: EngineMeta,
    ddg_category: String,
}

impl DuckDuckGoExtra {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let ddg_category = config
            .extra
            .get("ddg_category")
            .and_then(toml::Value::as_str)
            .unwrap_or("images")
            .to_string();
        if !PATH_MAP.iter().any(|(name, _, _)| *name == ddg_category) {
            return Err(format!(
                "unsupported DuckDuckGo category {ddg_category:?} (expected images/videos/news)"
            ));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            ddg_category,
        }))
    }

    fn json_path(&self) -> &str {
        PATH_MAP
            .iter()
            .find(|(name, _, _)| *name == self.ddg_category)
            .map(|(_, path, _)| *path)
            .unwrap_or("i.js")
    }

    fn page_step(&self) -> u32 {
        PATH_MAP
            .iter()
            .find(|(name, _, _)| *name == self.ddg_category)
            .map(|(_, _, step)| *step)
            .unwrap_or(100)
    }

    /// The JSON request for a known token.
    fn json_request(&self, query: &Query, pageno: u32, vqd: &str) -> EngineRequest {
        let region = locale_region(&query.locale);
        let region = if region.is_empty() || region == "all" {
            "wt-wt".to_string()
        } else {
            region
        };
        let mut pairs: Vec<(&str, String)> = vec![
            ("o", "json".to_string()),
            ("q", query.query_text()),
            ("u", "bing".to_string()),
            ("l", region),
            ("bpia", "1".to_string()),
            ("vqd", vqd.to_string()),
            ("a", "h_".to_string()),
        ];
        let ct = if query.locale.is_empty() || query.locale == "all" {
            "EN".to_string()
        } else {
            query
                .locale
                .split('-')
                .next()
                .unwrap_or("EN")
                .to_uppercase()
        };
        pairs.push(("ct", ct));
        if pageno > 1 {
            pairs.push(("s", ((pageno - 1) * self.page_step()).to_string()));
        }
        // safesearch: 0 -> "1", 1 -> none, 2 -> "1"
        if let Some(safe) = match query.safesearch {
            0 => Some("1"),
            1 => None,
            _ => Some("1"),
        } {
            pairs.push(("p", safe.to_string()));
        }
        let path = self.json_path();
        if self.ddg_category == "images" {
            // The image endpoint only answers a POST, and only to the exact
            // parameter set its own page sends: extra parameters such as
            // l=/u=/bpia= are accepted by v.js and news.js but make i.js
            // answer 403 to any client whose TLS fingerprint is not Chrome's.
            let form = vec![
                ("q".to_string(), query.query_text().to_string()),
                ("vqd".to_string(), vqd.to_string()),
                ("f".to_string(), ",,,".to_string()),
                ("o".to_string(), "json".to_string()),
                ("p".to_string(), pageno.to_string()),
            ];
            EngineRequest::post(format!("{BASE_URL}/{path}"))
                .form(form)
                .header("Accept", "application/json, text/javascript, */*; q=0.01")
                .header("Accept-Language", "en-US,en;q=0.7")
                .header("Referer", "https://duckduckgo.com/")
                .header("Origin", "https://duckduckgo.com")
                .header("Sec-Fetch-Dest", "empty")
                .header("Sec-Fetch-Mode", "cors")
                .header("Sec-Fetch-Site", "same-origin")
        } else {
            EngineRequest::get(format!("{BASE_URL}/{path}?{}", query_string(&pairs)))
                .header("Accept", "*/*")
                .header("Referer", "https://duckduckgo.com/")
        }
    }
}

/// The token DuckDuckGo puts in front of its JSON endpoints, as it appears in
/// the search page: `vqd="4-111680579107680465429855695188216229812"`.
fn vqd_from(body: &str) -> Option<String> {
    let mut rest = body;
    while let Some(index) = rest.find("vqd") {
        let mut after = &rest[index + 3..];
        // The page spells the token both as `vqd="4-42"` and, inside its
        // inline JSON, as `"vqd":"4-42"`, so quotes and separators are skipped
        // before the digits start.
        loop {
            let trimmed = after.trim_start_matches(['"', '\'', '=', ':', ' ']);
            if trimmed.len() == after.len() {
                break;
            }
            after = trimmed;
        }
        let token: String = after
            .chars()
            .take_while(|character| character.is_ascii_digit() || *character == '-')
            .collect();
        if token.contains('-') && token.chars().any(|character| character.is_ascii_digit()) {
            return Some(token);
        }
        rest = after;
    }
    None
}

/// The page number the request carried, so the followup keeps paging.
fn pageno_from(url: &str) -> u32 {
    url.split(['?', '&'])
        .find_map(|part| part.strip_prefix("_pg="))
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1)
}

/// Format a JSON scalar (string or number) the way the markup wants it.
fn scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

impl Engine for DuckDuckGoExtra {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let text = query.query_text();
        if text.chars().count() >= 500 {
            return Err(EngineError::Message(
                "duckduckgo does not accept queries with 500 or more characters".into(),
            ));
        }
        // A token pinned in the configuration skips the page fetch entirely.
        if let Some(vqd) = self.meta.opt_str("vqd").filter(|value| !value.is_empty()) {
            return Ok(self.json_request(query, pageno, vqd));
        }
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", text),
            ("iar", self.ddg_category.clone()),
            ("iax", self.ddg_category.clone()),
            ("ia", self.ddg_category.clone()),
        ];
        if pageno > 1 {
            pairs.push(("_pg", pageno.to_string()));
        }
        let lang = locale_language(&query.locale);
        let language = if lang.is_empty() || lang == "all" {
            "en-US,en;q=0.9".to_string()
        } else {
            format!("{lang},{};q=0.7", locale_region(&query.locale))
        };
        Ok(
            EngineRequest::get(format!("{BASE_URL}/?{}", query_string(&pairs)))
                .header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                )
                .header("Accept-Language", &language)
                .header("Referer", "https://duckduckgo.com/"),
        )
    }

    fn followup_request(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Option<EngineRequest>, EngineError> {
        let body = response.text();
        if body.contains("anomaly") {
            return Err(EngineError::Captcha);
        }
        let Some(vqd) = vqd_from(&body) else {
            return Ok(None);
        };
        Ok(Some(self.json_request(
            query,
            pageno_from(&response.final_url),
            &vqd,
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        if body.contains("botnet_challenge") {
            return Err(EngineError::Captcha);
        }
        let json = response.json()?;
        let Some(results) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut items = Vec::new();
        for result in results {
            let title = strip_tags(&json_str(result, "title"));
            if title.is_empty() {
                continue;
            }
            match self.ddg_category.as_str() {
                "images" => {
                    let url = json_str(result, "url");
                    let image = json_str(result, "image");
                    let thumbnail = json_str(result, "thumbnail");
                    if url.is_empty() || image.is_empty() {
                        continue;
                    }
                    let resolution = json_path(result, "width")
                        .and_then(serde_json::Value::as_i64)
                        .zip(json_path(result, "height").and_then(serde_json::Value::as_i64))
                        .map(|(width, height)| format!("{width} x {height}"))
                        .unwrap_or_else(|| json_str(result, "resolution"));
                    items.push(
                        ResultBuilder::new(url, title)
                            .template(Template::Images)
                            .image(image)
                            .thumbnail(thumbnail)
                            .content(resolution)
                            .metadata("source", json_str(result, "source"))
                            .build(),
                    );
                }
                "videos" => {
                    let url = json_str(result, "content");
                    let thumbnail = json_path(result, "thumbnail")
                        .map(scalar)
                        .filter(|value| !value.is_empty())
                        .or_else(|| {
                            json_path(result, "images.small")
                                .map(scalar)
                                .filter(|value| !value.is_empty())
                        })
                        .or_else(|| {
                            json_path(result, "images.medium")
                                .map(scalar)
                                .filter(|value| !value.is_empty())
                        })
                        .unwrap_or_default();
                    if url.is_empty() {
                        continue;
                    }
                    let uploader = json_str(result, "uploader");
                    let uploader = if uploader.is_empty() {
                        json_str(result, "publisher")
                    } else {
                        uploader
                    };
                    items.push(
                        ResultBuilder::new(url, title)
                            .template(Template::Videos)
                            .content(json_str(result, "description"))
                            .thumbnail(thumbnail)
                            .metadata(
                                "length",
                                json_path(result, "duration")
                                    .map(scalar)
                                    .unwrap_or_default(),
                            )
                            .metadata("uploader", uploader)
                            .build(),
                    );
                }
                _ => {
                    let url = json_str(result, "url");
                    if url.is_empty() {
                        continue;
                    }
                    let published = json_path(result, "date")
                        .and_then(serde_json::Value::as_i64)
                        .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
                        .map(|instant| instant.format("%Y-%m-%d").to_string())
                        .unwrap_or_default();
                    items.push(
                        ResultBuilder::new(url, title)
                            .content(strip_tags(&json_str(result, "excerpt")))
                            .published_date(published)
                            .build(),
                    );
                }
            }
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_for(category: &str) -> Arc<DuckDuckGoExtra> {
        let mut config = EngineConfig::new(category, "duckduckgo_extra", "", &[category]);
        config
            .extra
            .insert("ddg_category".into(), toml::Value::String(category.into()));
        DuckDuckGoExtra::from_config(&config).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_news_results() {
        let fixture = r#"{
          "results": [
            {"url": "https://example.com/a", "title": "Rust News",
             "excerpt": "<b>Big</b> release", "date": 1704067200}
          ]
        }"#;
        let response = EngineResponse::new(200, BASE_URL, fixture.as_bytes().to_vec());
        let items = engine_for("news").parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.published_date.as_deref(), Some("2024-01-01"));
                assert_eq!(result.content, "Big release");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn parses_image_results() {
        let fixture = r#"{
          "results": [
            {"url": "https://example.com/page", "title": "Logo",
             "image": "https://example.com/logo.png",
             "thumbnail": "https://example.com/thumb.png",
             "width": 800, "height": 600, "source": "example"}
          ]
        }"#;
        let response = EngineResponse::new(200, BASE_URL, fixture.as_bytes().to_vec());
        let items = engine_for("images").parse(&response, &query()).unwrap();
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://example.com/logo.png")
                );
                assert_eq!(result.content, "800 x 600");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn parses_video_results() {
        let fixture = r#"{
          "results": [
            {"content": "https://youtube.example/watch", "title": "Clip",
             "description": "Watch this", "duration": "3:51",
             "thumbnail": "https://example.com/t.jpg", "uploader": "Someone"}
          ]
        }"#;
        let response = EngineResponse::new(200, BASE_URL, fixture.as_bytes().to_vec());
        let items = engine_for("videos").parse(&response, &query()).unwrap();
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://youtube.example/watch");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://example.com/t.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn vqd_tokens_are_extracted() {
        assert_eq!(
            vqd_from(r#"<html>vqd="4-42"</html>"#).as_deref(),
            Some("4-42")
        );
        assert_eq!(
            vqd_from(r#"<html><script>{"vqd":"4-111680579107680465429855695188216229812"}</script></html>"#)
                .as_deref(),
            Some("4-111680579107680465429855695188216229812")
        );
        assert_eq!(vqd_from("vqd=4-777&x=1").as_deref(), Some("4-777"));
        assert_eq!(vqd_from("nothing here"), None);
    }

    #[test]
    fn build_request_fetches_the_search_page_for_the_token() {
        let request = engine_for("images").build_request(&query(), 2).unwrap();
        assert_eq!(request.method, crate::engine::Method::Get);
        assert!(request.url.starts_with("https://duckduckgo.com/?"));
        assert!(request.url.contains("iar=images"));
        assert!(request.url.contains("_pg=2"));
    }

    #[test]
    fn followup_posts_the_json_request_for_images() {
        let page =
            r#"<html><script>{"vqd":"4-111680579107680465429855695188216229812"}</script></html>"#;
        let response = EngineResponse::new(
            200,
            "https://duckduckgo.com/?q=rust&iar=images&_pg=2",
            page.as_bytes().to_vec(),
        );
        let request = engine_for("images")
            .followup_request(&response, &query())
            .unwrap()
            .unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, "https://duckduckgo.com/i.js");
        assert!(request.form.contains(&(
            "vqd".to_string(),
            "4-111680579107680465429855695188216229812".to_string()
        )));
        assert!(request.form.contains(&("f".to_string(), ",,,".to_string())));
        assert!(
            request
                .form
                .contains(&("o".to_string(), "json".to_string()))
        );
        assert!(request.form.contains(&("p".to_string(), "2".to_string())));
        assert!(!request.form.iter().any(|(key, _)| key == "l"));
    }

    #[test]
    fn followup_gets_the_json_request_for_videos() {
        let page = r#"<html>vqd="4-42"</html>"#;
        let response = EngineResponse::new(
            200,
            "https://duckduckgo.com/?q=rust",
            page.as_bytes().to_vec(),
        );
        let request = engine_for("videos")
            .followup_request(&response, &query())
            .unwrap()
            .unwrap();
        assert_eq!(request.method, crate::engine::Method::Get);
        assert!(request.url.starts_with("https://duckduckgo.com/v.js?"));
        assert!(request.url.contains("vqd=4-42"));
    }

    #[test]
    fn a_page_without_the_token_has_no_followup() {
        let response = EngineResponse::new(200, BASE_URL, b"<html>nothing</html>".to_vec());
        assert!(
            engine_for("news")
                .followup_request(&response, &query())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn bad_category_is_rejected() {
        let mut config = EngineConfig::new("bad", "duckduckgo_extra", "", &["general"]);
        config
            .extra
            .insert("ddg_category".into(), toml::Value::String("maps".into()));
        assert!(DuckDuckGoExtra::from_config(&config).is_err());
    }
}
