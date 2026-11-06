//! Helpers shared by the engines: HTML extraction in the spirit of upstream's
//! `xpath` engine, text cleanup and URL unwrapping.

use std::sync::OnceLock;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};

use super::{EngineError, EngineItem};
use crate::results::{Priority, SearchResult, Template};

/// Parse a document.
///
/// The <noscript> wrapper tags are stripped first. The parser runs with
/// scripting enabled, so markup inside <noscript> would otherwise collapse
/// into a single text node and stay invisible to every selector. Several
/// upstreams (Bing among them) ship the actual result list exactly there.
pub fn parse_html(body: &str) -> Html {
    if body.contains("noscript") {
        Html::parse_document(&strip_noscript_tags(body))
    } else {
        Html::parse_document(body)
    }
}

/// Drop the opening and closing <noscript> tags, keeping the content.
fn strip_noscript_tags(body: &str) -> String {
    static OPEN: OnceLock<Regex> = OnceLock::new();
    static CLOSE: OnceLock<Regex> = OnceLock::new();
    let open = OPEN.get_or_init(|| Regex::new(r"(?i)<noscript\b[^>]*>").expect("noscript regex"));
    let close = CLOSE.get_or_init(|| Regex::new(r"(?i)</noscript\s*>").expect("noscript regex"));
    let without_open = open.replace_all(body, "");
    close.replace_all(&without_open, "").into_owned()
}

/// Compile a CSS selector, mapping a bad selector to an engine error.
pub fn selector(css: &str) -> Result<Selector, EngineError> {
    Selector::parse(css)
        .map_err(|error| EngineError::Parse(format!("bad selector {css:?}: {error:?}")))
}

/// All elements matching `css`.
pub fn select_elements<'a>(html: &'a Html, css: &str) -> Result<Vec<ElementRef<'a>>, EngineError> {
    let selector = selector(css)?;
    Ok(html.select(&selector).collect())
}

/// The first element matching `css`.
pub fn select_one<'a>(html: &'a Html, css: &str) -> Result<Option<ElementRef<'a>>, EngineError> {
    let selector = selector(css)?;
    Ok(html.select(&selector).next())
}

/// The first child of `root` matching `css`.
pub fn child<'a>(root: ElementRef<'a>, css: &str) -> Option<ElementRef<'a>> {
    let selector = Selector::parse(css).ok()?;
    root.select(&selector).next()
}

/// All children of `root` matching `css`.
pub fn children<'a>(root: ElementRef<'a>, css: &str) -> Vec<ElementRef<'a>> {
    match Selector::parse(css) {
        Ok(selector) => root.select(&selector).collect(),
        Err(_) => Vec::new(),
    }
}

/// Collapse whitespace and trim.
pub fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text of `root` itself.
pub fn text(root: ElementRef<'_>) -> String {
    clean(&root.text().collect::<Vec<_>>().join(" "))
}

/// The text of the first child matching `css`.
pub fn text_of(root: ElementRef<'_>, css: &str) -> Option<String> {
    child(root, css).map(text).filter(|value| !value.is_empty())
}

/// An attribute of the first child matching `css`.
pub fn attr_of(root: ElementRef<'_>, css: &str, attribute: &str) -> Option<String> {
    child(root, css)
        .and_then(|element| element.value().attr(attribute))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// An attribute of `root`.
pub fn attr(root: ElementRef<'_>, attribute: &str) -> Option<String> {
    root.value()
        .attr(attribute)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Resolve a (possibly relative) URL against a base, unwrapping the redirect
/// wrappers used by DuckDuckGo and Bing.
pub fn absolute_url(raw: &str, base: &str) -> Option<String> {
    let raw = decode_entities(raw).trim().to_string();
    if raw.is_empty() || raw.starts_with("javascript:") {
        return None;
    }
    if let Some(index) = raw.find("uddg=") {
        let encoded = raw[index + 5..].split('&').next().unwrap_or_default();
        if let Ok(decoded) = percent_encoding::percent_decode_str(encoded).decode_utf8() {
            return Some(decoded.to_string());
        }
    }
    if raw.contains("/ck/a") {
        if let Some(index) = raw.find("u=a1") {
            let encoded = raw[index + 4..].split('&').next().unwrap_or_default();
            if let Some(decoded) = decode_base64_url(encoded) {
                return Some(decoded);
            }
        }
    }
    let base_url = url::Url::parse(base).ok()?;
    url::Url::parse(&raw)
        .or_else(|_| base_url.join(&raw))
        .ok()
        .map(|url| url.to_string())
}

fn decode_base64_url(encoded: &str) -> Option<String> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [0xffu8; 256];
    for (index, byte) in TABLE.iter().enumerate() {
        lookup[*byte as usize] = index as u8;
    }
    let bytes: Vec<u8> = encoded
        .bytes()
        .map(|byte| match byte {
            b'-' => b'+',
            b'_' => b'/',
            other => other,
        })
        .filter(|byte| *byte != b'=')
        .collect();
    let mut output = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for byte in chunk {
            let value = lookup[*byte as usize];
            if value == 0xff {
                return None;
            }
            buffer = (buffer << 6) | u32::from(value);
            bits += 6;
        }
        // 4 characters carry 3 bytes, 3 characters 2 bytes, 2 characters 1 byte.
        // A byte starts at bit `bits - 8 * (position + 1)`, counted from the top.
        let emitted = bits / 8;
        for position in 0..emitted {
            let shift = bits - 8 * (position + 1);
            output.push((buffer >> shift) as u8);
        }
    }
    String::from_utf8(output).ok()
}

/// Decode the entities that show up in URLs, titles and feeds: the XML set,
/// the common HTML ones and numeric character references.
pub fn decode_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        output.push_str(&rest[..index]);
        let tail = &rest[index..];
        if let Some(semicolon) = tail.find(';').filter(|end| *end <= 10) {
            let body = &tail[1..semicolon];
            let numeric =
                if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(decimal) = body.strip_prefix('#') {
                    decimal.parse().ok()
                } else {
                    None
                };
            if let Some(character) = numeric.and_then(char::from_u32) {
                output.push(character);
                rest = &tail[semicolon + 1..];
                continue;
            }
        }
        let (decoded, length) = if tail.starts_with("&amp;") {
            ("&", 5)
        } else if tail.starts_with("&lt;") {
            ("<", 4)
        } else if tail.starts_with("&gt;") {
            (">", 4)
        } else if tail.starts_with("&quot;") {
            ("\"", 6)
        } else if tail.starts_with("&#39;") || tail.starts_with("&apos;") {
            ("'", 5)
        } else if tail.starts_with("&nbsp;") {
            (" ", 6)
        } else {
            ("&", 1)
        };
        output.push_str(decoded);
        rest = &tail[length..];
    }
    output.push_str(rest);
    output
}

/// A convenience builder for engine results.
pub struct ResultBuilder {
    result: SearchResult,
}

impl ResultBuilder {
    pub fn new(url: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            result: SearchResult::new(url, clean(&title.into())),
        }
    }

    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.result.content = clean(&content.into());
        self
    }

    pub fn published_date(mut self, date: impl Into<String>) -> Self {
        let date = date.into();
        if !date.trim().is_empty() {
            self.result.published_date = Some(date);
        }
        self
    }

    pub fn author(mut self, author: impl Into<String>) -> Self {
        let author = author.into();
        if !author.trim().is_empty() {
            self.result.author = Some(author);
        }
        self
    }

    pub fn thumbnail(mut self, url: impl Into<String>) -> Self {
        self.result.thumbnail = Some(url.into());
        self
    }

    pub fn image(mut self, url: impl Into<String>) -> Self {
        self.result.img_src = Some(url.into());
        self
    }

    pub fn template(mut self, template: Template) -> Self {
        self.result.template = template;
        self
    }

    pub fn priority(mut self, priority: Priority) -> Self {
        self.result.priority = priority;
        self
    }

    pub fn metadata(mut self, key: &str, value: impl Into<String>) -> Self {
        let value = value.into();
        if !value.trim().is_empty() {
            self.result.metadata.insert(key.to_string(), value);
        }
        self
    }

    pub fn build(self) -> EngineItem {
        EngineItem::Result(self.result)
    }
}

/// Build a result item in one expression.
pub fn result(
    url: impl Into<String>,
    title: impl Into<String>,
    content: impl Into<String>,
) -> EngineItem {
    ResultBuilder::new(url, title).content(content).build()
}

/// Value at a JSON pointer-ish path, e.g. `json_path(&value, "data.0.title")`.
pub fn json_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for part in path.split('.') {
        current = match current {
            serde_json::Value::Object(map) => map.get(part)?,
            serde_json::Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// The string at `path`, or an empty string.
pub fn json_str(value: &serde_json::Value, path: &str) -> String {
    json_path(value, path)
        .map(|value| match value {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Number(number) => number.to_string(),
            serde_json::Value::Bool(flag) => flag.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

/// Characters that are percent-encoded in query strings: everything except the
/// RFC 3986 unreserved characters (so `max_results` stays readable).
pub const QUERY_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encode a query string value.
pub fn encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, QUERY_ENCODE_SET).to_string()
}

/// Encode key/value pairs as a query string.
pub fn query_string(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Strip the HTML tags of a snippet (engines returning HTML inside JSON).
pub fn strip_tags(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut inside = false;
    for character in value.chars() {
        match character {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => output.push(character),
            _ => {}
        }
    }
    clean(&decode_entities(&output))
}

/// The language part of a SearXNG locale ("ja-JP" -> "ja"), "all" when unset.
pub fn locale_language(locale: &str) -> String {
    let locale = locale.trim();
    if locale.is_empty() || locale == "all" || locale == "auto" {
        return "all".to_string();
    }
    locale
        .split(['-', '_'])
        .next()
        .filter(|language| {
            !language.is_empty() && language.bytes().all(|byte| byte.is_ascii_alphabetic())
        })
        .unwrap_or("all")
        .to_lowercase()
}

/// The region part of a SearXNG locale ("ja-JP" -> "jp"), "all" when unset.
pub fn locale_region(locale: &str) -> String {
    let locale = locale.trim();
    if locale.is_empty() || locale == "all" || locale == "auto" {
        return "all".to_string();
    }
    match locale.split(['-', '_']).nth(1) {
        Some(region) => region.to_lowercase(),
        None => "all".to_string(),
    }
}

/// Parse the language pair and payload used by SearXNG's online-dictionary
/// processors (for example `en-de hello` or `english-german hallo`).
/// Unknown language identifiers and queries without a payload are rejected.
pub fn online_dictionary_direction(text: &str) -> Option<(String, String, String)> {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^.*?([a-z]+)-([a-z]+) (.+)$").expect("dictionary pattern")
    });
    let captures = PATTERN.captures(text)?;
    let payload = captures[3].trim();
    if payload.is_empty() {
        return None;
    }
    let from = resolve_locale_id(&captures[1])?;
    let to = resolve_locale_id(&captures[2])?;
    Some((from, to, payload.to_string()))
}

fn resolve_locale_id(value: &str) -> Option<String> {
    crate::locales::LOCALES
        .iter()
        .find(|locale| {
            if value.len() == 2 {
                locale
                    .id
                    .get(..2)
                    .is_some_and(|id| id.eq_ignore_ascii_case(value))
            } else {
                locale.english.eq_ignore_ascii_case(value)
                    || locale.native.eq_ignore_ascii_case(value)
            }
        })
        .and_then(|locale| locale.id.get(..2))
        .map(str::to_lowercase)
}

/// One entry of an RSS 2.0 feed (Google News, Bing News, ...).
#[derive(Debug, Clone, Default)]
pub struct RssItem {
    pub title: String,
    pub link: String,
    pub description: String,
    pub pub_date: Option<String>,
    pub source: Option<String>,
    pub categories: Vec<String>,
}

/// Parse the `<item>` elements of an RSS 2.0 feed.
///
/// Upstream parses feeds with lxml/feedparser; only a structurally broken
/// document is an error here, a feed without items yields an empty list.
pub fn parse_rss(xml: &str) -> Result<Vec<RssItem>, EngineError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut items = Vec::new();
    let mut item: Option<RssItem> = None;
    let mut field: Option<&'static str> = None;
    let mut captured = String::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| EngineError::Parse(format!("invalid RSS response: {error}")))?;
        match event {
            quick_xml::events::Event::Start(start) => {
                let name = start.local_name().as_ref().to_ascii_lowercase();
                field = match name.as_slice() {
                    b"item" => {
                        item = Some(RssItem::default());
                        None
                    }
                    b"title" if item.is_some() => Some("title"),
                    b"link" if item.is_some() => Some("link"),
                    b"description" | b"summary" if item.is_some() => Some("description"),
                    b"pubdate" | b"published" | b"updated" if item.is_some() => Some("pubdate"),
                    b"source" if item.is_some() => Some("source"),
                    b"category" if item.is_some() => Some("category"),
                    _ => None,
                };
                if field.is_some() {
                    captured.clear();
                }
            }
            quick_xml::events::Event::CData(text) => {
                // CDATA sections carry raw HTML ("<br>") in many feeds.
                if field.is_some() {
                    let raw = String::from_utf8_lossy(&text.into_inner()).to_string();
                    captured.push_str(&decode_entities(&raw));
                }
            }
            quick_xml::events::Event::Text(text) => {
                if field.is_some() {
                    // Feeds use HTML entities (&nbsp;, &#8217;) that the XML
                    // unescaper refuses, so decode the raw text ourselves.
                    let raw = String::from_utf8_lossy(&text.into_inner()).to_string();
                    captured.push_str(&decode_entities(&raw));
                }
            }
            quick_xml::events::Event::End(end) => {
                if end.local_name().as_ref().eq_ignore_ascii_case(b"item") {
                    if let Some(entry) = item.take() {
                        items.push(entry);
                    }
                } else if let (Some(entry), Some(field)) = (item.as_mut(), field) {
                    let value = clean(&std::mem::take(&mut captured));
                    match field {
                        "title" => entry.title = value,
                        "link" => {
                            if entry.link.is_empty() {
                                entry.link = value;
                            }
                        }
                        "description" => entry.description = value,
                        "pubdate" => entry.pub_date = Some(value),
                        "source" => entry.source = Some(value),
                        "category" if !value.is_empty() => entry.categories.push(value),
                        _ => {}
                    }
                }
                field = None;
                captured.clear();
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(items)
}

/// Unwrap the `url=` parameter of a redirect link (Bing News apiclick).
pub fn unwrap_url_parameter(link: &str) -> String {
    let Some(index) = link.find("url=") else {
        return link.to_string();
    };
    let encoded = link[index + 4..].split('&').next().unwrap_or_default();
    match percent_encoding::percent_decode_str(encoded).decode_utf8() {
        Ok(decoded) => decoded.to_string(),
        Err(_) => link.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noscript_wrappers_are_stripped() {
        let html =
            parse_html("<html><body><noscript><div id='inner'>kept</div></noscript></body></html>");
        let found = select_elements(&html, "div#inner").expect("selector");
        assert_eq!(found.len(), 1);
        assert_eq!(text(found[0]), "kept");
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt;"), "a & b <c>");
        assert_eq!(decode_entities("a&nbsp;b"), "a b");
        assert_eq!(decode_entities("it&#8217;s &#x2764;"), "it’s ❤");
    }

    #[test]
    fn relative_urls_are_resolved() {
        assert_eq!(
            absolute_url("/path/page", "https://example.com/base/").as_deref(),
            Some("https://example.com/path/page")
        );
    }

    #[test]
    fn duckduckgo_redirects_are_unwrapped() {
        let wrapped = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fhello&rut=abc";
        assert_eq!(
            absolute_url(wrapped, "https://duckduckgo.com/").as_deref(),
            Some("https://example.com/hello")
        );
    }

    #[test]
    fn bing_redirects_are_unwrapped() {
        // The base64 payload of \`u=a1...\` is the target URL.
        let payload = "aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw";
        assert_eq!(
            decode_base64_url(payload).as_deref(),
            Some("https://rust-lang.org/")
        );
        let href = format!("https://www.bing.com/ck/a?!&&p=abc&u=a1{payload}&ntb=1");
        assert_eq!(
            absolute_url(&href, "https://www.bing.com/").as_deref(),
            Some("https://rust-lang.org/")
        );
    }

    #[test]
    fn javascript_urls_are_rejected() {
        assert!(absolute_url("javascript:alert(1)", "https://example.com").is_none());
    }

    #[test]
    fn tags_are_stripped_from_snippets() {
        assert_eq!(strip_tags("<b>bold</b> text"), "bold text");
    }

    #[test]
    fn json_path_walks_arrays() {
        let value: serde_json::Value = serde_json::json!({"a": {"b": [{"c": "d"}]}});
        assert_eq!(json_str(&value, "a.b.0.c"), "d");
    }

    #[test]
    fn query_strings_are_encoded() {
        assert_eq!(query_string(&[("q", "a b".to_string())]), "q=a%20b");
    }

    #[test]
    fn locale_language_rejects_url_syntax() {
        for (locale, expected) in [
            ("ja-JP", "ja"),
            ("EN_us", "en"),
            ("simple", "simple"),
            ("", "all"),
            ("all", "all"),
            ("auto", "all"),
            ("en/", "all"),
            ("en\\", "all"),
            ("en.", "all"),
            ("en@", "all"),
            ("en?", "all"),
            ("en#", "all"),
            ("en:", "all"),
            ("en%2f", "all"),
            ("-US", "all"),
        ] {
            assert_eq!(locale_language(locale), expected, "{locale:?}");
        }
    }

    #[test]
    fn html_extraction_helpers_work() {
        let html = parse_html("<ul><li><a href=\"/x\">Title</a><p>Body</p></li></ul>");
        let items = select_elements(&html, "li").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(text_of(items[0], "a").as_deref(), Some("Title"));
        assert_eq!(attr_of(items[0], "a", "href").as_deref(), Some("/x"));
    }

    #[test]
    fn online_dictionary_direction_parses_language_pairs() {
        assert_eq!(
            online_dictionary_direction("en-de hello"),
            Some(("en".to_string(), "de".to_string(), "hello".to_string()))
        );
        assert_eq!(
            online_dictionary_direction("english-german hallo"),
            Some(("en".to_string(), "de".to_string(), "hallo".to_string()))
        );
        assert_eq!(
            online_dictionary_direction("ja-en 東京 tower"),
            Some(("ja".to_string(), "en".to_string(), "東京 tower".to_string()))
        );
        assert_eq!(online_dictionary_direction("xx-yy hello"), None);
        assert_eq!(online_dictionary_direction("hello world"), None);
        assert_eq!(online_dictionary_direction("en-de"), None);
    }
}
