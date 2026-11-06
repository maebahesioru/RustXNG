//! Public domain image archive engine, ported from searx/engines/public_domain_image_archive.py.
//! The site is a Svelte single page app that queries an Algolia proxy on AWS
//! Lambda; the proxy URL is baked into the InfiniteSearch bundle, so this port
//! uses it as the default and lets the api_url setting override it. The proxy
//! answers without the search token the page fetches (it is optional), which
//! keeps the request single-shot. Metadata lines are joined with a pipe
//! because the result builder collapses the newline upstream uses.
// engine-id: public_domain_image_archive

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images"],
    shortcut: "pdia",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://pdimagearchive.org";
/// The Algolia proxy found in /_astro/InfiniteSearch.*.js; the page posts
/// {"indexName","query","page","hitsPerPage","filters"?,"token"} here.
const DEFAULT_API_URL: &str =
    "https://tc1386gc0f.execute-api.eu-west-1.amazonaws.com/default/search-proxy";
const THUMBNAIL_SUFFIX: &str = "?fit=max&h=360&w=360";
const PAGE_SIZE: i64 = 20;

/// Public domain image archive, through its Algolia proxy.
pub struct PublicDomainImageArchive {
    meta: EngineMeta,
    page_size: i64,
}

impl PublicDomainImageArchive {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let page_size = config.opt_i64("page_size").unwrap_or(PAGE_SIZE);
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta, page_size }))
    }

    fn base_url(&self) -> &str {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        base.trim_end_matches('/')
    }

    fn api_url(&self) -> String {
        match self.meta.opt_str("api_url") {
            Some(url) => url.trim_end_matches('/').to_string(),
            None => DEFAULT_API_URL.to_string(),
        }
    }
}

impl Engine for PublicDomainImageArchive {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let body = serde_json::json!({
            "page": pageno.saturating_sub(1),
            "query": query.query_text(),
            "hitsPerPage": self.page_size,
            "indexName": "prod_all-images",
        });
        Ok(EngineRequest::post(self.api_url())
            .header("Content-Type", "application/json")
            .json_body(body))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        if response.status == 403 {
            return Err(EngineError::AccessDenied);
        }
        if response.status != 200 {
            return Err(EngineError::Message(format!(
                "public domain image archive returned HTTP {}",
                response.status
            )));
        }
        let json = response.json()?;
        let Some(hits) = json_path(&json, "results.0.hits").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let base = self.base_url();
        let mut results = Vec::new();
        for hit in hits {
            let object_id = json_str(hit, "objectID");
            let thumbnail = json_str(hit, "thumbnail");
            let title = json_str(hit, "title");
            if object_id.is_empty() || thumbnail.is_empty() || title.trim().is_empty() {
                continue;
            }
            let image = thumbnail.split('?').next().unwrap_or_default().to_string();
            let mut content = Vec::new();
            let themes = json_str(hit, "themes");
            if !themes.is_empty() {
                content.push(format!("Themes: {themes}"));
            }
            let work = json_str(hit, "encompassingWork");
            if !work.is_empty() {
                content.push(format!("Encompassing work: {work}"));
            }
            let url = clean_url(&format!("{base}/images/{object_id}"));
            if !url.starts_with("http") {
                continue;
            }
            let title = format!(
                "{} by {} {}",
                title.trim(),
                json_str(hit, "artist"),
                json_str(hit, "displayYear")
            );
            results.push(
                ResultBuilder::new(url, title)
                    .content(content.join(" | "))
                    .template(Template::Images)
                    .image(clean_url(&image))
                    .thumbnail(clean_url(&format!("{image}{THUMBNAIL_SUFFIX}")))
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Drop the imgix tracking parameters, like upstream.
fn clean_url(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    let kept: Vec<String> = parsed
        .query_pairs()
        .filter(|(key, _)| key != "ixid" && key != "s")
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.set_query(Some(&kept.join("&")));
    }
    parsed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Method;

    const FIXTURE: &str = r#"{"results":[{"hits":[
      {"objectID":"abc123","title":"A print","artist":"Someone","displayYear":"1658",
       "thumbnail":"https://img.example/print.jpg?ixid=XYZ&s=abc",
       "themes":"Animals","encompassingWork":"A book"},
      {"objectID":"","title":"Broken","artist":"Nobody","thumbnail":""}
    ]}]}"#;

    fn engine() -> Arc<PublicDomainImageArchive> {
        let mut config = EngineConfig::new(
            "public domain image archive",
            "public_domain_image_archive",
            "pdia",
            &["images"],
        );
        config.extra.insert(
            "api_url".into(),
            toml::Value::String("https://example.test/search-proxy".into()),
        );
        PublicDomainImageArchive::from_config(&config).unwrap()
    }

    #[test]
    fn parses_the_algolia_hits() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!()
        };
        assert_eq!(result.url, "https://pdimagearchive.org/images/abc123");
        assert_eq!(result.title, "A print by Someone 1658");
        assert_eq!(result.template, Template::Images);
        let content = "Themes: Animals | Encompassing work: A book";
        assert_eq!(result.content, content);
        let image = result.img_src.clone().unwrap_or_default();
        assert!(image.ends_with("/print.jpg"));
        let thumb = result.thumbnail.clone().unwrap_or_default();
        assert!(thumb.ends_with("?fit=max&h=360&w=360"));
    }

    #[test]
    fn defaults_to_the_lambda_proxy() {
        let config = EngineConfig::new(
            "public domain image archive",
            "public_domain_image_archive",
            "pdia",
            &["images"],
        );
        let engine = PublicDomainImageArchive::from_config(&config).unwrap();
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 1).unwrap();
        assert_eq!(request.url, DEFAULT_API_URL);
    }

    #[test]
    fn posts_the_algolia_query() {
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.url, "https://example.test/search-proxy");
        let body = request.json.clone().unwrap();
        assert_eq!(body["page"], serde_json::json!(2));
        assert_eq!(body["query"], serde_json::json!("cat"));
        assert_eq!(body["hitsPerPage"], serde_json::json!(20));
        assert_eq!(body["indexName"], serde_json::json!("prod_all-images"));
    }
}
