//! DeStatis engine, ported from searx/engines/destatis.py.
// engine-id: destatis

use std::sync::Arc;

use scraper::node::Node;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &[],
    shortcut: "destat",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.destatis.de";
const SEARCH_PATH: &str = "/SiteGlobals/Forms/Suche/Expertensuche_Formular.html";

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The text directly inside an element, without the text of its children.
fn direct_text(root: scraper::ElementRef<'_>) -> String {
    let mut output = String::new();
    for node in root.children() {
        if let Node::Text(text) = node.value() {
            output.push_str(&text.text);
        }
    }
    clean(&output)
}

/// DeStatis, the German federal statistics search.
pub struct Destatis {
    meta: EngineMeta,
}

impl Destatis {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Destatis {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let search = self
            .meta
            .search_url
            .clone()
            .unwrap_or_else(|| format!("{base}{SEARCH_PATH}"));
        // The results are only rendered when the search form is submitted; without
        // submit=Suchen the page comes back with the form alone.
        let pairs: Vec<(&str, String)> = vec![
            ("templateQueryString", query.query_text()),
            ("submit", "Suchen".to_string()),
            ("gtp", format!("474_list%3D{}", pageno.max(1))),
        ];
        Ok(EngineRequest::get(format!(
            "{search}?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let base_slash = format!("{}/", base.trim_end_matches('/'));
        let mut results = Vec::new();
        // The result list lives inside <section id="searchresults">; the wrapper is a
        // section, not a div, and the heading keeps the anchor two levels down.
        for item in select_elements(&html, "#searchresults div.c-result")? {
            // Suggested results repeat on every page, upstream drops them after page one.
            if query.pageno > 1 {
                let recommended = attr(item, "class")
                    .map(|class| {
                        class
                            .split_whitespace()
                            .any(|part| part == "c-result--recommended")
                    })
                    .unwrap_or(false);
                if recommended {
                    continue;
                }
            }
            let Some(anchor) = child(item, "h2.c-result__heading > a").or_else(|| child(item, "a"))
            else {
                continue;
            };
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base_slash) else {
                continue;
            };
            let title = direct_text(anchor);
            if title.is_empty() || !is_http(&url) {
                continue;
            }
            let date = text_of(item, "span.c-result__date").unwrap_or_default();
            let content = text_of(item, "h2.c-result__heading + p")
                .or_else(|| text_of(item, "div.column p"))
                .unwrap_or_default();
            let doctype = text_of(item, "div.c-result__doctype p").unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title).content(content);
            if !doctype.is_empty() {
                builder = builder.metadata("doctype", doctype);
            }
            if !date.is_empty() {
                builder = builder.metadata("date", date);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<section class="l-content-wrapper" id="searchresults"><div class="row"><div class="column large-10">
      <div class="c-result">
        <div class="row">
          <div class="column small-12 medium-3">
            <div class="c-result__doctype"><p>Pressemitteilung</p></div>
          </div>
          <div class="column small-12 medium-7">
            <h2 class="c-result__heading">
              <a href="DE/Themen/Facts"><span class="c-result__date">01.02.2024</span>Bevoelkerung</a>
            </h2>
            <p>Statistik zur Bevoelkerung.</p>
          </div>
        </div>
      </div>
      <div class="c-result c-result--recommended">
        <div class="row collapse">
          <div class="column">
            <h2 class="c-result__heading">
              <div class="c-result__flag"><p>Empfehlung der Redaktion</p></div>
              <a href="DE/Themen/Other"><span class="c-result__date">02.02.2024</span>Empfohlen</a>
            </h2>
            <p>Empfohlen.</p>
          </div>
        </div>
      </div>
    </div></div></section>"##;

    fn engine() -> Arc<Destatis> {
        Destatis::from_config(&EngineConfig::new("destatis", "destatis", "destat", &[])).unwrap()
    }

    fn query(pageno: u32) -> Query {
        let mut query = Query::parse(
            "bevoelkerung",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        query.pageno = pageno;
        query
    }

    #[test]
    fn parses_the_result_blocks() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query(1)).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result");
        };
        assert_eq!(result.url, "https://www.destatis.de/DE/Themen/Facts");
        assert_eq!(result.title, "Bevoelkerung");
        assert_eq!(result.content, "Statistik zur Bevoelkerung.");
        assert_eq!(
            result.metadata.get("doctype").map(String::as_str),
            Some("Pressemitteilung")
        );
        assert_eq!(
            result.metadata.get("date").map(String::as_str),
            Some("01.02.2024")
        );
    }

    #[test]
    fn drops_recommended_results_after_page_one() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query(2)).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result");
        };
        assert_eq!(result.title, "Bevoelkerung");
    }

    #[test]
    fn builds_the_search_request() {
        let request = engine().build_request(&query(2), 2).unwrap();
        assert_eq!(
            request.url,
            "https://www.destatis.de/SiteGlobals/Forms/Suche/Expertensuche_Formular.html?templateQueryString=bevoelkerung&submit=Suchen&gtp=474_list%253D2"
        );
    }
}
