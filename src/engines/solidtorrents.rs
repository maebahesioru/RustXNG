//! SolidTorrents engine, ported from searx/engines/solidtorrents.py.
// engine-id: solidtorrents

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "solid",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// SolidTorrents now redirects to Bitsearch, which serves the same index from
/// its own domain under a completely different markup.
const BASE_URL: &str = "https://bitsearch.eu";

/// The configured base without a trailing slash.
fn site_base(meta: &EngineMeta) -> &str {
    meta.base_url
        .as_deref()
        .unwrap_or(BASE_URL)
        .trim_end_matches('/')
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The number in front of a label, e.g. "1227 seeders" -> "1227".
fn count_of(cells: &[String], label: &str) -> String {
    cells
        .iter()
        .find(|cell| cell.ends_with(label))
        .map(|cell| cell.trim_end_matches(label).trim().to_string())
        .unwrap_or_default()
}

/// SolidTorrents, the HTML search page.
pub struct SolidTorrents {
    meta: EngineMeta,
}

impl SolidTorrents {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for SolidTorrents {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> =
            vec![("q", query.query_text()), ("page", pageno.to_string())];
        Ok(EngineRequest::get(format!(
            "{}/search?{}",
            site_base(&self.meta),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = site_base(&self.meta);
        let mut results = Vec::new();
        for card in select_elements(&html, "div.torrent-result-card")? {
            let Some(href) = attr_of(card, "h3 a", "href") else {
                continue;
            };
            let title = text_of(card, "h3 a").unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let Some(url) = absolute_url(&href, base) else {
                continue;
            };
            if !is_http(&url) {
                continue;
            }
            let magnet = attr_of(card, "a[href^=\"magnet:\"]", "href").unwrap_or_default();
            let torrent =
                attr_of(card, "a[href^=\"/download/torrent/\"]", "href").unwrap_or_default();
            let torrent_url = absolute_url(&torrent, base).unwrap_or_default();
            // The card carries two meta rows: category/size/date, then the swarm
            // counters. Each cell is an inline-flex wrapper around icon + value.
            let rows = children(card, "div.torrent-result-meta");
            let cells: Vec<Vec<String>> = rows
                .iter()
                .map(|row| {
                    children(*row, "span.inline-flex")
                        .into_iter()
                        .map(text)
                        .filter(|cell| !cell.is_empty())
                        .collect()
                })
                .collect();
            let head = cells.first().cloned().unwrap_or_default();
            let swarm = cells.get(1).cloned().unwrap_or_default();
            let category = head.first().cloned().unwrap_or_default();
            let filesize = head.get(1).cloned().unwrap_or_default();
            let date = head.get(2).cloned().unwrap_or_default();
            let seed = count_of(&swarm, "seeders");
            let leech = count_of(&swarm, "leechers");
            let downloads = count_of(&swarm, "downloads");
            let mut builder = ResultBuilder::new(url, title)
                .metadata("category", category)
                .metadata("seed", seed)
                .metadata("leech", leech)
                .metadata("downloads", downloads)
                .metadata("filesize", filesize)
                .metadata("magnetlink", magnet)
                .metadata("torrentfile", torrent_url);
            if let Ok(parsed) = chrono::NaiveDate::parse_from_str(date.trim(), "%m/%d/%Y") {
                builder = builder.published_date(parsed.format("%Y-%m-%dT00:00:00").to_string());
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<div class="torrent-results">
      <div class="torrent-result-card bg-white rounded-lg">
        <div class="flex items-start justify-between">
          <div class="flex-1 min-w-0">
            <div class="flex items-center space-x-2 mb-2">
              <h3 class="text-base sm:text-lg font-medium">
                <a href="/torrent/65036d54e82916e7547a82fa" class="hover:text-primary">Ubuntu 24.04</a>
              </h3>
            </div>
            <div class="torrent-result-meta flex flex-wrap items-center gap-4 text-sm mb-3">
              <span class="inline-flex items-center space-x-1"><i class="fas fa-download text-green-500"></i><span>Softwares/Windows</span></span>
              <span class="inline-flex items-center space-x-1"><i class="fas fa-download"></i><span>5.8 GB</span></span>
              <span class="inline-flex items-center space-x-1"><i class="fas fa-calendar"></i><span>10/24/2023</span></span>
            </div>
            <div class="torrent-result-meta flex flex-wrap items-center gap-4 text-sm">
              <span class="inline-flex items-center space-x-1 text-green-600"><i class="fas fa-arrow-up"></i><span class="font-medium">120</span><span>seeders</span></span>
              <span class="inline-flex items-center space-x-1 text-red-600"><i class="fas fa-arrow-down"></i><span class="font-medium">10</span><span>leechers</span></span>
              <span class="inline-flex items-center space-x-1 text-blue-600"><i class="fas fa-download"></i><span class="font-medium">5</span><span>downloads</span></span>
            </div>
          </div>
          <div class="hidden sm:flex flex-col space-y-2 ml-6">
            <a href="/download/torrent/BDBB2C28E0C568B712857BD0185D37333A7298C4" class="bg-blue-600">Torrent</a>
            <a href="magnet:?xt=urn:btih:BDBB2C28E0C568B712857BD0185D37333A7298C4&amp;dn=Ubuntu" class="bg-green-600">Magnet</a>
          </div>
        </div>
      </div>
      <div class="torrent-result-card bg-white">
        <p>not a torrent</p>
      </div>
    </div>"##;

    fn engine() -> Arc<SolidTorrents> {
        SolidTorrents::from_config(&EngineConfig::new(
            "solidtorrents",
            "solidtorrents",
            "solid",
            &["files"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_result_cards() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("ubuntu", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.url,
                    "https://bitsearch.eu/torrent/65036d54e82916e7547a82fa"
                );
                assert_eq!(result.title, "Ubuntu 24.04");
                assert_eq!(result.metadata["category"], "Softwares/Windows");
                assert_eq!(result.metadata["seed"], "120");
                assert_eq!(result.metadata["leech"], "10");
                assert_eq!(result.metadata["downloads"], "5");
                assert_eq!(result.metadata["filesize"], "5.8 GB");
                assert_eq!(
                    result.metadata["magnetlink"],
                    "magnet:?xt=urn:btih:BDBB2C28E0C568B712857BD0185D37333A7298C4&dn=Ubuntu"
                );
                assert_eq!(
                    result.metadata["torrentfile"],
                    "https://bitsearch.eu/download/torrent/BDBB2C28E0C568B712857BD0185D37333A7298C4"
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2023-10-24T00:00:00")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_search_url() {
        let query = Query::parse("ubuntu iso", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://bitsearch.eu/search?q=ubuntu%20iso&page=2"
        );
    }
}
