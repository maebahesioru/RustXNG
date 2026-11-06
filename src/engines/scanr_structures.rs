//! ScanR organizations engine, ported from searx/engines/scanr_structures.py.
//!
//! The upstream REST API (`/api/structures/search`) was retired; the web app now
//! queries the Elasticsearch indices behind `{base}/api/<index>/_search` directly,
//! so this engine speaks Elasticsearch.  See `scanr-organizations`.

// engine-id: scanr_structures

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science"],
    shortcut: "scs",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://scanr.enseignementsup-recherche.gouv.fr";
const INDEX: &str = "scanr-organizations";
const PAGE_SIZE: u32 = 20;

/// The ScanR open data API, limited to research structures.
pub struct ScanrStructures {
    meta: EngineMeta,
}

impl ScanrStructures {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    }

    fn search_url(&self) -> String {
        self.meta
            .search_url
            .clone()
            .unwrap_or_else(|| format!("{}/api/{}/_search", self.base_url(), INDEX))
    }

    fn page_size(&self) -> u32 {
        self.meta
            .opt_i64("page_size")
            .filter(|value| *value > 0)
            .map(|value| u32::try_from(value).unwrap_or(PAGE_SIZE))
            .unwrap_or(PAGE_SIZE)
    }
}

/// `label` is an object keyed by language; fall back to whatever it holds.
fn label_of(source: &serde_json::Value) -> String {
    for key in ["label.default", "label.fr", "label.en"] {
        let value = json_str(source, key);
        if !value.is_empty() {
            return value;
        }
    }
    json_path(source, "label")
        .and_then(|value| value.as_object())
        .and_then(|map| {
            map.values()
                .find_map(|value| value.as_str().map(str::to_string))
        })
        .unwrap_or_default()
}

/// The main address is normally (but not always) the first one.
fn address_of(source: &serde_json::Value) -> String {
    let Some(entries) = json_path(source, "address").and_then(|value| value.as_array()) else {
        return String::new();
    };
    let entry = entries
        .iter()
        .find(|entry| {
            json_path(entry, "main")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        })
        .or_else(|| entries.first());
    let Some(entry) = entry else {
        return String::new();
    };
    let city = json_str(entry, "city");
    let country = json_str(entry, "country");
    match (city.is_empty(), country.is_empty()) {
        (false, false) => format!("{city}, {country}"),
        (false, true) => city,
        (true, false) => country,
        (true, true) => String::new(),
    }
}

impl Engine for ScanrStructures {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let size = self.page_size();
        let from = u64::from(pageno.saturating_sub(1)) * u64::from(size);
        let body = serde_json::json!({
            "size": size,
            "from": from,
            "_source": [
                "id", "label", "kind", "level", "status", "address", "externalIds",
                "publicationsCount", "projectsCount", "patentsCount",
            ],
            "query": {
                "multi_match": {
                    "query": query.query_text(),
                    "type": "best_fields",
                    "fields": ["label.*^3", "alias.*", "acronym.*^2", "autocompleted.*"],
                }
            },
        });
        Ok(EngineRequest::post(self.search_url())
            .json_body(body)
            .header("Content-Type", "application/json"))
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
        let base = self.base_url();
        let mut results = Vec::new();
        for hit in hits {
            let source = json_path(hit, "_source").unwrap_or(hit);
            let id = {
                let id = json_str(source, "id");
                if id.is_empty() {
                    json_str(hit, "_id")
                } else {
                    id
                }
            };
            let title = label_of(source);
            if id.is_empty() || title.is_empty() {
                continue;
            }
            let url = format!("{base}/organizations/{}", encode(&id));

            let mut parts = Vec::new();
            let kind = json_str(source, "kind.0");
            if !kind.is_empty() {
                parts.push(kind);
            }
            let level = json_str(source, "level");
            if !level.is_empty() {
                parts.push(level);
            }
            let address = address_of(source);
            if !address.is_empty() {
                parts.push(address);
            }
            for (path, noun) in [
                ("publicationsCount", "publications"),
                ("projectsCount", "projects"),
                ("patentsCount", "patents"),
            ] {
                let count = json_path(source, path)
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0);
                if count > 0 {
                    parts.push(format!("{count} {noun}"));
                }
            }
            let content = parts.join(" · ");

            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(siren) = json_path(source, "externalIds")
                .and_then(|value| value.as_array())
                .and_then(|entries| {
                    entries.iter().find(|entry| {
                        json_path(entry, "type").and_then(|value| value.as_str()) == Some("siren")
                    })
                })
                .map(|entry| json_str(entry, "id"))
                .filter(|id| !id.is_empty())
            {
                builder = builder.metadata("siren", siren);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"took": 3, "hits": {"total": {"value": 3, "relation": "eq"}, "hits": [
      {"_id": "doc-1", "_source": {"id": "443011044",
        "label": {"default": "UNIVERSITE CLARIANE"}, "kind": ["Secteur privé"],
        "level": "Association déclarée",
        "address": [{"main": false, "city": "Paris", "country": "France"},
                     {"main": true, "city": "Clermont-Ferrand", "country": "France"}],
        "externalIds": [{"id": "443011044", "type": "siren"}],
        "publicationsCount": 12, "projectsCount": 0, "patentsCount": 2}},
      {"_id": "doc-2", "_source": {"label": {"fr": "LABO DEUX"}}},
      {"_id": "doc-3", "_source": {"id": "struct-3", "label": {"default": ""}}}
    ]}}"##;

    fn engine() -> Arc<ScanrStructures> {
        ScanrStructures::from_config(&EngineConfig::new(
            "scanr structures",
            "scanr_structures",
            "scs",
            &["science"],
        ))
        .unwrap()
    }

    fn query() -> Query {
        Query::parse("cancer", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_organization_hits() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "UNIVERSITE CLARIANE");
                assert_eq!(
                    result.url,
                    "https://scanr.enseignementsup-recherche.gouv.fr/organizations/443011044"
                );
                assert_eq!(
                    result.content,
                    "Secteur privé · Association déclarée · Clermont-Ferrand, France · 12 publications · 2 patents"
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "LABO DEUX");
                assert_eq!(
                    result.url,
                    "https://scanr.enseignementsup-recherche.gouv.fr/organizations/doc-2"
                );
                assert!(result.content.is_empty());
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_elasticsearch_request() {
        let request = engine().build_request(&query(), 2).unwrap();
        assert_eq!(
            request.url,
            "https://scanr.enseignementsup-recherche.gouv.fr/api/scanr-organizations/_search"
        );
        let body = request.json.expect("a JSON body");
        assert_eq!(body["size"], serde_json::json!(20));
        assert_eq!(body["from"], serde_json::json!(20));
        assert_eq!(
            body["query"]["multi_match"]["query"],
            serde_json::json!("cancer")
        );
        assert_eq!(
            body["query"]["multi_match"]["fields"][0],
            serde_json::json!("label.*^3")
        );
        assert!(body.get("searchField").is_none());
        assert!(request.form.is_empty());
    }

    #[test]
    fn empty_hits_yield_no_results() {
        let response = EngineResponse::new(
            200,
            BASE_URL,
            br#"{"hits": {"total": {"value": 0}, "hits": []}}"#.to_vec(),
        );
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
