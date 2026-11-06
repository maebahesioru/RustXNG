//! CSV, RSS and OpenSearch responses, ported from `searx/webutils.py` and
//! `searx/templates/opensearch_response_rss.xml`.

use axum::http::header;
use axum::response::{IntoResponse, Response};

use super::AppState;
use crate::aggregator::SearchResponse;
use crate::engine::util::encode;

/// `format=csv`: one row per result, like upstream's `write_csv_response`.
/// The exact JSON document of upstream's `get_json_response`.
///
/// Only upstream's fields are emitted, which is what a SearXNG client expects.
/// The richer response (timings, counters, engine data) is served with
/// `format=json&full=1` through [`json_response_full`].
pub fn upstream_json(response: &SearchResponse) -> serde_json::Value {
    let results: Vec<serde_json::Value> = response
        .results
        .iter()
        .map(|result| {
            let mut object = serde_json::json!({
                "url": result.url,
                "template": format!("{}.html", result.template.as_str()),
                "engine": result.engine,
                "engines": result.engines,
                "positions": result.positions,
                "score": result.score,
                "category": result.category,
                "title": result.title,
                "content": result.content,
                "img_src": result.img_src.clone().unwrap_or_default(),
                "thumbnail": result.thumbnail.clone().unwrap_or_default(),
                "author": result.author.clone().unwrap_or_default(),
                "publishedDate": result.published_date,
                "priority": match result.priority {
                    crate::results::Priority::Medium => "",
                    other => other.as_str(),
                },
                "metadata": result
                    .metadata
                    .iter()
                    .map(|(key, value)| format!("{key}: {value}"))
                    .collect::<Vec<_>>()
                    .join(" | "),
            });
            if let Some(raw) = &result.raw_content {
                object["raw_content"] = serde_json::json!(raw);
                if let Some(truncated) = result.raw_content_truncated {
                    object["raw_content_truncated"] = serde_json::json!(truncated);
                }
            }
            object
        })
        .collect();
    let answers: Vec<serde_json::Value> = response
        .answers
        .iter()
        .map(|answer| serde_json::json!({ "answer": answer }))
        .collect();
    let mut unresponsive: Vec<(String, String)> = response
        .unresponsive_engines
        .iter()
        .map(|engine| {
            let mut text = crate::engine::error_type_label(engine.error_type.as_str()).to_string();
            if engine.suspended {
                text = format!("Suspended: {text}");
            }
            (engine.engine.clone(), text)
        })
        .collect();
    unresponsive.sort();

    serde_json::json!({
        "query": response.query,
        "results": results,
        "answers": answers,
        "corrections": response.corrections,
        "infoboxes": response.infoboxes,
        "suggestions": response.suggestions,
        "unresponsive_engines": unresponsive,
    })
}

/// The rich JSON response (upstream fields plus rustxng extras) for
/// `format=json&full=1`.
pub fn json_response_full(response: &SearchResponse) -> serde_json::Value {
    let mut value = upstream_json(response);
    if let Some(object) = value.as_object_mut() {
        object.insert("search_query".into(), response.search_query.clone().into());
        object.insert(
            "number_of_results".into(),
            response.number_of_results.into(),
        );
        object.insert("pageno".into(), response.pageno.into());
        object.insert("paging".into(), response.paging.into());
        object.insert("engines".into(), serde_json::json!(response.engines));
        object.insert("timings".into(), serde_json::json!(response.timings));
        object.insert(
            "engine_data".into(),
            serde_json::json!(response.engine_data),
        );
        object.insert(
            "redirect_url".into(),
            response
                .redirect_url
                .clone()
                .map(Into::into)
                .unwrap_or(serde_json::Value::Null),
        );
        object.insert(
            "error".into(),
            response
                .error
                .clone()
                .map(Into::into)
                .unwrap_or(serde_json::Value::Null),
        );
    }
    value
}

/// The host of a result URL (upstream's `parsed_url.netloc`).
fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_default()
}

/// `format=csv`: upstream's `write_csv_response` columns and row types.
/// The column names of upstream's \`write_csv_response\`.
pub const CSV_COLUMNS: &str = "title,url,content,host,engine,score,type";

/// \`format=csv\`: upstream's \`write_csv_response\` columns and row types.
pub fn csv_response(response: &SearchResponse) -> Response {
    let mut body = format!("{CSV_COLUMNS}\n");
    for result in &response.results {
        body.push_str(&format!(
            "{},{},{},{},{},{:.6},result\n",
            csv_field(&result.title),
            csv_field(&result.url),
            csv_field(&result.content),
            csv_field(&host_of(&result.url)),
            csv_field(&result.engine),
            result.score,
        ));
    }
    for answer in &response.answers {
        body.push_str(&format!(
            "{},,,,{},0.000000,answer\n",
            csv_field(answer),
            csv_field("")
        ));
    }
    for suggestion in &response.suggestions {
        body.push_str(&format!(
            "{},,,,{},0.000000,suggestion\n",
            csv_field(suggestion),
            csv_field("")
        ));
    }
    for correction in &response.corrections {
        body.push_str(&format!(
            "{},,,,{},0.000000,correction\n",
            csv_field(correction),
            csv_field("")
        ));
    }
    let disposition = format!(
        "attachment;Filename=rustxng-{}.csv",
        response.search_query.replace(['"', '\r', '\n'], "")
    );
    (
        [
            (
                header::CONTENT_TYPE,
                "application/csv; charset=utf-8".to_string(),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        body,
    )
        .into_response()
}

fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// `format=rss`: an RSS 2.0 feed with one item per result.
pub fn rss_response(state: &AppState, response: &SearchResponse, raw_query: &str) -> Response {
    let base = state.base_url();
    let instance = &state.settings().general.instance_name;
    let search_url = format!("{base}/search?q={}", encode(raw_query));
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:opensearch=\"http://a9.com/-/spec/opensearch/1.1/\">\n<channel>\n",
    );
    xml.push_str(&format!("<title>{}</title>\n", escape(instance)));
    xml.push_str(&format!(
        "<link>{}</link>\n<description>Search results for \"{}\" - {}</description>\n",
        escape(&search_url),
        escape(raw_query),
        escape(instance)
    ));
    xml.push_str(&format!(
        "<opensearch:totalResults>{}</opensearch:totalResults>\n<opensearch:startIndex>1</opensearch:startIndex>\n<opensearch:itemsPerPage>{}</opensearch:itemsPerPage>\n",
        response.number_of_results,
        response.results.len()
    ));
    for result in &response.results {
        xml.push_str("<item>\n");
        xml.push_str(&format!("<title>{}</title>\n", escape(&result.title)));
        xml.push_str(&format!("<link>{}</link>\n", escape(&result.url)));
        xml.push_str(&format!(
            "<description>{}</description>\n",
            escape(&result.content)
        ));
        xml.push_str(&format!(
            "<guid isPermaLink=\"true\">{}</guid>\n",
            escape(&result.url)
        ));
        if let Some(date) = &result.published_date {
            xml.push_str(&format!("<pubDate>{}</pubDate>\n", escape(date)));
        }
        xml.push_str(&format!(
            "<category>{}</category>\n",
            escape(&result.category)
        ));
        xml.push_str("</item>\n");
    }
    xml.push_str("</channel>\n</rss>\n");
    ([(header::CONTENT_TYPE, "text/xml; charset=utf-8")], xml).into_response()
}

/// `/opensearch.xml`: the browser search plugin description.
pub fn opensearch_response(state: &AppState) -> Response {
    let base = state.base_url();
    let instance = &state.settings().general.instance_name;
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<OpenSearchDescription xmlns=\"http://a9.com/-/spec/opensearch/1.1/\">\n<ShortName>{short}</ShortName>\n<Description>Search {name}</Description>\n<InputEncoding>UTF-8</InputEncoding>\n<Url type=\"text/html\" method=\"GET\" template=\"{base}/search?q={{searchTerms}}\" />\n<Url type=\"application/x-suggestions+json\" method=\"GET\" template=\"{base}/autocompleter?q={{searchTerms}}\" />\n</OpenSearchDescription>\n",
        short = escape(
            &instance
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        ),
        name = escape(instance),
        base = escape(&base),
    );
    (
        [(
            header::CONTENT_TYPE,
            "application/opensearchdescription+xml",
        )],
        xml,
    )
        .into_response()
}

/// Minimal XML escaping for the feed bodies.
pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_json_forms_report_raw_content_truncation() {
        let query =
            crate::query::Query::parse("test", &crate::engine::EngineRegistry::default(), &[]);
        let mut response = crate::aggregator::SearchResponse::new(&query);
        let mut result = crate::results::SearchResult::new("https://example.org/", "source");
        result.raw_content = Some("shortened page".into());
        result.raw_content_truncated = Some(true);
        response.results.push(result);
        for value in [upstream_json(&response), json_response_full(&response)] {
            assert_eq!(value["results"][0]["raw_content_truncated"], true);
        }
        response.results[0].raw_content = None;
        assert!(
            upstream_json(&response)["results"][0]
                .get("raw_content_truncated")
                .is_none()
        );
    }

    #[test]
    fn the_upstream_json_has_only_upstream_fields() {
        use crate::aggregator::SearchResponse;
        use crate::engine::EngineRegistry;
        use crate::query::Query;
        use crate::settings::Settings;
        let registry = EngineRegistry::from_settings(&Settings::defaults()).unwrap();
        let query = Query::parse("rust", &registry, &[]);
        let mut response = SearchResponse::new(&query);
        response.results.push(crate::results::SearchResult::new(
            "https://example.com/a",
            "A",
        ));
        let value = upstream_json(&response);
        let mut keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "answers",
                "corrections",
                "infoboxes",
                "query",
                "results",
                "suggestions",
                "unresponsive_engines"
            ]
        );
        assert_eq!(value["results"][0]["template"], "default.html");
    }

    #[test]
    fn the_full_json_adds_the_counters() {
        use crate::aggregator::SearchResponse;
        use crate::engine::EngineRegistry;
        use crate::query::Query;
        use crate::settings::Settings;
        let registry = EngineRegistry::from_settings(&Settings::defaults()).unwrap();
        let query = Query::parse("rust", &registry, &[]);
        let response = SearchResponse::new(&query);
        let value = json_response_full(&response);
        assert!(value.get("timings").is_some());
        assert!(value.get("number_of_results").is_some());
    }

    #[test]
    fn csv_uses_the_upstream_columns() {
        assert_eq!(CSV_COLUMNS, "title,url,content,host,engine,score,type");
    }

    #[test]
    fn csv_fields_are_quoted() {
        assert_eq!(csv_field("a \"b\" c"), "\"a \"\"b\"\" c\"");
    }

    #[test]
    fn xml_is_escaped() {
        assert_eq!(escape("<a & b>"), "&lt;a &amp; b&gt;");
    }
}
