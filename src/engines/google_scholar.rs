//! Google Scholar engine, ported from searx/engines/google_scholar.py.
// engine-id: google_scholar

use std::sync::Arc;

use chrono::Datelike;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "gos",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://scholar.google.com/scholar";

pub struct GoogleScholar {
    meta: EngineMeta,
}

impl GoogleScholar {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for GoogleScholar {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let hl = if language == "all" {
            "en".to_string()
        } else {
            language.clone()
        };
        let lr = if language == "all" {
            String::new()
        } else {
            format!("lang_{language}")
        };
        let mut pairs = vec![
            ("q", query.query_text()),
            ("hl", hl),
            ("lr", lr),
            ("ie", "utf8".to_string()),
            ("oe", "utf8".to_string()),
            ("start", ((pageno - 1) * 10).to_string()),
            ("as_sdt", "2007".to_string()),
            ("as_vis", "0".to_string()),
        ];
        if query.time_range.is_some() {
            let year = chrono::Utc::now().year() - 1;
            pairs.push(("as_ylo", year.to_string()));
        }
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("Accept", "*/*")
                .cookie("CONSENT", "YES+"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
            let target = response.header("location").unwrap_or_default();
            if target.contains("/sorry/index?continue") {
                return Err(EngineError::AccessDenied);
            }
            return Err(EngineError::Message(format!("redirected to {target}")));
        }
        let html = parse_html(&response.text());
        if !select_elements(&html, "form#gs_captcha_f")?.is_empty() {
            return Err(EngineError::Captcha);
        }
        let mut items = Vec::new();
        for element in select_elements(&html, "div[data-rp]")? {
            let Some(link) = child(element, "h3 a") else {
                continue;
            };
            let title = text(link);
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, "https://scholar.google.com/") else {
                continue;
            };
            if title.is_empty() || !url.starts_with("http") {
                continue;
            }
            let document_type = text_of(element, "span.gs_ctg2").unwrap_or_default();
            let pub_type = document_type.trim_matches(['[', ']']).to_lowercase();
            let content = text_of(element, "div.gs_rs").unwrap_or_default();
            let (authors, journal, publisher, published) =
                parse_gs_a(&text_of(element, "div.gs_a").unwrap_or_default());
            let publisher = if url.contains(publisher.as_str()) {
                String::new()
            } else {
                publisher
            };
            let comments = children(element, "div.gs_fl a")
                .into_iter()
                .find(|anchor| {
                    attr(*anchor, "href").is_some_and(|href| href.starts_with("/scholar?cites="))
                })
                .map(text)
                .unwrap_or_default();
            let doc_url = attr_of(element, "div.gs_or_ggsm a", "href").unwrap_or_default();
            let (html_url, pdf_url) = if document_type == "[PDF]" {
                (String::new(), doc_url)
            } else {
                (doc_url, String::new())
            };
            items.push(
                ResultBuilder::new(url, title)
                    .content(content)
                    .author(authors.join(", "))
                    .published_date(published.unwrap_or_default())
                    .metadata("type", pub_type)
                    .metadata("journal", journal)
                    .metadata("publisher", publisher)
                    .metadata("comments", comments)
                    .metadata("html_url", html_url)
                    .metadata("pdf_url", pdf_url)
                    .build(),
            );
        }
        for anchor in select_elements(&html, "div[class*='gs_qsuggest_wrap'] li a")? {
            let value = text(anchor);
            if !value.is_empty() {
                items.push(EngineItem::Suggestion(value));
            }
        }
        for anchor in select_elements(&html, "div.gs_r.gs_pda > a")? {
            let value = text(anchor);
            if !value.is_empty() {
                items.push(EngineItem::Correction(value));
            }
        }
        Ok(items)
    }
}

/// Upstream's parse_gs_a: authors, journal, publisher and year of the green line.
fn parse_gs_a(value: &str) -> (Vec<String>, String, String, Option<String>) {
    if value.trim().is_empty() {
        return (Vec::new(), String::new(), String::new(), None);
    }
    let parts: Vec<&str> = value.split(" - ").collect();
    let authors: Vec<String> = parts[0].split(", ").map(str::to_string).collect();
    let publisher = parts[parts.len() - 1].to_string();
    if parts.len() != 3 {
        return (authors, String::new(), publisher, None);
    }
    let journal_year: Vec<&str> = parts[1].split(", ").collect();
    let mut journal = String::new();
    if journal_year.len() > 1 {
        journal = journal_year[..journal_year.len() - 1].join(", ");
    }
    let year = journal_year[journal_year.len() - 1].trim();
    let digits = year.len() == 4 && year.chars().all(|c| c.is_ascii_digit());
    (
        authors,
        journal,
        publisher,
        digits.then(|| year.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<GoogleScholar> {
        let config = EngineConfig::new("google scholar", "google_scholar", "gos", &["science"]);
        GoogleScholar::from_config(&config).unwrap()
    }

    const FIXTURE: &str = r##"<div data-rp="1"><h3 class="gs_rt"><a href="https://example.com/paper">Rust for systems</a></h3><div class="gs_a">Alice Smith, Bob Jones - Journal of Rust, 2023 - example.org</div><div class="gs_rs">A paper about <b>Rust</b> and safety.</div><div class="gs_fl"><a href="/scholar?cites=123">Cited by 7</a></div><div class="gs_or_ggsm"><a href="https://example.com/paper.pdf">PDF</a></div><span class="gs_ctg2">[PDF]</span></div>
    <div data-rp="1"><h3 class="gs_rt">[ZITATION] Something</h3></div><div class="gs_qsuggest_wrap"><ul><li><a href="/scholar?q=rust">rust language</a></li></ul></div>
    <div class="gs_r gs_pda"><a href="/scholar?q=rust">did you mean rust</a></div>"##;

    #[test]
    fn parses_the_result_list() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 3);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a paper")
        };
        assert_eq!(result.url, "https://example.com/paper");
        assert_eq!(result.title, "Rust for systems");
        assert_eq!(result.content, "A paper about Rust and safety.");
        assert_eq!(result.author.as_deref(), Some("Alice Smith, Bob Jones"));
        assert_eq!(result.published_date.as_deref(), Some("2023"));
        assert_eq!(result.metadata.get("type").map(String::as_str), Some("pdf"));
        assert_eq!(
            result.metadata.get("journal").map(String::as_str),
            Some("Journal of Rust")
        );
        assert_eq!(
            result.metadata.get("publisher").map(String::as_str),
            Some("example.org")
        );
        assert_eq!(
            result.metadata.get("comments").map(String::as_str),
            Some("Cited by 7")
        );
        assert_eq!(
            result.metadata.get("pdf_url").map(String::as_str),
            Some("https://example.com/paper.pdf")
        );
        assert!(matches!(&items[1], EngineItem::Suggestion(value) if value == "rust language"));
        assert!(matches!(&items[2], EngineItem::Correction(value) if value == "did you mean rust"));
    }

    #[test]
    fn builds_the_paged_url() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.locale = "en-US".to_string();
        query.time_range = Some(crate::query::TimeRange::Year);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://scholar.google.com/scholar?")
        );
        for part in [
            "q=rust",
            "hl=en",
            "lr=lang_en",
            "start=10",
            "as_sdt=2007",
            "as_vis=0",
        ] {
            assert!(
                request.url.contains(part),
                "missing {part} in {}",
                request.url
            );
        }
        assert!(request.url.contains("as_ylo="));
    }
}
