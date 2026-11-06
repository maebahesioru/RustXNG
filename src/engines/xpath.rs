//! The declarative `xpath` engine, ported from `searx/engines/xpath.py`.
//!
//! Upstream defines whole engines in `settings.yml` (`engine: xpath`) by giving
//! a search URL plus XPath expressions for the result fields. This port reads
//! the very same settings keys, so the engine definitions can be imported from
//! upstream's configuration (see `tools/import_engines.py`).

use std::sync::Arc;
use std::time::Duration;

use crate::engine::util::*;
use crate::engine::xml::{XmlDocument, looks_like_xml};
use crate::engine::xpath::{Node, XPath, XmlNode};
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// One result field: either a node-set path, or a value expression
/// (concat(...), substring-after(...), a string literal) that upstream
/// evaluates with query()[0]. searchch needs the value form for its URL.
enum Field {
    Path(XPath),
    Value(String),
}

impl Field {
    fn parse(expression: &str) -> Result<Self, String> {
        match XPath::parse(expression) {
            Ok(path) => Ok(Field::Path(path)),
            Err(error) => {
                if is_value_expression(expression) {
                    Ok(Field::Value(expression.trim().to_string()))
                } else {
                    Err(error)
                }
            }
        }
    }
}

fn is_value_expression(expression: &str) -> bool {
    let expression = expression.trim();
    if expression.starts_with("concat(")
        || expression.starts_with("substring-before(")
        || expression.starts_with("substring-after(")
    {
        return true;
    }
    matches!(expression.chars().next(), Some('\u{27}') | Some('"'))
}

/// A declaratively configured HTML engine.
pub struct XPathEngine {
    meta: EngineMeta,
    search_url: String,
    results_xpath: Option<XPath>,
    url_xpath: Option<Field>,
    title_xpath: Option<Field>,
    content_xpath: Option<Field>,
    thumbnail_xpath: Option<Field>,
    suggestion_xpath: Option<XPath>,
    /// XPath selecting a fragment URL the page loads its results from.
    followup_xpath: Option<XPath>,
    followup_headers: Vec<(String, String)>,
    url_prefix: String,
    first_page_num: u32,
    method: String,
    request_body: Option<String>,
    headers: Vec<(String, String)>,
    cookies: Vec<(String, String)>,
    time_range_url: String,
    time_range_map: Vec<(String, String)>,
    safesearch_map: Vec<(String, String)>,
}

impl XPathEngine {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let search_url = config
            .search_url
            .clone()
            .or_else(|| config.opt_str("search_url").map(str::to_string))
            .ok_or_else(|| format!("xpath engine {:?} needs a search_url", config.name))?;
        let opt_xpath = |key: &str| -> Result<Option<XPath>, String> {
            match config.opt_str(key) {
                Some(expression) if !expression.trim().is_empty() => XPath::parse(expression)
                    .map(Some)
                    .map_err(|error| format!("{:?}: {key}: {error}", config.name)),
                _ => Ok(None),
            }
        };
        let opt_field = |key: &str| -> Result<Option<Field>, String> {
            match config.opt_str(key) {
                Some(expression) if !expression.trim().is_empty() => Field::parse(expression)
                    .map(Some)
                    .map_err(|error| format!("{:?}: {key}: {error}", config.name)),
                _ => Ok(None),
            }
        };
        let first_page_num = config
            .opt_i64("first_page_num")
            .map(|value| value.max(0) as u32)
            .unwrap_or(1);
        let headers = string_pairs(config.extra.get("headers"));
        let cookies = string_pairs(config.extra.get("cookies"));
        let time_range_map = string_pairs(config.extra.get("time_range_map"));
        let safesearch_map = string_pairs(config.extra.get("safesearch_map"));
        let followup_headers = string_pairs(config.extra.get("followup_headers"));
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            search_url,
            results_xpath: opt_xpath("results_xpath")?,
            url_xpath: opt_field("url_xpath")?,
            title_xpath: opt_field("title_xpath")?,
            content_xpath: opt_field("content_xpath")?,
            thumbnail_xpath: opt_field("thumbnail_xpath")?,
            suggestion_xpath: opt_xpath("suggestion_xpath")?,
            followup_xpath: opt_xpath("followup_xpath")?,
            followup_headers,
            url_prefix: config.opt_str("url_prefix").unwrap_or_default().to_string(),
            first_page_num,
            method: config.opt_str("method").unwrap_or("GET").to_string(),
            request_body: config.opt_str("request_body").map(str::to_string),
            headers,
            cookies,
            time_range_url: config
                .opt_str("time_range_url")
                .unwrap_or_default()
                .to_string(),
            time_range_map,
            safesearch_map,
        }))
    }

    fn template(&self, query: &Query, pageno: u32) -> String {
        let language = locale_language(&query.locale);
        let pageno_value = if pageno <= 1 {
            self.first_page_num
        } else {
            (pageno - 1) + self.first_page_num
        };
        let time_range = query
            .time_range
            .map(|range| range.as_str())
            .and_then(|range| {
                self.time_range_map
                    .iter()
                    .find(|(key, _)| key == range)
                    .map(|(_, value)| value.clone())
            })
            .unwrap_or_default();
        let safe_search = self
            .safesearch_map
            .iter()
            .find(|(key, _)| key == &query.safesearch.to_string())
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        let time_range_output = if time_range.is_empty() {
            String::new()
        } else {
            self.time_range_url.replace("{time_range_val}", &time_range)
        };
        let query_encoded = encode(&query.query_text());
        self.search_url
            .replace("{query}", &query_encoded)
            .replace("{pageno}", &pageno_value.to_string())
            .replace("{lang}", &language)
            .replace("{language}", &language)
            .replace("{time_range}", &time_range_output)
            .replace("{safe_search}", &safe_search)
    }
}

impl Engine for XPathEngine {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn timeout(&self) -> Duration {
        self.meta.timeout
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let url = self.template(query, pageno);
        let mut request = if self.request_body.is_some() || self.method.eq_ignore_ascii_case("POST")
        {
            EngineRequest::post(url)
        } else {
            EngineRequest::get(url)
        };
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        for (name, value) in &self.cookies {
            request = request.cookie(name, value);
        }
        if let Some(body) = &self.request_body {
            let encoded_query = encode(&query.query_text());
            let body = body.replace("{query}", &encoded_query);
            request = request
                .header("Content-Type", "application/x-www-form-urlencoded")
                .form(
                    body.split('&')
                        .filter_map(|pair| pair.split_once('='))
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect(),
                );
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        // A 404 means the site has no page for this query (woxikon answers
        // 404 for a synonym it does not know). That is an empty result set,
        // not a broken engine, and reporting it as an HTTP error hid the
        // engine from the coverage count.
        if response.status == 404 {
            return Ok(Vec::new());
        }
        let body = response.text();
        // RSS/RDF feeds (JVN, JPCERT/CC, IPA) are XML. Running them through
        // the HTML parser loses the text of <link> — a void element in HTML —
        // which silently turned every result's URL empty and produced 0 items.
        // See `crate::engine::xml` for the diagnosis and the tree it builds.
        if looks_like_xml(&body, response.header("content-type")) {
            if let Some(document) = XmlDocument::parse(&body) {
                return Ok(self.parse_xml(&document));
            }
            // Declared XML but not well-formed: fall through to the HTML
            // parser rather than reporting a parse error, so a slightly broken
            // feed still yields whatever an HTML scrape can find.
        }
        Ok(self.parse_html(&body))
    }

    /// Some sites serve a shell page whose result list is loaded by a second
    /// request to a signed fragment URL, so the first response has nothing to
    /// parse. `followup_xpath` selects that URL from the shell; the aggregator
    /// fetches it with the remaining timeout and parses the fragment instead.
    fn followup_request(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Option<EngineRequest>, EngineError> {
        let Some(expression) = &self.followup_xpath else {
            return Ok(None);
        };
        let html = parse_html(&response.text());
        let target = expression
            .select(&html)
            .into_iter()
            .map(|node| node.text())
            .find(|text| !text.is_empty());
        let Some(target) = target else {
            return Ok(None);
        };
        let url = absolute_url(&target, &origin_of(&self.search_url)).unwrap_or(target);
        let mut request = EngineRequest::get(url);
        for (name, value) in &self.followup_headers {
            request = request.header(name, value);
        }
        Ok(Some(request))
    }
}

impl XPathEngine {
    /// The HTML path: unchanged behaviour for every CSS/XPath scraped engine.
    fn parse_html(&self, body: &str) -> Vec<EngineItem> {
        let html = parse_html(body);
        let base = origin_of(&self.search_url);
        let mut items = Vec::new();

        let containers: Vec<Node<'_>> = match &self.results_xpath {
            Some(expression) => expression.select(&html),
            None => html
                .root_element()
                .descendants()
                .filter_map(scraper::ElementRef::wrap)
                .map(Node::Element)
                .collect(),
        };

        for container in containers {
            let Node::Element(element) = container else {
                continue;
            };
            let evaluate = |field: &Option<Field>| -> Option<String> {
                match field.as_ref()? {
                    Field::Path(path) => path
                        .select_in(element)
                        .into_iter()
                        .map(|node| node.text())
                        .find(|text| !text.is_empty()),
                    Field::Value(expression) => {
                        crate::engine::xpath::evaluate_in(element, expression)
                    }
                }
            };
            let Some(url) = evaluate(&self.url_xpath).map(|url| {
                let with_prefix = format!("{}{}", self.url_prefix, url);
                absolute_url(&with_prefix, &base).unwrap_or(with_prefix)
            }) else {
                continue;
            };
            let title = evaluate(&self.title_xpath)
                .or_else(|| Some(text(element)))
                .unwrap_or_default();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let content = evaluate(&self.content_xpath).unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(thumbnail) = evaluate(&self.thumbnail_xpath) {
                if let Some(thumbnail) = absolute_url(&thumbnail, &base) {
                    builder = builder.thumbnail(thumbnail);
                }
            }
            items.push(builder.build());
        }

        if let Some(expression) = &self.suggestion_xpath {
            for node in expression.select(&html) {
                let suggestion = node.text();
                if !suggestion.is_empty() {
                    items.push(EngineItem::Suggestion(suggestion));
                }
            }
        }
        items
    }

    /// The XML path: same field extraction, XML node set instead of HTML.
    fn parse_xml(&self, document: &XmlDocument) -> Vec<EngineItem> {
        let base = origin_of(&self.search_url);
        let mut items = Vec::new();

        let containers: Vec<XmlNode<'_>> = match &self.results_xpath {
            Some(expression) => expression.select_xml(document),
            None => document.roots.iter().map(XmlNode::Element).collect(),
        };

        for container in containers {
            let XmlNode::Element(element) = container else {
                continue;
            };
            let evaluate = |field: &Option<Field>| -> Option<String> {
                match field.as_ref()? {
                    Field::Path(path) => path
                        .select_xml_in(element)
                        .into_iter()
                        .map(|node| node.text())
                        .find(|text| !text.is_empty()),
                    // Value expressions only apply to HTML results; an XML feed
                    // that asks for one simply yields no value.
                    Field::Value(_) => None,
                }
            };
            let Some(url) = evaluate(&self.url_xpath).map(|url| {
                let with_prefix = format!("{}{}", self.url_prefix, url);
                absolute_url(&with_prefix, &base).unwrap_or(with_prefix)
            }) else {
                continue;
            };
            let title = evaluate(&self.title_xpath)
                .or_else(|| Some(XmlNode::Element(element).text()))
                .unwrap_or_default();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let content = evaluate(&self.content_xpath).unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(thumbnail) = evaluate(&self.thumbnail_xpath) {
                if let Some(thumbnail) = absolute_url(&thumbnail, &base) {
                    builder = builder.thumbnail(thumbnail);
                }
            }
            items.push(builder.build());
        }

        if let Some(expression) = &self.suggestion_xpath {
            for node in expression.select_xml(document) {
                let suggestion = node.text();
                if !suggestion.is_empty() {
                    items.push(EngineItem::Suggestion(suggestion));
                }
            }
        }
        items
    }
}

/// Turn a `toml::Value` holding a table into name/value pairs.
fn string_pairs(value: Option<&toml::Value>) -> Vec<(String, String)> {
    match value {
        Some(toml::Value::Table(table)) => table
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
            .collect(),
        _ => Vec::new(),
    }
}

/// The scheme and host of an engine URL, used to resolve relative links.
fn origin_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => format!(
            "{}://{}",
            parsed.scheme(),
            parsed.host_str().unwrap_or_default()
        ),
        Err(_) => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    fn engine() -> Arc<XPathEngine> {
        let mut config = EngineConfig::new("lobsters", "xpath", "lob", &["general"]);
        config.search_url =
            Some("https://lobste.rs/search?q={query}&what=stories&order=relevance".into());
        config.extra.insert(
            "results_xpath".into(),
            toml::Value::String("//div[@class=\"story\"]".into()),
        );
        config.extra.insert(
            "title_xpath".into(),
            toml::Value::String("./a[@class=\"u-url\"]".into()),
        );
        config.extra.insert(
            "url_xpath".into(),
            toml::Value::String("./a[@class=\"u-url\"]/@href".into()),
        );
        config.extra.insert(
            "content_xpath".into(),
            toml::Value::String("./div[@class=\"story_text\"]".into()),
        );
        XPathEngine::from_config(&config).unwrap()
    }

    const FIXTURE: &str = r#"<html><body>
      <div class="story">
        <a class="u-url" href="/s/abc">Rust in the kernel</a>
        <div class="story_text">A discussion.</div>
      </div>
      <div class="story">
        <a class="u-url" href="https://example.com/x">Second story</a>
        <div class="story_text">More text.</div>
      </div>
    </body></html>"#;

    #[test]
    fn builds_the_url_from_the_template() {
        let query = Query::parse("rust async", &EngineRegistry::default(), &[]);
        let url = engine().build_request(&query, 1).unwrap().url;
        assert_eq!(
            url,
            "https://lobste.rs/search?q=rust%20async&what=stories&order=relevance"
        );
    }

    #[test]
    fn parses_the_configured_fields() {
        let response = EngineResponse::new(200, "https://lobste.rs/", FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://lobste.rs/s/abc");
                assert_eq!(result.title, "Rust in the kernel");
                assert_eq!(result.content, "A discussion.");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => assert_eq!(result.url, "https://example.com/x"),
            _ => panic!("expected a result"),
        }
    }

    /// An RSS 1.0 / RDF feed shaped like `https://jvn.jp/rss/jvn.rdf`.
    const RDF_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns="http://purl.org/rss/1.0/"
  xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
  xmlns:dc="http://purl.org/dc/elements/1.1/">
 <channel rdf:about="https://jvn.jp/">
  <title>JVN RSS Feed</title>
 </channel>
 <item rdf:about="https://jvn.jp/jp/JVN51223238/">
  <title>GROWI vulnerability</title>
  <link>https://jvn.jp/jp/JVN51223238/</link>
  <description>desc one</description>
  <dc:date>2026-10-27T15:00:00+09:00</dc:date>
 </item>
 <item rdf:about="https://jvn.jp/vu/JVNVU97400824/">
  <title>Brotli out-of-bounds write</title>
  <link>https://jvn.jp/vu/JVNVU97400824/</link>
  <description>desc two</description>
 </item>
</rdf:RDF>"#;

    /// An `xpath` engine configured for the RDF feed above.
    fn rss_engine() -> Arc<XPathEngine> {
        let mut config = EngineConfig::new("jvn-rss", "xpath", "jvn", &["general"]);
        config.search_url = Some("https://jvn.jp/rss/jvn.rdf".into());
        config
            .extra
            .insert("results_xpath".into(), toml::Value::String("//item".into()));
        config
            .extra
            .insert("url_xpath".into(), toml::Value::String("./link".into()));
        config
            .extra
            .insert("title_xpath".into(), toml::Value::String("./title".into()));
        config.extra.insert(
            "content_xpath".into(),
            toml::Value::String("./description".into()),
        );
        XPathEngine::from_config(&config).unwrap()
    }

    #[test]
    fn parses_an_rss_rdf_feed() {
        // The feed is XML, so the XML path must be taken.
        let response = EngineResponse::new(200, "https://jvn.jp/", RDF_FIXTURE.as_bytes().to_vec());
        let query = Query::parse("x", &EngineRegistry::default(), &[]);
        let items = rss_engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2, "both feed items must survive");
        match &items[0] {
            EngineItem::Result(result) => {
                // <link> keeps its text: this is exactly what the HTML parser lost.
                assert_eq!(result.url, "https://jvn.jp/jp/JVN51223238/");
                assert_eq!(result.title, "GROWI vulnerability");
                assert_eq!(result.content, "desc one");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://jvn.jp/vu/JVNVU97400824/");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn the_html_fixture_still_takes_the_html_path() {
        // Guard the regression this change could cause: a page that is not XML
        // must keep using the HTML parser and keep producing the same results.
        let response = EngineResponse::new(200, "https://lobste.rs/", FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        assert_eq!(engine().parse(&response, &query).unwrap().len(), 2);
    }

    #[test]
    fn a_declared_xml_response_that_is_not_well_formed_falls_back_to_html() {
        // A broken feed must not become a silent 0: the HTML parser still runs.
        let broken = r#"<?xml version="1.0"?><rdf:RDF><item><link>https://x/"></rdf:RDF>"#;
        let mut config = EngineConfig::new("feed", "xpath", "f", &["general"]);
        config.search_url = Some("https://example.com/feed".into());
        config
            .extra
            .insert("results_xpath".into(), toml::Value::String("//item".into()));
        config
            .extra
            .insert("url_xpath".into(), toml::Value::String(".//a/@href".into()));
        config
            .extra
            .insert("title_xpath".into(), toml::Value::String(".//a".into()));
        let engine = XPathEngine::from_config(&config).unwrap();
        let body = broken.replace("https://x/", "<a href=\"https://x/\">t</a>");
        let response = EngineResponse::new(200, "https://example.com/feed", body.into_bytes());
        let query = Query::parse("x", &EngineRegistry::default(), &[]);
        // No panic and no parse error: the HTML path handled it.
        let items = engine.parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn the_document_root_can_be_named_in_a_path() {
        use crate::engine::xml::XmlDocument;
        use crate::engine::xpath::XPath;
        let document = XmlDocument::parse(RDF_FIXTURE).unwrap();
        // The root is rdf:RDF, so //RDF//item must reach the items by local name,
        // and //rdf:RDF//item by the qualified name. Before the root was made a
        // candidate of the first step, both returned 0.
        for expression in ["//item", "//RDF//item", "//rdf:RDF//item", "/RDF//item"] {
            let items = XPath::parse(expression).unwrap().select_xml(&document);
            assert_eq!(items.len(), 2, "{expression} must reach both items");
        }
    }

    #[test]
    fn prefixed_elements_and_attributes_are_reachable() {
        use crate::engine::xml::XmlDocument;
        use crate::engine::xpath::XPath;
        let document = XmlDocument::parse(RDF_FIXTURE).unwrap();
        // dc:date by qualified name.
        let dates = XPath::parse("//dc:date").unwrap().select_xml(&document);
        assert_eq!(dates.len(), 1);
        assert_eq!(dates[0].text(), "2026-10-27T15:00:00+09:00");
        // rdf:about as an attribute predicate.
        let items = XPath::parse("//item[@rdf:about]")
            .unwrap()
            .select_xml(&document);
        assert_eq!(items.len(), 2);
    }
}
