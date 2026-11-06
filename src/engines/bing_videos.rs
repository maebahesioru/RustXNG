//! Bing-Videos engine, ported from `searx/engines/bing_videos.py`.

// engine-id: bing_videos

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
    shortcut: "biv",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.bing.com/videos/asyncv2";

fn time_map(range: TimeRange) -> i64 {
    match range {
        TimeRange::Day => 60 * 24,
        TimeRange::Week => 60 * 24 * 7,
        TimeRange::Month => 60 * 24 * 31,
        TimeRange::Year => 60 * 24 * 365,
    }
}

/// Bing video search.
pub struct BingVideos {
    meta: EngineMeta,
}

impl BingVideos {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for BingVideos {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("async", "content".to_string()),
            ("first", ((pageno.saturating_sub(1)) * 35 + 1).to_string()),
            ("count", "35".to_string()),
        ];
        let language = locale_language(&query.locale);
        let region = locale_region(&query.locale);
        if language != "all" && region != "all" {
            pairs.push(("setlang", language.clone()));
            pairs.push(("cc", region));
        }
        if let Some(range) = query.time_range {
            pairs.push(("form", "VRFLTR".to_string()));
            pairs.push(("qft", format!("filterui:videoage-lt{}", time_map(range))));
        }
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("Accept-Language", &format!("{language};q=0.9")),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        for element in select_elements(&html, "div[id*='mc_vtvc_video']")? {
            // The metadata is a JSON document inside the vrhm attribute.
            let Some(metadata) = attr_of(element, "div.vrhdata", "vrhm").map(|value| clean(&value))
            else {
                continue;
            };
            if metadata.is_empty() {
                continue;
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&metadata) else {
                continue;
            };
            let url = json_str(&json, "murl");
            let title = json_str(&json, "vt");
            let length = json_str(&json, "du");
            if url.is_empty() {
                continue;
            }
            let info = text_of(element, "div.mc_vtvc_meta_block span")
                .unwrap_or_default()
                .replace("  -  ", " - ");
            let thumbnail = attr_of(
                element,
                "img[class^='rms'], img[class*='rms']",
                "data-src-hq",
            )
            .unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title)
                .content(info)
                .template(Template::Videos);
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            if !length.is_empty() {
                builder = builder.metadata("length", length);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vrhm_metadata() {
        let body = r#"<div id="mc_vtvc_video">
          <div class="vrhdata" vrhm='{"murl":"https://www.example.com/video.mp4","vt":"A video","du":"PT5M32S"}'></div>
          <div class="mc_vtvc_meta_block"><span>5:32</span><span>Channel</span></div>
          <img class="rms" data-src-hq="https://img.example.com/t.jpg">
        </div>"#;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let engine = BingVideos::from_config(&EngineConfig::new(
            "bing videos",
            "bing_videos",
            "biv",
            &["videos"],
        ))
        .unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.example.com/video.mp4");
                assert_eq!(result.title, "A video");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://img.example.com/t.jpg")
                );
            }
            _ => panic!("expected a video result"),
        }
    }

    #[test]
    fn time_range_sets_filters() {
        let engine = BingVideos::from_config(&EngineConfig::new(
            "bing videos",
            "bing_videos",
            "biv",
            &["videos"],
        ))
        .unwrap();
        let mut query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Day);
        let request = engine.build_request(&query, 1).unwrap();
        assert!(request.url.contains("form=VRFLTR"));
        assert!(
            request
                .url
                .contains(&format!("qft=filterui%3Avideoage-lt{}", 60 * 24))
        );
    }
}
