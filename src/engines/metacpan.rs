//! metacpan engine, ported from searx/engines/metacpan.py.
// engine-id: metacpan

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "packages"],
    shortcut: "cpan",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://fastapi.metacpan.org/v1/file/_search";
const PAGE_SIZE: u32 = 20;

/// metacpan module search.
pub struct Metacpan {
    meta: EngineMeta,
}

impl Metacpan {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn page_size(&self) -> u32 {
        self.meta
            .opt_i64("page_size")
            .filter(|value| *value > 0)
            .map(|value| value.min(5000) as u32)
            .unwrap_or(PAGE_SIZE)
    }

    fn search_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| SEARCH_URL.to_string())
    }
}

impl Engine for Metacpan {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let size = self.page_size();
        let from = u64::from(pageno.max(1) - 1) * u64::from(size);
        let body = serde_json::json!({
            "query": {"bool": {
                "must": [{"multi_match": {
                    "type": "most_fields",
                    "fields": ["documentation", "documentation.*"],
                    "analyzer": "camelcase",
                    "query": query.query_text()
                }}],
                "filter": [
                    {"exists": {"field": "documentation"}},
                    {"term": {"status": "latest"}},
                    {"term": {"indexed": true}},
                    {"term": {"authorized": true}}
                ]
            }},
            "sort": [{"_score": {"order": "desc"}}, {"date": {"order": "desc"}}],
            "_source": ["documentation", "abstract"],
            "from": from,
            "size": size
        });
        Ok(EngineRequest::post(self.search_url())
            .header("Accept", "application/json")
            .header("content-type", "application/json")
            .json_body(body))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(hits) = json_path(&json, "hits.hits").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for hit in hits {
            let module = json_str(hit, "_source.documentation");
            if module.is_empty() {
                continue;
            }
            let url = format!("https://metacpan.org/pod/{module}");
            if !is_http_url(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, module)
                    .content(json_str(hit, "_source.abstract"))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"hits":{"total":2,"hits":[
      {"_source":{"documentation":"Mojo::DOM","abstract":"HTML parser"}},
      {"_source":{"abstract":"no module"}}
    ]}}"##;

    fn engine() -> Arc<Metacpan> {
        Metacpan::from_config(&EngineConfig::new(
            "metacpan",
            "metacpan",
            "cpan",
            &["it", "packages"],
        ))
        .unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_modules() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("dom")).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://metacpan.org/pod/Mojo::DOM");
        assert_eq!(result.title, "Mojo::DOM");
        assert_eq!(result.content, "HTML parser");
    }

    #[test]
    fn builds_the_post_request() {
        let request = engine().build_request(&query("dom"), 2).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, SEARCH_URL);
        let body = request.json.expect("a JSON body");
        assert_eq!(body["query"]["bool"]["must"][0]["multi_match"]["query"], "dom");
        assert_eq!(
            body["query"]["bool"]["must"][0]["multi_match"]["analyzer"],
            "camelcase"
        );
        assert_eq!(body["from"], 20);
        assert_eq!(body["size"], 20);
        assert_eq!(body["_source"][0], "documentation");
        assert_eq!(body["query"]["bool"]["filter"][2]["term"]["indexed"], true);
        assert!(body.get("filter").is_none());
        assert_eq!(body["sort"][1]["date"]["order"], "desc");
    }

    #[test]
    fn the_page_size_is_configurable() {
        let mut config = EngineConfig::new("metacpan", "metacpan", "cpan", &["it"]);
        config
            .extra
            .insert("page_size".into(), toml::Value::Integer(5));
        let engine = Metacpan::from_config(&config).unwrap();
        let request = engine.build_request(&query("dom"), 3).unwrap();
        let body = request.json.expect("a JSON body");
        assert_eq!(body["size"], 5);
        assert_eq!(body["from"], 10);
    }

    #[test]
    fn a_missing_container_yields_no_results() {
        let response = EngineResponse::new(200, SEARCH_URL, b"{}".to_vec());
        assert!(engine().parse(&response, &query("dom")).unwrap().is_empty());
    }
}
