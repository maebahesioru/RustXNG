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

/// Collapse whitespace and drop empty lines.
fn normalise(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, max: usize) -> String {
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
