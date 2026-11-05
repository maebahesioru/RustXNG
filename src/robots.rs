//! `robots.txt` handling for the crawler, following RFC 9309.
//!
//! The rules a polite crawler must honour:
//!
//! * the group that names our product token wins over `*`;
//! * inside a group the **longest** matching pattern decides, and `Allow`
//!   beats `Disallow` when both match with the same length;
//! * `*` matches any run of characters and a trailing `$` anchors the end;
//! * a missing file (4xx) means everything is allowed, but an unreachable one
//!   (5xx, 429, network failure) means **nothing** may be fetched;
//! * `Crawl-delay` is not part of the RFC but widely published, and we honour it.

use std::time::Duration;

/// One allow or disallow pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    allow: bool,
    pattern: String,
}

/// The rules and crawl delay that apply to one product token.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Robots {
    rules: Vec<Rule>,
    delay: Option<f64>,
    sitemaps: Vec<String>,
    /// Everything is forbidden (the file could not be fetched).
    deny_all: bool,
}

/// Longest crawl delay we are willing to wait out between two requests.
const MAX_CRAWL_DELAY: f64 = 30.0;

impl Robots {
    /// Nothing is restricted (no robots.txt on the host).
    pub fn allow_all() -> Self {
        Self::default()
    }

    /// Nothing may be fetched (robots.txt exists but cannot be read).
    pub fn deny_all() -> Self {
        Self {
            deny_all: true,
            ..Self::default()
        }
    }

    /// The policy for an HTTP answer to `GET /robots.txt`.
    pub fn from_response(status: u16, body: &str, product: &str) -> Self {
        match status {
            200..=299 => Self::parse(body, product),
            429 | 500..=599 => Self::deny_all(),
            // 404, 410, 401, 403 ...: the host publishes no restrictions.
            _ => Self::allow_all(),
        }
    }

    /// Parse a robots.txt for the given product token (for example
    /// `rustxng-index`).
    pub fn parse(body: &str, product: &str) -> Self {
        let product = product.trim().to_ascii_lowercase();
        // (agents of the group, rules, delay)
        let mut groups: Vec<(Vec<String>, Vec<Rule>, Option<f64>)> = Vec::new();
        let mut sitemaps = Vec::new();
        let mut reading_agents = false;

        for raw in body.lines() {
            // BOMs and trailing comments are not part of a directive.
            let line = raw.trim_start_matches('\u{feff}');
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "user-agent" => {
                    if !reading_agents {
                        groups.push((Vec::new(), Vec::new(), None));
                    }
                    reading_agents = true;
                    if let Some(group) = groups.last_mut() {
                        group.0.push(value.to_ascii_lowercase());
                    }
                }
                "allow" | "disallow" => {
                    reading_agents = false;
                    if let Some(group) = groups.last_mut() {
                        // `Disallow:` with no value forbids nothing.
                        if !value.is_empty() {
                            group.1.push(Rule {
                                allow: key == "allow",
                                pattern: normalize_octets(value),
                            });
                        }
                    }
                }
                "crawl-delay" => {
                    reading_agents = false;
                    if let (Some(group), Ok(seconds)) = (groups.last_mut(), value.parse::<f64>()) {
                        if seconds.is_finite() && seconds >= 0.0 {
                            group.2 = Some(seconds);
                        }
                    }
                }
                "sitemap" => {
                    if value.starts_with("http") {
                        sitemaps.push(value.to_string());
                    }
                }
                _ => reading_agents = false,
            }
        }

        // The most specific agent token that matches us decides. Groups for the
        // same token are merged, as the RFC requires.
        let mut best_len = 0usize;
        let mut chosen: Vec<&(Vec<String>, Vec<Rule>, Option<f64>)> = Vec::new();
        for group in &groups {
            let specificity = group
                .0
                .iter()
                .filter(|agent| *agent != "*" && !agent.is_empty() && product.contains(*agent))
                .map(String::len)
                .max()
                .unwrap_or(0);
            if specificity == 0 {
                continue;
            }
            if specificity > best_len {
                best_len = specificity;
                chosen.clear();
            }
            if specificity == best_len {
                chosen.push(group);
            }
        }
        if chosen.is_empty() {
            chosen = groups
                .iter()
                .filter(|group| group.0.iter().any(|agent| agent == "*"))
                .collect();
        }

        let mut rules = Vec::new();
        let mut delay = None;
        for group in chosen {
            rules.extend(group.1.iter().cloned());
            delay = delay.or(group.2);
        }
        Self {
            rules,
            delay,
            sitemaps,
            deny_all: false,
        }
    }

    /// Whether the crawler may fetch this path (with its query string).
    pub fn allowed(&self, path_and_query: &str) -> bool {
        if self.deny_all {
            return false;
        }
        let target = if path_and_query.is_empty() {
            "/"
        } else {
            path_and_query
        };
        let target = normalize_octets(target);
        let mut best: Option<(usize, bool)> = None;
        for rule in &self.rules {
            if !pattern_matches(&rule.pattern, &target) {
                continue;
            }
            let length = rule.pattern.len();
            best = match best {
                None => Some((length, rule.allow)),
                Some((current, _)) if length > current => Some((length, rule.allow)),
                Some((current, allowed)) if length == current => {
                    Some((length, allowed || rule.allow))
                }
                keep => keep,
            };
        }
        best.is_none_or(|(_, allow)| allow)
    }

    /// The host's requested pause between two requests, capped.
    pub fn crawl_delay(&self) -> Option<Duration> {
        self.delay
            .map(|seconds| Duration::from_secs_f64(seconds.min(MAX_CRAWL_DELAY)))
    }

    /// `Sitemap:` lines found in the file.
    pub fn sitemaps(&self) -> &[String] {
        &self.sitemaps
    }
}

fn normalize_octets(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    let mut bytes = value.bytes().peekable();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let mut lookahead = bytes.clone();
            let escaped = lookahead
                .next()
                .and_then(|high| (high as char).to_digit(16))
                .zip(lookahead.next().and_then(|low| (low as char).to_digit(16)));
            if let Some((high, low)) = escaped {
                bytes = lookahead;
                let decoded = (high * 16 + low) as u8;
                if decoded.is_ascii_alphanumeric() || b"-._~".contains(&decoded) {
                    out.push(decoded as char);
                } else {
                    out.push('%');
                    out.push(HEX[high as usize] as char);
                    out.push(HEX[low as usize] as char);
                }
                continue;
            }
        }
        if byte.is_ascii() {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 15) as usize] as char);
        }
    }
    out
}

/// Match a robots pattern (`*` wildcard, trailing `$` anchor) against the
/// start of a path.
fn pattern_matches(pattern: &str, target: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(rest) => (rest, true),
        None => (pattern, false),
    };
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut position = 0usize;
    for (index, part) in parts.iter().enumerate() {
        if index == 0 {
            if !target.starts_with(part) {
                return false;
            }
            position = part.len();
            continue;
        }
        if part.is_empty() {
            continue;
        }
        match target[position..].find(part) {
            Some(found) => position += found + part.len(),
            None => return false,
        }
    }
    if !anchored {
        return true;
    }
    // Anchored: the last literal part has to end the path. Re-check from the
    // end because a greedy first match may have stopped too early.
    match parts.last() {
        Some(last) if parts.len() > 1 && last.is_empty() => true,
        Some(last) if parts.len() > 1 && !last.is_empty() => target.ends_with(last),
        _ => position == target.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRODUCT: &str = "rustxng-index";

    #[test]
    fn longest_match_decides_and_allow_wins_ties() {
        let robots = Robots::parse(
            "User-agent: *\nDisallow: /docs/\nAllow: /docs/public/\nDisallow: /a\nAllow: /a\n",
            PRODUCT,
        );
        assert!(!robots.allowed("/docs/secret"));
        assert!(robots.allowed("/docs/public/page"));
        assert!(robots.allowed("/a"), "equal length: Allow wins");
        assert!(robots.allowed("/elsewhere"));
    }

    #[test]
    fn our_own_group_overrides_the_wildcard_group() {
        let body = "User-agent: *\nDisallow: /\n\nUser-agent: rustxng-index\nDisallow: /private\n";
        let robots = Robots::parse(body, PRODUCT);
        assert!(robots.allowed("/page"));
        assert!(!robots.allowed("/private/x"));
    }

    #[test]
    fn consecutive_agents_share_one_group() {
        let body = "User-agent: googlebot\nUser-agent: rustxng-index\nDisallow: /nope\n";
        let robots = Robots::parse(body, PRODUCT);
        assert!(!robots.allowed("/nope"));
        assert!(robots.allowed("/yes"));
    }

    #[test]
    fn wildcards_and_end_anchor() {
        let robots = Robots::parse(
            "User-agent: *\nDisallow: /*.pdf$\nDisallow: /search?*q=\n",
            PRODUCT,
        );
        assert!(!robots.allowed("/files/a.pdf"));
        assert!(robots.allowed("/files/a.pdf.html"));
        assert!(!robots.allowed("/search?lang=en&q=x"));
        assert!(robots.allowed("/search"));
    }

    #[test]
    fn anchored_trailing_wildcards_cover_descendants() {
        let robots = Robots::parse("User-agent: *\nDisallow: /private/*$\n", PRODUCT);
        assert!(!robots.allowed("/private/"));
        assert!(!robots.allowed("/private/child/grandchild?x=1"));
        assert!(robots.allowed("/private"));
        let all = Robots::parse("User-agent: *\nDisallow: /*$\n", PRODUCT);
        assert!(!all.allowed("/anything"));
    }

    #[test]
    fn rules_compare_encoded_octets_without_decoding_reserved_characters() {
        let robots = Robots::parse(
            "User-agent: *\nDisallow: /内部/\nDisallow: /%73ecret/\nDisallow: /a%2fb\n",
            PRODUCT,
        );
        assert!(!robots.allowed("/%e5%86%85%e9%83%a8/page"));
        assert!(!robots.allowed("/内部/page"));
        assert!(!robots.allowed("/secret/page"));
        assert!(!robots.allowed("/%73ecret/page"));
        assert!(!robots.allowed("/a%2Fb"));
        assert!(robots.allowed("/a/b"));
    }

    #[test]
    fn empty_disallow_allows_everything() {
        let robots = Robots::parse("User-agent: *\nDisallow:\n", PRODUCT);
        assert!(robots.allowed("/anything"));
    }

    #[test]
    fn missing_file_allows_but_unreachable_file_denies() {
        assert!(Robots::from_response(404, "", PRODUCT).allowed("/x"));
        assert!(Robots::from_response(403, "", PRODUCT).allowed("/x"));
        assert!(!Robots::from_response(503, "", PRODUCT).allowed("/x"));
        assert!(!Robots::from_response(429, "", PRODUCT).allowed("/x"));
    }

    #[test]
    fn crawl_delay_is_read_and_capped() {
        let robots = Robots::parse("User-agent: *\nCrawl-delay: 2.5\n", PRODUCT);
        assert_eq!(robots.crawl_delay(), Some(Duration::from_millis(2500)));
        let slow = Robots::parse("User-agent: *\nCrawl-delay: 9999\n", PRODUCT);
        assert_eq!(slow.crawl_delay(), Some(Duration::from_secs(30)));
        assert_eq!(Robots::allow_all().crawl_delay(), None);
    }

    #[test]
    fn comments_bom_and_japanese_text_do_not_break_parsing() {
        let body = "\u{feff}# ロボット向け\nUser-agent: * # 全員\nDisallow: /内部/ # 非公開\n";
        let robots = Robots::parse(body, PRODUCT);
        assert!(!robots.allowed("/内部/ページ"));
        assert!(robots.allowed("/公開"));
    }

    #[test]
    fn sitemaps_are_collected() {
        let robots = Robots::parse(
            "Sitemap: https://example.com/sitemap.xml\nUser-agent: *\nDisallow:\n",
            PRODUCT,
        );
        assert_eq!(robots.sitemaps(), ["https://example.com/sitemap.xml"]);
    }
}
