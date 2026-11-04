//! Readable text extraction for the optional raw-content results.
//!
//! This is not a full readability implementation: it takes the largest of the
//! article/main containers, falls back to the body, drops script and style
//! text, and normalises the whitespace, which is enough for an agent to read
//! the page behind a search result.
//!
//! Block boundaries (paragraphs, headings, list items, table rows, `<br>`)
//! become line breaks, and `<pre>` blocks keep their own lines and
//! indentation, so code, configuration files and lists stay usable.

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
    let mut segments = Vec::new();
    collect_text(element, &mut segments);
    join_lines(&segments)
}

/// A piece of extracted text, before whitespace is normalised.
enum Segment {
    /// Flowing text whose whitespace collapses.
    Flow(String),
    /// A block boundary: the following text starts on a new line.
    Break,
    /// Preformatted text whose lines and indentation are kept.
    Pre(String),
}

/// Elements that start and end a line of their own.
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "section",
    "summary",
    "table",
    "tr",
    "ul",
];

/// A step of the walk: visit a node, or close the block it opened.
enum Step<N> {
    Enter(N),
    Break,
}

/// Collect every text node under the element except non-content subtrees,
/// marking block boundaries and keeping `<pre>` text verbatim.
///
/// The walk is iterative so that a deeply nested page cannot overflow the
/// stack of the process that extracts its text.
fn collect_text(element: ElementRef<'_>, out: &mut Vec<Segment>) {
    let mut stack = vec![Step::Enter(*element)];
    while let Some(step) = stack.pop() {
        let node = match step {
            Step::Break => {
                out.push(Segment::Break);
                continue;
            }
            Step::Enter(node) => node,
        };
        if let Some(text) = node.value().as_text() {
            out.push(Segment::Flow(text.to_string()));
            continue;
        }
        let Some(element) = ElementRef::wrap(node) else {
            stack.extend(node.children().rev().map(Step::Enter));
            continue;
        };
        let name = element.value().name();
        if NON_CONTENT.contains(&name) {
            continue;
        }
        match name {
            "br" => out.push(Segment::Break),
            "pre" => out.push(Segment::Pre(preformatted_text(element))),
            _ if BLOCKS.contains(&name) => {
                out.push(Segment::Break);
                stack.push(Step::Break);
                stack.extend(node.children().rev().map(Step::Enter));
            }
            _ => stack.extend(node.children().rev().map(Step::Enter)),
        }
    }
}

/// The text of a `<pre>` subtree as written, without non-content subtrees;
/// `<br>` inside it is a line break. Syntax highlighting splits code into many
/// inline elements, so text nodes are joined without separators.
fn preformatted_text(pre: ElementRef<'_>) -> String {
    let mut out = String::new();
    let mut stack = vec![*pre];
    while let Some(node) = stack.pop() {
        if let Some(text) = node.value().as_text() {
            out.push_str(text);
            continue;
        }
        match node.value().as_element().map(|element| element.name()) {
            Some(name) if NON_CONTENT.contains(&name) => {}
            Some("br") => out.push('\n'),
            _ => stack.extend(node.children().rev()),
        }
    }
    out
}

/// Join the segments into lines: flowing text is normalised line by line and
/// empty lines are dropped. Preformatted blocks keep their lines and
/// indentation, minus trailing spaces and the blank lines around them.
fn join_lines(segments: &[Segment]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let flush = |line: &mut String, lines: &mut Vec<String>| {
        let collapsed = normalise(line);
        if !collapsed.is_empty() {
            lines.push(collapsed);
        }
        line.clear();
    };
    for segment in segments {
        match segment {
            Segment::Flow(text) => {
                line.push_str(text);
                line.push(' ');
            }
            Segment::Break => flush(&mut line, &mut lines),
            Segment::Pre(text) => {
                flush(&mut line, &mut lines);
                let text = text.replace("\r\n", "\n").replace('\r', "\n");
                let pre: Vec<&str> = text.lines().map(str::trim_end).collect();
                let first = pre.iter().position(|line| !line.is_empty());
                let last = pre.iter().rposition(|line| !line.is_empty());
                if let (Some(first), Some(last)) = (first, last) {
                    lines.extend(pre[first..=last].iter().map(|line| line.to_string()));
                }
            }
        }
    }
    flush(&mut line, &mut lines);
    lines.join("\n")
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
        assert_eq!(main_text(&html), "本文\n続き");
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

    #[test]
    fn blocks_become_lines_and_inline_text_stays_on_one_line() {
        let html = Html::parse_document(
            "<html><body><article><h2>설치</h2><p>먼저 <b>tmux</b>를   설치한다.</p>\
             <ul><li>하나</li><li>둘<br>셋</li></ul>\
             <table><tr><td>키</td><td>값</td></tr><tr><td>a</td><td>1</td></tr></table>\
             </article></body></html>",
        );
        assert_eq!(
            main_text(&html),
            "설치\n먼저 tmux 를 설치한다.\n하나\n둘\n셋\n키 값\na 1"
        );
    }

    #[test]
    fn preformatted_code_keeps_lines_and_indentation() {
        let html = Html::parse_document(
            "<html><body><article><p>Put this in <code>config.yaml</code>:</p>\
             <pre><code>\n<span class=\"k\">server</span>:\n  port: <span>8080</span>\n    \n  \
             host: 0.0.0.0   \n</code></pre><p>Then restart.</p></article></body></html>",
        );
        assert_eq!(
            main_text(&html),
            "Put this in config.yaml :\nserver:\n  port: 8080\n\n  host: 0.0.0.0\nThen restart."
        );
    }

    #[test]
    fn preformatted_text_skips_scripts_and_honours_br() {
        let html = Html::parse_document(
            "<html><body><pre>a<br>  b<script>x=1</script>\r\nc</pre></body></html>",
        );
        assert_eq!(main_text(&html), "a\n  b\nc");
    }
}
