//! External bangs (`!!<bang>`), ported from `searx/external_bang.py`.
//!
//! Upstream ships a compressed trie with ~13.5k bangs in
//! `searx/data/external_bangs.json`. `tools/import_external_bangs.py` flattens
//! the very same data set into `src/data/external_bangs.tsv`, which is loaded
//! lazily here. Lookup and autocompletion behave like the upstream trie walk:
//! an exact bang resolves to its URL template, a prefix yields the best ranked
//! completions.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The flattened bang table, generated from upstream's trie.
const BANGS_TSV: &str = include_str!("data/external_bangs.tsv");

/// How many completions an unknown prefix may yield.
pub const AUTOCOMPLETE_LIMIT: usize = 10;

/// A bang that redirects the query to an external service.
#[derive(Debug, Clone, Copy)]
pub struct ExternalBang {
    /// The bang itself, without the leading `!!`.
    pub bang: &'static str,
    /// Human readable service name (upstream keeps this out of the trie, we use
    /// the bang itself).
    pub name: &'static str,
    /// URL template: `{query}` is replaced by the URL encoded query.
    pub url: &'static str,
    /// Ordering hint from upstream, lower is better.
    pub rank: u16,
}

struct Table {
    by_bang: HashMap<&'static str, ExternalBang>,
    bangs: Vec<ExternalBang>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut by_bang = HashMap::new();
        let mut bangs = Vec::new();
        for line in BANGS_TSV.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let mut fields = line.split('\t');
            let (Some(bang), Some(url)) = (fields.next(), fields.next()) else {
                continue;
            };
            let rank = fields
                .next()
                .and_then(|rank| rank.trim().parse::<u16>().ok())
                .unwrap_or(0);
            let entry = ExternalBang {
                bang: Box::leak(bang.to_string().into_boxed_str()),
                name: Box::leak(bang.to_string().into_boxed_str()),
                url: Box::leak(url.to_string().into_boxed_str()),
                rank,
            };
            by_bang.insert(entry.bang, entry);
            bangs.push(entry);
        }
        Table { by_bang, bangs }
    })
}

/// Number of bundled external bangs.
pub fn len() -> usize {
    table().bangs.len()
}

/// Look up an external bang by name (case insensitive).
pub fn lookup(bang: &str) -> Option<&'static ExternalBang> {
    let needle = bang.trim();
    let table = table();
    if let Some(entry) = table.by_bang.get(needle) {
        return Some(entry);
    }
    let lowered = needle.to_lowercase();
    table
        .bangs
        .iter()
        .find(|entry| entry.bang.to_lowercase() == lowered)
}

/// Bangs whose name starts with `prefix`, best ranked first (upstream sorts by
/// `-rank, bang`).
pub fn autocomplete(prefix: &str) -> Vec<&'static str> {
    let needle = prefix.trim().to_lowercase();
    if needle.is_empty() {
        return vec!["g", "ddg", "bing"];
    }
    let mut matches: Vec<&ExternalBang> = table()
        .bangs
        .iter()
        .filter(|entry| entry.bang.to_lowercase().starts_with(&needle))
        .collect();
    matches.sort_by(|left, right| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| left.bang.cmp(right.bang))
    });
    matches
        .into_iter()
        .take(AUTOCOMPLETE_LIMIT)
        .map(|entry| entry.bang)
        .collect()
}

/// Expand the `{query}` placeholder of an external bang URL.
pub fn expand_url(template: &str, query: &str) -> String {
    // Upstream keeps the main page when the query is empty.
    if query.trim().is_empty() {
        if let Ok(parsed) = url::Url::parse(template) {
            return format!(
                "{}://{}",
                parsed.scheme(),
                parsed.host_str().unwrap_or_default()
            );
        }
    }
    let encoded: String =
        percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
    template.replace("{query}", &encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtered_upstream_table_is_bundled() {
        assert!(
            len() > 12_000,
            "expected the upstream table, got {} entries",
            len()
        );
        assert_eq!(lookup("w").map(|entry| entry.bang), Some("w"));
        assert_eq!(lookup("ddg").map(|entry| entry.bang), Some("ddg"));
    }

    #[test]
    fn lookups_are_case_insensitive() {
        assert!(lookup("DDG").is_some());
        assert!(lookup("does-not-exist-9f8a").is_none());
    }

    #[test]
    fn autocomplete_prefers_ranked_matches() {
        let suggestions = autocomplete("duck");
        assert!(suggestions.contains(&"duckduckgo"), "{suggestions:?}");
        assert!(!autocomplete("").is_empty());
    }

    #[test]
    fn google_bangs_are_available() {
        for bang in [
            "g",
            "google",
            "yt",
            "youtube",
            "gtranslate",
            "ytfr",
            "invidious",
        ] {
            assert!(lookup(bang).is_some(), "{bang}");
        }
    }

    #[test]
    fn urls_are_expanded() {
        let entry = lookup("w").expect("wikipedia bang");
        let url = expand_url(entry.url, "cats & dogs");
        assert!(url.starts_with("https://en.wikipedia.org/"), "{url}");
        assert!(url.contains("cats%20%26%20dogs"), "{url}");
    }
}
