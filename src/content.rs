//! Readable text extraction for the optional raw-content results.
//!
//! This is not a full readability implementation: it takes the largest of the
//! article/main containers, falls back to the body, drops script and style
//! text, and normalises the whitespace, which is enough for an agent to read
//! the page behind a search result.

use scraper::{ElementRef, Html};

/// Longest text returned per page, in characters, unless the caller asks for
/// a different limit.
const MAX_CHARS: usize = 4_000;

/// Containers that may hold the article body.
const CONTAINERS: &str = "article, main, [role=main]";

/// Elements whose text is markup or scripting rather than readable content.
const NON_CONTENT: &[&str] = &["script", "style", "template", "noscript"];

/// The readable text of a parsed HTML document.
pub fn main_text(html: &Html) -> String {
    main_text_limited(html, MAX_CHARS)
}

/// The readable text of a parsed HTML document, capped at the given length.
///
/// The container is chosen before the cap applies, so a long article is not
/// lost to a shorter sibling that happens to come first in the document.
pub fn main_text_limited(html: &Html, max: usize) -> String {
    let text = largest_container(html)
        .or_else(|| body_text(html))
        .unwrap_or_default();
    truncate(&text, max)
}

/// The readable text of the largest article-like container that has any.
fn largest_container(html: &Html) -> Option<String> {
    let selector = scraper::Selector::parse(CONTAINERS).ok()?;
    html.select(&selector)
        .map(readable_text)
        .filter(|text| !text.is_empty())
        .max_by_key(|text| text.chars().count())
}

fn body_text(html: &Html) -> Option<String> {
    let selector = scraper::Selector::parse("body").ok()?;
    html.select(&selector)
        .map(readable_text)
        .find(|text| !text.is_empty())
}

fn readable_text(element: ElementRef<'_>) -> String {
    let mut out = String::new();
    collect_text(element, &mut out);
    normalise(&out)
}

/// Append every text node under the element except non-content subtrees.
///
/// The walk is iterative so that a deeply nested page cannot overflow the
/// stack of the process that extracts its text.
fn collect_text(element: ElementRef<'_>, out: &mut String) {
    let mut stack = vec![*element];
    while let Some(node) = stack.pop() {
        if let Some(text) = node.value().as_text() {
            out.push_str(text);
            out.push(' ');
            continue;
        }
        if node
            .value()
            .as_element()
            .is_some_and(|element| NON_CONTENT.contains(&element.name()))
        {
            continue;
        }
        stack.extend(node.children().rev());
    }
}

/// The canonical URL a page declares for itself, if any.
///
/// Reads `<link rel="canonical">` first and falls back to
/// `<meta property="og:url">`. Relative values are resolved against
/// `base_url` (the URL the document was fetched from) and fragments are
/// dropped, because a fragment never identifies a different page.
///
/// This only *extracts* the declaration. Whether it is a self-canonical
/// (the common case, and no signal either way) or a cross-URL pointer to
/// the original page is for the caller to decide.
pub fn canonical_url(html: &Html, base_url: &str) -> Option<String> {
    let base = url::Url::parse(base_url).ok();
    let resolve = |raw: &str| -> Option<String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let mut parsed = match &base {
            Some(base) => base.join(raw).ok()?,
            None => url::Url::parse(raw).ok()?,
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            return None;
        }
        parsed.set_fragment(None);
        Some(parsed.to_string())
    };
    if let Ok(selector) = scraper::Selector::parse("link[rel~=\"canonical\"]") {
        for element in html.select(&selector) {
            if let Some(resolved) = element.value().attr("href").and_then(resolve) {
                return Some(resolved);
            }
        }
    }
    if let Ok(selector) = scraper::Selector::parse("meta[property=\"og:url\"]") {
        for element in html.select(&selector) {
            if let Some(resolved) = element.value().attr("content").and_then(resolve) {
                return Some(resolved);
            }
        }
    }
    None
}

/// Collapse whitespace and drop empty lines.
pub(crate) fn normalise(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_wins_over_body() {
        let html = Html::parse_document(
            "<html><body><nav>menu</nav><article><p>Hello   world</p></article></body></html>",
        );
        assert_eq!(main_text(&html), "Hello world");
    }

    #[test]
    fn plain_body_is_the_fallback() {
        let html = Html::parse_document("<html><body><p>Just text</p></body></html>");
        assert_eq!(main_text(&html), "Just text");
    }

    fn doc(head: &str) -> Html {
        Html::parse_document(&format!(
            "<html><head>{head}</head><body><p>x</p></body></html>"
        ))
    }

    #[test]
    fn canonical_link_wins_over_og_url() {
        let html = doc(
            "<link rel=\"canonical\" href=\"https://docs.example.com/a\">\
             <meta property=\"og:url\" content=\"https://other.example.com/a\">",
        );
        assert_eq!(
            canonical_url(&html, "https://mirror.example.com/a"),
            Some("https://docs.example.com/a".to_string())
        );
    }

    #[test]
    fn canonical_relative_href_is_resolved_and_fragment_dropped() {
        let html = doc("<link rel=\"canonical\" href=\"/guide#top\">");
        assert_eq!(
            canonical_url(&html, "https://example.com/docs/guide"),
            Some("https://example.com/guide".to_string())
        );
    }

    #[test]
    fn og_url_is_the_fallback() {
        let html = doc("<meta property=\"og:url\" content=\"https://example.com/x\">");
        assert_eq!(
            canonical_url(&html, "https://example.com/y"),
            Some("https://example.com/x".to_string())
        );
    }

    #[test]
    fn self_canonical_is_returned_verbatim_for_the_caller_to_classify() {
        let html = doc("<link rel=\"canonical\" href=\"https://example.com/a\">");
        assert_eq!(
            canonical_url(&html, "https://example.com/a"),
            Some("https://example.com/a".to_string())
        );
    }

    #[test]
    fn no_canonical_declaration_returns_none() {
        let html = doc("<title>plain</title>");
        assert_eq!(canonical_url(&html, "https://example.com/a"), None);
    }

    #[test]
    fn non_http_canonical_is_rejected() {
        let html = doc("<link rel=\"canonical\" href=\"ftp://example.com/a\">");
        assert_eq!(canonical_url(&html, "https://example.com/a"), None);
    }

    #[test]
    fn long_text_is_truncated() {
        let body = "a".repeat(MAX_CHARS + 100);
        let html = Html::parse_document(&format!("<html><body><p>{body}</p></body></html>"));
        let text = main_text(&html);
        assert!(text.chars().count() <= MAX_CHARS + 1);
    }

    #[test]
    fn largest_article_wins_over_a_leading_advertisement() {
        let article = "本記事".repeat(50);
        let html = Html::parse_document(&format!(
            "<html><body><article><p>広告</p></article><article><p>{article}</p></article></body></html>"
        ));
        assert_eq!(main_text(&html), article);
    }

    #[test]
    fn script_style_template_and_noscript_are_excluded() {
        let html = Html::parse_document(
            "<html><body><article><p>本文</p><script>secret=1</script>\
             <style>.ad{color:red}</style><template><p>雛形</p></template>\
             <noscript>JSなし</noscript><p>続き</p></article></body></html>",
        );
        assert_eq!(main_text(&html), "本文 続き");
    }

    #[test]
    fn body_fallback_also_excludes_scripts() {
        let html =
            Html::parse_document("<html><body><script>secret=1</script><p>本文</p></body></html>");
        assert_eq!(main_text(&html), "本文");
    }

    #[test]
    fn limit_applies_after_picking_the_largest_article() {
        let long = "x".repeat(40);
        let html = Html::parse_document(&format!(
            "<html><body><article><p>短い</p></article><article><p>{long}</p></article></body></html>"
        ));
        assert_eq!(main_text_limited(&html, 10), "xxxxxxxxxx…");
    }
}
