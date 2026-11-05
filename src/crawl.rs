//! Pure helpers of the crawler: URL normalisation, indexing directives and
//! canonical links. They live in the library so they can be unit tested; the
//! network and threading live in `rustxng-index`.

use scraper::{Html, Selector};
use url::Url;

/// The product token the crawler announces and robots.txt groups are matched
/// against.
pub const PRODUCT: &str = "rustxng-index";

/// The User-Agent of every crawler request: a stable, identifiable name with a
/// place to read about the bot, never a browser look-alike.
pub fn user_agent(contact: &str) -> String {
    let base = format!(
        "{PRODUCT}/{} (+https://github.com/sahenjp/RustXNG",
        env!("CARGO_PKG_VERSION")
    );
    let contact = contact.trim();
    if contact.is_empty() {
        format!("{base})")
    } else {
        format!("{base}; {contact})")
    }
}

/// Query parameters that only track visitors and never change the page.
fn is_tracking_param(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("utm_")
        || matches!(
            name.as_str(),
            "fbclid" | "gclid" | "msclkid" | "mc_cid" | "mc_eid" | "yclid" | "ref_src" | "igshid"
        )
}

/// Normalise a URL so the same page is not crawled or indexed twice: no
/// fragment, no tracking parameters, no default port, and a stable query.
pub fn normalize_url(raw: &str) -> Option<String> {
    let mut url = Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.host_str()?;
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    if url.query().is_some() {
        let kept: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(name, _)| !is_tracking_param(name))
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        if kept.is_empty() {
            url.set_query(None);
        } else {
            let mut pairs = url.query_pairs_mut();
            pairs.clear();
            pairs.extend_pairs(kept.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }
    }
    Some(url.to_string())
}

/// What a page and its headers allow the crawler to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Directives {
    pub noindex: bool,
    pub nofollow: bool,
}

impl Directives {
    fn absorb(&mut self, text: &str) {
        let lowered = text.to_ascii_lowercase();
        for token in lowered.split([',', ' ']).map(str::trim) {
            match token {
                "noindex" => self.noindex = true,
                "nofollow" => self.nofollow = true,
                "none" => {
                    self.noindex = true;
                    self.nofollow = true;
                }
                _ => {}
            }
        }
    }
}

/// Directives from an `X-Robots-Tag` header value. A value may start with a
/// user agent token (`rustxng-index: noindex`); tokens for other bots are
/// ignored.
pub fn header_directives(value: &str) -> Directives {
    let mut directives = Directives::default();
    let mut applies = true;
    for part in value.split([',', ';']) {
        let part = part.trim();
        match part.split_once(':') {
            Some((agent, rest)) if !agent.contains(char::is_whitespace) => {
                let agent = agent.trim().to_ascii_lowercase();
                if matches!(
                    agent.as_str(),
                    "max-snippet" | "max-image-preview" | "max-video-preview" | "unavailable_after"
                ) {
                    continue;
                }
                applies = agent == PRODUCT || agent == "*";
                if applies {
                    directives.absorb(rest);
                }
            }
            _ if applies => directives.absorb(part),
            _ => {}
        }
    }
    directives
}

/// Directives from `<meta name="robots">` / `<meta name="rustxng-index">`.
pub fn meta_directives(html: &Html) -> Directives {
    let mut directives = Directives::default();
    let Ok(selector) = Selector::parse("meta[name][content]") else {
        return directives;
    };
    for element in html.select(&selector) {
        let name = element
            .value()
            .attr("name")
            .unwrap_or("")
            .to_ascii_lowercase();
        if name == "robots" || name == PRODUCT {
            directives.absorb(element.value().attr("content").unwrap_or(""));
        }
    }
    directives
}

/// The same-origin `<link rel="canonical">` target, if the page names one.
///
/// A canonical on another origin is ignored: trusting it would let any page
/// redirect our index entry to a domain it does not control.
pub fn canonical_url(html: &Html, base: &Url) -> Option<String> {
    let selector = Selector::parse("link[rel][href]").ok()?;
    for element in html.select(&selector) {
        let rel = element.value().attr("rel").unwrap_or("");
        if !rel
            .split_whitespace()
            .any(|token| token.eq_ignore_ascii_case("canonical"))
        {
            continue;
        }
        let resolved = base.join(element.value().attr("href")?).ok()?;
        if resolved.origin() != base.origin() {
            return None;
        }
        return normalize_url(resolved.as_str());
    }
    None
}

/// Parse URL locations from a sitemap or sitemap index.
pub fn sitemap_locations(xml: &[u8], limit: usize) -> Result<(bool, Vec<String>), String> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_reader(xml);
    let mut index = None;
    let mut urls = Vec::new();
    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(element) if index.is_none() => {
                index = match element.local_name().as_ref() {
                    "urlset" => Some(false),
                    "sitemapindex" => Some(true),
                    _ => return Err("not a sitemap document".into()),
                };
            }
            Event::Start(element) if element.local_name().as_ref() == "loc" => {
                if urls.len() >= limit {
                    break;
                }
                let text = reader
                    .read_text(element.name())
                    .map_err(|error| error.to_string())?;
                let text = quick_xml::escape::unescape(&text).map_err(|error| error.to_string())?;
                if text.len() <= 8192 {
                    if let Some(url) = normalize_url(&text) {
                        urls.push(url);
                    }
                }
                if urls.len() >= limit {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    index
        .map(|index| (index, urls))
        .ok_or_else(|| "empty sitemap document".into())
}

/// Whether a link carries `rel="nofollow"` (or `ugc` / `sponsored`).
pub fn rel_forbids_following(rel: &str) -> bool {
    rel.split_whitespace().any(|token| {
        token.eq_ignore_ascii_case("nofollow")
            || token.eq_ignore_ascii_case("ugc")
            || token.eq_ignore_ascii_case("sponsored")
    })
}

/// A short, stable fingerprint of readable text, used to drop the same
/// content crawled under two URLs.
pub fn content_fingerprint(text: &str) -> u64 {
    // FNV-1a over the whitespace-normalised, lower-cased text.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for word in text.split_whitespace() {
        for byte in word.to_lowercase().bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash ^= 0x20;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// A snippet for result lists: the first sentences of the readable text.
pub fn snippet(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(max_chars).collect();
    // Prefer ending at a sentence or word boundary.
    let end = cut
        .rfind(['。', '.', '!', '?'])
        .filter(|position| *position > max_chars / 2)
        .map(|position| position + cut[position..].chars().next().map_or(1, char::len_utf8))
        .or_else(|| cut.rfind(' '))
        .unwrap_or(cut.len());
    format!("{}…", cut[..end].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_names_the_bot_and_never_pretends_to_be_a_browser() {
        let agent = user_agent("ops@example.org");
        assert!(agent.starts_with("rustxng-index/"));
        assert!(agent.contains("ops@example.org"));
        assert!(!agent.contains("Mozilla"));
        assert!(user_agent("").ends_with("RustXNG)"));
    }

    #[test]
    fn normalisation_drops_fragments_tracking_and_credentials() {
        assert_eq!(
            normalize_url("HTTPS://User:pw@Example.COM:443/a/b?utm_source=x&id=7&fbclid=z#top"),
            Some("https://example.com/a/b?id=7".to_string())
        );
        assert_eq!(
            normalize_url("https://example.com/p?utm_campaign=1"),
            Some("https://example.com/p".to_string())
        );
        assert_eq!(normalize_url("mailto:a@b.c"), None);
        assert_eq!(normalize_url("javascript:alert(1)"), None);
    }

    #[test]
    fn header_directives_honour_agent_prefixes() {
        assert!(header_directives("noindex, nofollow").noindex);
        assert!(header_directives("none").nofollow);
        assert!(header_directives("rustxng-index: noindex").noindex);
        assert!(!header_directives("googlebot: noindex").noindex);
        assert!(!header_directives("all").noindex);
        assert!(header_directives("googlebot: nofollow, rustxng-index: noindex").noindex);
        assert!(!header_directives("rustxng-index: noindex, googlebot: nofollow").nofollow);
        assert!(header_directives("max-snippet: 50, noindex").noindex);
    }

    #[test]
    fn meta_directives_are_read_from_robots_meta() {
        let html = Html::parse_document(
            r#"<head><meta name="robots" content="noindex,follow"></head><body>x</body>"#,
        );
        assert_eq!(
            meta_directives(&html),
            Directives {
                noindex: true,
                nofollow: false
            }
        );
        let other = Html::parse_document(r#"<meta name="googlebot" content="noindex">"#);
        assert!(!meta_directives(&other).noindex);
    }

    #[test]
    fn canonical_is_followed_only_within_the_same_host() {
        let base = Url::parse("https://example.com/a?page=2").unwrap();
        let same = Html::parse_document(r#"<link rel="canonical" href="/a">"#);
        assert_eq!(
            canonical_url(&same, &base),
            Some("https://example.com/a".to_string())
        );
        let foreign = Html::parse_document(r#"<link rel="canonical" href="https://evil.test/a">"#);
        assert_eq!(canonical_url(&foreign, &base), None);
        let port =
            Html::parse_document(r#"<link rel="canonical" href="https://example.com:8443/a">"#);
        assert_eq!(canonical_url(&port, &base), None);
    }

    #[test]
    fn sitemaps_support_namespaces_entities_and_bounded_locations() {
        let xml = br#"<sm:urlset xmlns:sm="http://www.sitemaps.org/schemas/sitemap/0.9">
            <sm:url><sm:loc>https://example.org/p?a=1&amp;b=2</sm:loc></sm:url>
            <sm:url><sm:loc>https://example.org/second</sm:loc></sm:url>
        </sm:urlset>"#;
        assert_eq!(
            sitemap_locations(xml, 1).unwrap(),
            (false, vec!["https://example.org/p?a=1&b=2".into()])
        );
        assert!(sitemap_locations(xml, 0).unwrap().1.is_empty());
        assert_eq!(sitemap_locations(xml, 10).unwrap().1.len(), 2);
        assert!(sitemap_locations(b"<html><loc>x</loc></html>", 10).is_err());
    }

    #[test]
    fn nofollow_family_of_rel_values() {
        assert!(rel_forbids_following("noopener nofollow"));
        assert!(rel_forbids_following("UGC"));
        assert!(!rel_forbids_following("noopener"));
    }

    #[test]
    fn fingerprint_ignores_case_and_whitespace_only() {
        assert_eq!(
            content_fingerprint("Hello   World"),
            content_fingerprint("hello world")
        );
        assert_ne!(
            content_fingerprint("hello world"),
            content_fingerprint("hello there")
        );
    }

    #[test]
    fn snippet_ends_on_a_boundary() {
        let text = "First sentence here. Second sentence that keeps going and going for a while.";
        let cut = snippet(text, 40);
        assert!(cut.ends_with('…'));
        assert!(cut.starts_with("First sentence here."));
        assert_eq!(snippet("short", 40), "short");
        let japanese = snippet(&"あいうえお。".repeat(30), 20);
        assert!(japanese.chars().count() <= 21);
    }
}
