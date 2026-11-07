//! Startpage engine, ported from `searx/engines/startpage.py` (web search).
//!
//! Upstream supports the categories `web`, `news` and `images` through the
//! `startpage_categ` setting; this port implements the main `web` variant and
//! reads the category from the `search_type` config key.
//!
//! Deviations from upstream (documented in the porting report):
//!
//! * Upstream scrapes a fresh `sc` code from Startpage's home page (and solves
//!   the Anubis proof-of-work challenge) before every request. The frozen
//!   `build_request` interface cannot perform those extra round trips, so the
//!   `sc` value is read from the `sc_code` config option. Without a valid value
//!   Startpage answers with a captcha, which `parse` reports as
//!   `EngineError::Captcha`.
//! * Upstream maps SearXNG locales to Startpage's language/region traits fetched
//!   from the settings page; this port uses a small built-in language table.
// engine-id: startpage

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "sp",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.startpage.com";
const SEARCH_URL: &str = "https://www.startpage.com/sp/search";

/// Startpage.
pub struct Startpage {
    meta: EngineMeta,
    categ: String,
}

impl Startpage {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config.opt_str("search_type").unwrap_or("web").to_string();
        if !matches!(categ.as_str(), "web" | "images" | "news") {
            return Err(format!("startpage: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for Startpage {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = startpage_language(&query.locale);
        let region = startpage_region(&query.locale);
        let safe_search = match query.safesearch {
            0 => "none",
            1 => "moderate",
            _ => "heavy",
        };
        let mut form: Vec<(String, String)> = vec![
            ("query".into(), query.query_text()),
            ("cat".into(), self.categ.clone()),
            ("t".into(), "device".into()),
            (
                "sc".into(),
                self.meta.opt_str("sc_code").unwrap_or_default().to_string(),
            ),
            (
                "with_date".into(),
                match query.time_range {
                    Some(TimeRange::Day) => "d",
                    Some(TimeRange::Week) => "w",
                    Some(TimeRange::Month) => "m",
                    Some(TimeRange::Year) => "y",
                    None => "",
                }
                .to_string(),
            ),
            ("abd".into(), "1".into()),
            ("abe".into(), "1".into()),
            ("qsr".into(), "all".into()),
            ("qadf".into(), safe_search.into()),
            ("language".into(), language.clone()),
            ("lui".into(), language.clone()),
            ("segment".into(), "startpage.udog".into()),
        ];
        if pageno > 1 {
            form.push(("page".into(), pageno.to_string()));
        }
        // Startpage's "preferences" cookie; entries are joined with "N1N" and the
        // key/value of one entry with "EEE".
        let cookie = [
            ("date_time", "world".to_string()),
            ("disable_family_filter", safe_search.to_string()),
            ("disable_open_in_new_window", "0".to_string()),
            ("enable_post_method", "1".to_string()),
            ("enable_proxy_safety_suggest", "1".to_string()),
            ("enable_stay_control", "1".to_string()),
            ("instant_answers", "1".to_string()),
            ("lang_homepage", "s/device/en/".to_string()),
            ("num_of_results", "10".to_string()),
            ("suggestions", "1".to_string()),
            ("wt_unit", "celsius".to_string()),
            ("language", language.clone()),
            ("language_ui", language),
            ("search_results_region", region),
        ];
        let preferences = cookie
            .iter()
            .map(|(key, value)| format!("{key}EEE{value}"))
            .collect::<Vec<_>>()
            .join("N1N");
        let mut request = EngineRequest::post(SEARCH_URL)
            .form(form)
            .header("Origin", BASE_URL)
            .header("Referer", &format!("{BASE_URL}/"))
            .cookie("preferences", &preferences);
        if let Some(auth) = self.meta.opt_str("spchal_auth") {
            request = request.cookie("spchal-auth", auth);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        if body.contains("id=\"anubis_challenge\"") {
            return Err(EngineError::Captcha);
        }
        if response
            .header("location")
            .is_some_and(|location| location.starts_with("https://www.startpage.com/sp/captcha"))
        {
            return Err(EngineError::Captcha);
        }
        let Some(raw) = embedded_json(&body, "React.createElement(UIStartpage.AppSerp") else {
            return Ok(Vec::new());
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Err(EngineError::Parse(
                "startpage: embedded result JSON is invalid".into(),
            ));
        };
        let Some(mainline) = json_path(&json, "render.presenter.regions.mainline")
            .and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for section in mainline {
            let display_type = json_str(section, "display_type");
            let Some(items) = json_path(section, "results").and_then(serde_json::Value::as_array)
            else {
                continue;
            };
            for item in items {
                if display_type == "web-google" {
                    let url = json_str(item, "clickUrl");
                    let title = strip_tags(&json_str(item, "title"));
                    if title.is_empty() || !is_http_url(&url) {
                        continue;
                    }
                    let (content, published) =
                        split_published_date(&strip_tags(&json_str(item, "description")));
                    let mut builder = ResultBuilder::new(url, title).content(content);
                    if let Some(published) = published {
                        builder = builder.published_date(published);
                    }
                    results.push(builder.build());
                } else if display_type == "news-bing" {
                    let url = json_str(item, "clickUrl");
                    let title = strip_tags(&json_str(item, "title"));
                    if title.is_empty() || !is_http_url(&url) {
                        continue;
                    }
                    let mut builder = ResultBuilder::new(url, title)
                        .content(strip_tags(&json_str(item, "description")));
                    let thumbnail = json_str(item, "thumbnailUrl");
                    if !thumbnail.is_empty() {
                        builder = builder.thumbnail(format!("{BASE_URL}{thumbnail}"));
                    }
                    if let Some(date) = json_path(item, "date").and_then(serde_json::Value::as_i64)
                    {
                        if let Some(instant) = chrono::DateTime::from_timestamp(date / 1000, 0) {
                            builder =
                                builder.published_date(instant.format("%Y-%m-%d").to_string());
                        }
                    }
                    results.push(builder.build());
                }
            }
        }
        Ok(results)
    }
}

/// Extract the object that follows `marker` from an inline `React.createElement`
/// call, mirroring upstream's `extr(...) + "}"` trick.
fn embedded_json(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let open = rest.find('{')?;
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (offset, byte) in bytes.iter().enumerate().skip(open) {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
            continue;
        }
        match byte {
            b'"' => quoted = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(rest[open..=offset].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Upstream strips a leading "`2 Sep 2014 ... `" or "`5 days ago ... `" from the
/// snippet and uses it as the publication date.
fn split_published_date(content: &str) -> (String, Option<String>) {
    let Some(index) = content.find("... ") else {
        return (content.to_string(), None);
    };
    let head = content[..index].trim_end_matches('.').trim();
    let tail = content[index + 4..].to_string();
    if head.ends_with(" ago") {
        // "5 days ago" stays as the human readable date upstream computes.
        return (tail, Some(head.to_string()));
    }
    let mut parts = head.split_whitespace();
    let (Some(day), Some(month), Some(year)) = (parts.next(), parts.next(), parts.next()) else {
        return (content.to_string(), None);
    };
    if parts.next().is_some() {
        return (content.to_string(), None);
    }
    let Ok(day) = day.parse::<u32>() else {
        return (content.to_string(), None);
    };
    let Ok(year) = year.parse::<u32>() else {
        return (content.to_string(), None);
    };
    let Some(month) = month_number(month) else {
        return (content.to_string(), None);
    };
    (tail, Some(format!("{year:04}-{month:02}-{day:02}")))
}

/// The English month abbreviations Startpage uses in its snippets.
fn month_number(month: &str) -> Option<u32> {
    let month = month.to_lowercase();
    [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|candidate| month.starts_with(candidate))
    .map(|index| index as u32 + 1)
}

/// The Startpage search language for a SearXNG locale (a small stand-in for
/// upstream's `traits.get_language`).
fn startpage_language(locale: &str) -> String {
    let language = locale_language(locale);
    match language.as_str() {
        "de" => "deutsch",
        "en" => "english",
        "es" => "espanol",
        "fr" => "francais",
        "it" => "italiano",
        "ja" => "nihongo",
        "ko" => "hangul",
        "nl" => "nederlands",
        "pl" => "polski",
        "pt" => "portugues",
        "ru" => "russkiy",
        "zh" => "fantizhengwen",
        _ => "english",
    }
    .to_string()
}

/// The Startpage region for a SearXNG locale (a small stand-in for upstream's
/// `traits.get_region`).
fn startpage_region(locale: &str) -> String {
    let region = locale_region(locale);
    match region.as_str() {
        "all" => "en_US".to_string(),
        other => other.to_uppercase(),
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        "<html><script>React.createElement(UIStartpage.AppSerpWeb, {",
        "  \"render\": {\"presenter\": {\"regions\": {\"mainline\": [",
        "    {\"display_type\": \"web-google\", \"results\": [",
        "      {\"clickUrl\": \"https://www.rust-lang.org/\", \"title\": \"Rust lang\",",
        "       \"description\": \"2 Sep 2014 ... A fast systems language.\"}]}]}}}}})",
        "</script></html>"
    );

    fn engine() -> Arc<Startpage> {
        Startpage::from_config(&EngineConfig::new(
            "startpage",
            "startpage",
            "sp",
            &["general", "web"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_web_results() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust lang");
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.content, "A fast systems language.");
                assert_eq!(result.published_date.as_deref(), Some("2014-09-02"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_post_form_and_preferences_cookie() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.locale = "de-DE".into();
        query.safesearch = 1;
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, SEARCH_URL);
        assert!(
            request
                .form
                .contains(&("query".to_string(), "rust".to_string()))
        );
        assert!(
            request
                .form
                .contains(&("cat".to_string(), "web".to_string()))
        );
        assert!(
            request
                .form
                .contains(&("page".to_string(), "2".to_string()))
        );
        assert!(
            request
                .form
                .contains(&("language".to_string(), "deutsch".to_string()))
        );
        assert!(
            request
                .cookies
                .iter()
                .any(|(name, value)| name == "preferences" && value.contains("languageEEEdeutsch"))
        );
    }

    #[test]
    fn reports_the_anubis_challenge_as_a_captcha() {
        let response = EngineResponse::new(
            200,
            SEARCH_URL,
            b"<script id=\"anubis_challenge\" type=\"application/json\">".to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine().parse(&response, &query),
            Err(EngineError::Captcha)
        ));
    }
}
