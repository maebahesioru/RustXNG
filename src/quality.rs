//! Response sanity checks for engine results.
//!
//! SearXNG issue #6358 showed that a blocked upstream can answer with HTTP 200
//! and fabricated, placeholder-like content. The parser succeeds, so the result
//! set is silently poisoned. Rather than injecting the content, treat such an
//! answer as an engine failure: the engine is suspended and the reason shows up
//! in \`unresponsive_engines\`.
//!
//! The checks are deliberately conservative. A single weak signal never trips
//! them: a response is suspicious only when several signals agree, or when it
//! carries an explicit placeholder marker.

use crate::engine::EngineItem;
use crate::query::Query;

/// Why a response looks fabricated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suspicion {
    /// Placeholder text such as "lorem ipsum" or "test search".
    Placeholder,
    /// The same title repeated across most results.
    Repeated,
    /// The response has almost nothing in common with the query.
    Unrelated,
}

impl Suspicion {
    /// The stable reason shown to clients and written to the log.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Placeholder => "placeholder content",
            Self::Repeated => "repeated results",
            Self::Unrelated => "results unrelated to the query",
        }
    }
}

/// Common markers of fabricated provider responses.
const PLACEHOLDER_MARKERS: &[&str] = &[
    "lorem ipsum",
    "dolor sit amet",
    "test search",
    "placeholder text",
    "placeholder result",
    "dummy result",
    "sample result",
    "fabricated result",
];

/// Inspect one engine's parsed results.
///
/// Returns \`Some\` when the response should be rejected. A query with fewer
/// than three results is never rejected: there are too few signals.
pub fn inspect(query: &Query, items: &[EngineItem]) -> Option<Suspicion> {
    let results: Vec<(&str, &str, &str)> = items
        .iter()
        .filter_map(|item| match item {
            EngineItem::Result(result) => Some((
                result.title.as_str(),
                result.content.as_str(),
                result.url.as_str(),
            )),
            _ => None,
        })
        .collect();
    if results.len() < 3 {
        return None;
    }

    let query_text = query.query_text().to_lowercase();
    let markers: Vec<_> = PLACEHOLDER_MARKERS
        .iter()
        .copied()
        .filter(|marker| !requested_placeholder(&query_text, marker))
        .collect();
    let mut placeholder = 0usize;
    for (title, content, _) in &results {
        let haystack = format!("{} {}", title.to_lowercase(), content.to_lowercase());
        if markers.iter().any(|marker| haystack.contains(marker)) {
            placeholder += 1;
        }
    }
    if placeholder * 2 >= results.len() {
        return Some(Suspicion::Placeholder);
    }

    let repeated = repeated_titles(&results);
    let unrelated = unrelated_share(query, &results);

    // A majority of repeated titles is a strong signal on its own.
    if repeated * 5 >= results.len() * 3 {
        return Some(Suspicion::Repeated);
    }
    // "Nothing in common with the query" alone is too weak: an engine may
    // legitimately return pages that do not repeat the query words (species
    // names, infoboxes, ...). Require repeated titles as a second signal, so a
    // normal engine is never dropped.
    if results.len() >= 5 && unrelated >= 0.9 && repeated * 5 >= results.len() {
        return Some(Suspicion::Unrelated);
    }
    None
}

fn requested_placeholder(query: &str, marker: &str) -> bool {
    let word_character = |character: char| character.is_ascii_alphanumeric() || character == '_';
    query.match_indices(marker).any(|(start, _)| {
        !query[..start].ends_with(word_character)
            && !query[start + marker.len()..].starts_with(word_character)
    })
}

/// Number of results that share a title with another result.
fn repeated_titles(results: &[(&str, &str, &str)]) -> usize {
    use std::collections::HashMap;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (title, _, _) in results {
        let normalised: String = title
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if normalised.is_empty() {
            continue;
        }
        *counts.entry(normalised).or_default() += 1;
    }
    results
        .iter()
        .filter(|(title, _, _)| {
            let normalised = title
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            counts.get(&normalised).copied().unwrap_or(0) > 1
        })
        .count()
}

/// Share of results that mention none of the query's terms.
fn unrelated_share(query: &Query, results: &[(&str, &str, &str)]) -> f64 {
    let terms = query_terms(&query.query_text());
    if terms.is_empty() {
        return 0.0;
    }
    let hits = results
        .iter()
        .filter(|(title, content, url)| {
            let haystack = format!("{} {} {}", title, content, url).to_lowercase();
            terms.iter().any(|term| haystack.contains(term))
        })
        .count();
    1.0 - (hits as f64 / results.len() as f64)
}

/// Terms of a query used for the overlap check.
///
/// Latin words of three or more characters are used as they are. Queries
/// without long Latin words (Japanese, Chinese, ...) fall back to character
/// bigrams, which catch the same "nothing in common" case.
fn query_terms(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    // Latin words of three or more characters.
    let mut terms: Vec<String> = lower
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_string)
        .collect();
    // CJK text has no spaces, so use character bigrams for it.
    let cjk: Vec<char> = lower
        .chars()
        .filter(|character| !character.is_ascii() && character.is_alphanumeric())
        .collect();
    for window in cjk.windows(2) {
        terms.push(window.iter().collect());
    }
    terms.sort();
    terms.dedup();
    terms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::util::ResultBuilder;

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    fn item(title: &str, content: &str, url: &str) -> EngineItem {
        ResultBuilder::new(url, title).content(content).build()
    }

    #[test]
    fn placeholder_marker_rejects_the_response() {
        let items: Vec<EngineItem> = (0..5)
            .map(|index| {
                item(
                    &format!("Jegdaejo test search Ru ec uluira {index}"),
                    "Gomalje uba gubkulu gug ge zi raleje rezukgu evaso",
                    &format!("http://seholuk.ci/on{index}"),
                )
            })
            .collect();
        assert_eq!(
            inspect(&query("rust"), &items),
            Some(Suspicion::Placeholder)
        );
    }

    #[test]
    fn explicitly_requested_placeholder_phrases_are_not_provider_failures() {
        let items: Vec<_> = (0..3)
            .map(|index| {
                item(
                    &format!("Lorem ipsum generator {index}"),
                    "Lorem ipsum is placeholder text used in page design.",
                    &format!("https://example.com/generator/{index}"),
                )
            })
            .collect();
        assert_eq!(
            inspect(&query("\"LOREM   IPSUM\" placeholder text"), &items),
            None
        );
        assert_eq!(
            inspect(&query("lorem ipsumとは placeholder text"), &items),
            None
        );
        assert_eq!(
            inspect(&query("lorem ipsumx placeholder text"), &items),
            Some(Suspicion::Placeholder)
        );
    }

    #[test]
    fn repeated_titles_are_rejected() {
        let items: Vec<EngineItem> = (0..6)
            .map(|index| {
                item(
                    "Same page title",
                    "content",
                    &format!("https://example.com/{index}"),
                )
            })
            .collect();
        assert_eq!(inspect(&query("rust"), &items), Some(Suspicion::Repeated));
    }

    #[test]
    fn unrelated_results_with_repeated_titles_are_rejected() {
        let items: Vec<EngineItem> = (0..6)
            .map(|index| {
                let title = if index < 2 {
                    "甜点食谱".to_string()
                } else {
                    format!("家常菜指南 {index}")
                };
                item(
                    &title,
                    &format!("奶油和糖的做法 {index}"),
                    &format!("https://example.com/{index}"),
                )
            })
            .collect();
        assert_eq!(
            inspect(&query("tokio runtime"), &items),
            Some(Suspicion::Unrelated)
        );
    }

    #[test]
    fn normal_results_pass() {
        let items: Vec<EngineItem> = (0..6)
            .map(|index| {
                item(
                    &format!("Tokio runtime guide {index}"),
                    "Tokio is an asynchronous runtime for the Rust programming language.",
                    &format!("https://tokio.rs/{index}"),
                )
            })
            .collect();
        assert_eq!(inspect(&query("tokio runtime"), &items), None);
    }

    #[test]
    fn short_responses_are_never_rejected() {
        let items = vec![item("Unrelated title", "nothing", "https://example.com/")];
        assert_eq!(inspect(&query("rust"), &items), None);
    }

    #[test]
    fn japanese_queries_use_character_bigrams() {
        assert!(query_terms("非同期ランタイム").len() >= 2);
    }
}
