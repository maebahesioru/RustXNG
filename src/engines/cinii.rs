// SPDX-License-Identifier: AGPL-3.0-or-later
//! CiNii Research engine (no upstream searx equivalent).
// engine-id: cinii
//!
//! CiNii Research is the Japanese scholarly index run by NII. Its public
//! OpenSearch endpoint includes Japanese titles, authors and identifiers.
//!
//! The API accepts publication years or months (`YYYY` / `YYYYMM`), not
//! precise rolling day/week/month ranges. This adapter does not advertise
//! time-range support and refuses such requests rather than widening them.
//! API reference: <https://support.nii.ac.jp/ja/cir/r_opensearch>.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "ci",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

// v1 (/opensearch/all) now answers 403 "Service Unavailable" for anonymous
// clients; v2 (/opensearch/v2/all) is open and returns the flat JSON-LD shape
// this parser already expects.
const SEARCH_URL: &str = "https://cir.nii.ac.jp/opensearch/v2/all";
const PAGE_SIZE: u32 = 20;

/// CiNii Research's public OpenSearch API.
pub struct Cinii {
    meta: EngineMeta,
}

impl Cinii {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Cinii {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        if query.time_range.is_some() {
            return Err(EngineError::Message(
                "cinii does not support precise time-range filtering".into(),
            ));
        }
        let start = pageno
            .max(1)
            .checked_sub(1)
            .and_then(|page| page.checked_mul(PAGE_SIZE))
            .and_then(|offset| offset.checked_add(1))
            .filter(|start| *start <= 10_000)
            .ok_or_else(|| EngineError::Message("cinii page exceeds the API limit".into()))?;
        let pairs = vec![
            ("q", query.query_text()),
            ("format", "json".to_string()),
            ("count", PAGE_SIZE.to_string()),
            ("start", start.to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{SEARCH_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "items").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for record in items {
            let title = clean(&json_str(record, "title"));
            if title.is_empty() {
                continue;
            }
            let doi = identifier(record, "cir:DOI");
            let mut url = json_str(record, "link.@id");
            if url.is_empty() {
                url = json_str(record, "@id");
            }
            if !doi.is_empty() {
                url = format!("https://doi.org/{doi}");
            } else if url.is_empty() {
                continue;
            }
            let authors = string_array(record, "dc:creator")
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(clean)
                        .filter(|name| !name.is_empty())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let start = json_str(record, "prism:startingPage");
            let end = json_str(record, "prism:endingPage");
            let pages = if end.is_empty() {
                start
            } else {
                format!("{start}-{end}")
            };
            results.push(
                ResultBuilder::new(url, title)
                    .content(strip_tags(&json_str(record, "description")))
                    .author(authors)
                    .published_date(json_str(record, "prism:publicationDate"))
                    .metadata("doi", doi)
                    .metadata("naid", identifier(record, "cir:NAID"))
                    .metadata("journal", clean(&json_str(record, "prism:publicationName")))
                    .metadata("issn", json_str(record, "prism:issn"))
                    .metadata("volume", json_str(record, "prism:volume"))
                    .metadata("pages", pages)
                    .metadata("publisher", clean(&json_str(record, "dc:publisher")))
                    .metadata("type", json_str(record, "dc:type"))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn string_array<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a Vec<serde_json::Value>> {
    json_path(value, key).and_then(serde_json::Value::as_array)
}

/// The first `dc:identifier` entry with the given `@type`.
fn identifier(record: &serde_json::Value, kind: &str) -> String {
    let Some(entries) = json_path(record, "dc:identifier").and_then(serde_json::Value::as_array)
    else {
        return String::new();
    };
    entries
        .iter()
        .find(|entry| json_str(entry, "@type") == kind)
        .map(|entry| json_str(entry, "@value"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "@context": {"@vocab": "http://purl.org/rss/1.0/"},
      "title": "CiNii Research",
      "opensearch:totalResults": "47",
      "items": [
        {
          "@id": "https://cir.nii.ac.jp/crid/1360580232161241600",
          "@type": "item",
          "title": "Decrease in the Crystallite Diameter of Solid Crystalline Magnetite",
          "description": "<jats:p>A decrease in the crystallite diameter of ferrites.</jats:p>",
          "link": {"@id": "https://cir.nii.ac.jp/crid/1360580232161241600"},
          "dc:creator": ["Tsuchida, Takayuki", "Fukushima, Jun"],
          "dc:publisher": "Tokyo : MDPI",
          "dc:type": "Article",
          "prism:publicationName": "Nanomaterials",
          "prism:issn": "20794991",
          "prism:volume": "11",
          "prism:number": "4",
          "prism:startingPage": "984",
          "prism:endingPage": "984",
          "prism:publicationDate": "2021-04-11",
          "dc:identifier": [
            {"@type": "cir:DOI", "@value": "10.3390/nano11040984"},
            {"@type": "cir:NAID", "@value": "1390851204746911360"}
          ]
        },
        {
          "@id": "https://cir.nii.ac.jp/crid/1520853835155044608",
          "@type": "item",
          "title": "マグネタイト混合ペレットの還元ミクロ組織について",
          "link": {"@id": "https://cir.nii.ac.jp/crid/1520853835155044608"},
          "dc:creator": ["李 海洙"],
          "dc:type": "Article",
          "prism:publicationName": "生産研究",
          "prism:startingPage": "452",
          "prism:endingPage": "454",
          "prism:publicationDate": "1970-10",
          "dc:identifier": [{"@type": "cir:NAID", "@value": "40017960216"}]
        },
        {"title": ""}
      ]
    }"#;

    fn engine() -> Arc<Cinii> {
        Cinii::from_config(&EngineConfig::new("cinii", "cinii", "ci", &["science"])).unwrap()
    }

    fn query() -> Query {
        Query::parse(
            "マグネタイト",
            &crate::engine::EngineRegistry::default(),
            &[],
        )
    }

    #[test]
    fn parses_doi_and_naid_records() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://doi.org/10.3390/nano11040984");
                assert_eq!(
                    result.author.as_deref(),
                    Some("Tsuchida, Takayuki, Fukushima, Jun")
                );
                assert_eq!(result.published_date.as_deref(), Some("2021-04-11"));
                assert_eq!(
                    result.metadata.get("journal").map(String::as_str),
                    Some("Nanomaterials")
                );
                assert_eq!(
                    result.metadata.get("pages").map(String::as_str),
                    Some("984-984")
                );
                assert_eq!(
                    result.content,
                    "A decrease in the crystallite diameter of ferrites."
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://cir.nii.ac.jp/crid/1520853835155044608");
                assert_eq!(
                    result.metadata.get("naid").map(String::as_str),
                    Some("40017960216")
                );
                assert_eq!(result.published_date.as_deref(), Some("1970-10"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn refuses_inexact_time_ranges() {
        assert!(!engine().meta().time_range_support);
        for range in [
            crate::query::TimeRange::Day,
            crate::query::TimeRange::Week,
            crate::query::TimeRange::Month,
            crate::query::TimeRange::Year,
        ] {
            let mut query = query();
            query.time_range = Some(range);
            assert!(engine().build_request(&query, 1).is_err());
        }
    }

    #[test]
    fn pagination_is_bounded_without_overflow() {
        assert!(
            engine()
                .build_request(&query(), 0)
                .unwrap()
                .url
                .contains("start=1")
        );
        assert!(
            engine()
                .build_request(&query(), 500)
                .unwrap()
                .url
                .contains("start=9981")
        );
        for page in [501, u32::MAX] {
            assert!(engine().build_request(&query(), page).is_err());
        }
    }

    #[test]
    fn builds_a_paged_request() {
        let request = engine().build_request(&query(), 3).unwrap();
        assert!(request.url.starts_with(SEARCH_URL));
        assert!(request.url.contains("count=20"));
        assert!(request.url.contains("start=41"));
        assert!(request.url.contains("format=json"));
    }
}
