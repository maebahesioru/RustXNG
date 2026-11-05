//! Result collection, scoring and ordering, ported from `searx/results.py`
//! and `searx/result_types/`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::engine::EngineItem;

/// Result priority, upstream `MainResult.PriorityType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// The engine explicitly marked the result as unimportant.
    Low,
    #[default]
    Medium,
    /// Answers / direct hits, always ranked first.
    High,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Template used to render a result, upstream `result.template`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Template {
    #[default]
    Default,
    Images,
    Videos,
    Map,
    Music,
    Packages,
    /// Key/value rows from a database backed engine (upstream keyvalue.html).
    KeyValue,
}

impl Template {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Images => "images",
            Self::Videos => "videos",
            Self::Map => "map",
            Self::Music => "music",
            Self::Packages => "packages",
            Self::KeyValue => "keyvalue",
        }
    }
}

/// One search result, upstream `MainResult` / `LegacyResult`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResult {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub content: String,
    /// The engine that produced the result (first one wins).
    pub engine: String,
    /// Every engine that produced this result.
    #[serde(default)]
    pub engines: Vec<String>,
    /// 1-based positions inside each engine's result list.
    #[serde(default)]
    pub positions: Vec<u32>,
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub category: String,
    #[serde(default, serialize_with = "serialize_template")]
    pub template: Template,
    #[serde(
        default,
        skip_serializing_if = "is_medium",
        serialize_with = "serialize_priority"
    )]
    pub priority: Priority,
    #[serde(
        rename = "publishedDate",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub published_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub img_src: Option<String>,
    /// Engine specific extra fields (length, views, licence, ...).
    ///
    /// Upstream serialises these as one `"key: value | key: value"` string in
    /// the JSON API, so we do the same.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        serialize_with = "serialize_metadata"
    )]
    pub metadata: BTreeMap<String, String>,
    /// A short URL shown in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pretty_url: Option<String>,
    /// Which extra result page this came from (0 = page 1).
    #[serde(skip)]
    pub deep_page: u32,
    /// Readable page text, only when the request asked for raw content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_content: Option<String>,
    /// Whether optional raw content was cut at the configured character limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_content_truncated: Option<bool>,
    /// Whether the result belongs to the group of the previous one (UI only).
    #[serde(skip)]
    pub open_group: bool,
    #[serde(skip)]
    pub close_group: bool,
}

fn is_medium(priority: &Priority) -> bool {
    *priority == Priority::Medium
}

/// Upstream renders `default.html`, `images.html`, ... in its JSON API.
fn serialize_template<S>(template: &Template, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&format!("{}.html", template.as_str()))
}

/// Upstream keeps the engine metadata as one human readable string.
fn serialize_metadata<S>(
    metadata: &BTreeMap<String, String>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let text = metadata
        .iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join(" | ");
    serializer.serialize_str(&text)
}

/// Upstream uses an empty string for the default priority.
fn serialize_priority<S>(priority: &Priority, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(match priority {
        Priority::Medium => "",
        other => other.as_str(),
    })
}

impl SearchResult {
    pub fn new(url: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }

    pub fn with_template(mut self, template: Template) -> Self {
        self.template = template;
        self
    }

    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_metadata(mut self, key: &str, value: impl Into<String>) -> Self {
        self.metadata.insert(key.to_string(), value.into());
        self
    }
}

/// An infobox, upstream `Infobox`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Infobox {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub img_src: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<InfoboxUrl>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
    #[serde(default)]
    pub engine: String,
}

/// A link of an infobox (`urls` in the infobox template).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InfoboxUrl {
    pub title: String,
    pub url: String,
}

/// Engine timing, upstream `Timing`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timing {
    pub engine: String,
    pub total: f64,
    pub load: f64,
}

/// An engine that did not answer, upstream `UnresponsiveEngine`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UnresponsiveEngine {
    pub engine: String,
    #[serde(rename = "error_type")]
    pub error_type: String,
    pub suspended: bool,
}

/// Collects, deduplicates and scores the results of all engines.
#[derive(Debug, Default)]
pub struct ResultContainer {
    main: Vec<SearchResult>,
    index: HashMap<String, usize>,
    source_ranks: Vec<Vec<SourceRank>>,
    pub infoboxes: Vec<Infobox>,
    pub suggestions: BTreeSet<String>,
    pub answers: Vec<String>,
    pub corrections: BTreeSet<String>,
    pub engine_data: BTreeMap<String, BTreeMap<String, String>>,
    pub unresponsive_engines: Vec<UnresponsiveEngine>,
    pub timings: Vec<Timing>,
    pub paging: bool,
    pub redirect_url: Option<String>,
    closed: bool,
    ordered: Vec<SearchResult>,
}

#[derive(Debug, Clone, Copy)]
struct SourceRank {
    weight: f64,
    position: u32,
    deep_page: u32,
}

impl SourceRank {
    fn score(self, priority: Priority) -> f64 {
        let score = match priority {
            Priority::Low => 0.0,
            Priority::Medium => self.weight / f64::from(self.position),
            Priority::High => self.weight,
        };
        score * 0.5f64.powf(f64::from(self.deep_page))
    }
}

impl ResultContainer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge the results of one engine, weighting them with `weight`.
    pub fn extend(&mut self, engine_name: &str, weight: f64, items: Vec<EngineItem>) {
        self.extend_from_page(engine_name, weight, items, 0);
    }

    /// Merge one engine's results that came from a deeper result page.
    ///
    /// Deeper pages score lower because their positions are shifted by
    /// PAGE_POSITION_STRIDE per extra page, so a page-2 hit never outranks the
    /// same engine's page-1 hits.
    pub fn extend_from_page(
        &mut self,
        engine_name: &str,
        weight: f64,
        items: Vec<EngineItem>,
        page_offset: u32,
    ) {
        if self.closed {
            return;
        }
        const PAGE_POSITION_STRIDE: u32 = 100;
        let mut position = page_offset.saturating_mul(PAGE_POSITION_STRIDE);
        let deep_page = page_offset;
        for item in items {
            match item {
                EngineItem::Suggestion(suggestion) => {
                    if !suggestion.trim().is_empty() {
                        self.suggestions.insert(suggestion);
                    }
                }
                EngineItem::Answer(answer) => self.answers.push(answer),
                EngineItem::Correction(correction) => {
                    if !correction.trim().is_empty() {
                        self.corrections.insert(correction);
                    }
                }
                EngineItem::Infobox(mut infobox) => {
                    infobox.img_src = infobox.img_src.filter(|url| is_web_url(url));
                    infobox.url = infobox.url.filter(|url| is_web_url(url));
                    infobox.urls.retain(|link| is_web_url(&link.url));
                    infobox.engine = engine_name.to_string();
                    if infobox.id.is_empty() {
                        infobox.id = format!("{engine_name}-{}", self.infoboxes.len());
                    }
                    match self
                        .infoboxes
                        .iter_mut()
                        .find(|existing| existing.id == infobox.id)
                    {
                        Some(existing) => merge_infobox(existing, &infobox),
                        None => self.infoboxes.push(infobox),
                    }
                }
                EngineItem::EngineData { key, value } => {
                    self.engine_data
                        .entry(engine_name.to_string())
                        .or_default()
                        .insert(key, value);
                }
                EngineItem::Result(mut result) => {
                    result.deep_page = deep_page;
                    // Unwrap engine redirectors; drop dead wrappers outright so
                    // a redirector URL never reaches the user.
                    match unwrap_redirect(&result.url) {
                        Some(url) => result.url = url,
                        None => continue,
                    }
                    let keyvalue = result.template == Template::KeyValue
                        && result.url.starts_with("keyvalue:");
                    if !keyvalue && !is_web_url(&result.url) {
                        continue;
                    }
                    result.img_src = result.img_src.filter(|url| is_web_url(url));
                    result.thumbnail = result.thumbnail.filter(|url| is_web_url(url));
                    position = position.saturating_add(1);
                    result.positions = vec![position];
                    result.engine = engine_name.to_string();
                    result.engines = vec![engine_name.to_string()];
                    result.score = weight;
                    let rank = SourceRank {
                        weight,
                        position,
                        deep_page,
                    };
                    let key = dedup_key(&result.url);
                    match self.index.get(&key).copied() {
                        Some(existing) => {
                            let merged = &mut self.main[existing];
                            let ranks = &mut self.source_ranks[existing];
                            match merged
                                .engines
                                .iter()
                                .position(|engine| engine == engine_name)
                            {
                                Some(source) => {
                                    let previous = ranks[source];
                                    let score = rank.score(merged.priority);
                                    let previous_score = previous.score(merged.priority);
                                    if score > previous_score
                                        || (score == previous_score && position < previous.position)
                                    {
                                        ranks[source] = rank;
                                        merged.positions[source] = position;
                                    }
                                }
                                None => {
                                    ranks.push(rank);
                                    merged.positions.push(position);
                                }
                            }
                            merged.deep_page =
                                ranks.iter().map(|rank| rank.deep_page).min().unwrap_or(0);
                            merge_result(merged, &result);
                        }
                        None => {
                            self.index.insert(key, self.main.len());
                            self.main.push(result);
                            self.source_ranks.push(vec![rank]);
                        }
                    }
                }
            }
        }
    }

    /// Score and freeze the container (upstream `close`).
    pub fn close(
        &mut self,
        weight_of: impl Fn(&str) -> f64,
        query_terms: &[String],
        relevance_weight: f64,
        query_text: &str,
        authority_weight: f64,
    ) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.infoboxes
            .sort_by_key(|infobox| infobox.img_src.is_none());

        for (result, ranks) in self.main.iter_mut().zip(&self.source_ranks) {
            let mut weight = 1.0;
            for (source, engine) in result.engines.iter().enumerate() {
                weight *= ranks
                    .get(source)
                    .map(|rank| rank.weight)
                    .unwrap_or_else(|| weight_of(engine));
            }
            weight *= result.positions.len() as f64;
            let mut score = 0.0;
            for position in &result.positions {
                match result.priority {
                    Priority::Low => {}
                    Priority::High => score += weight,
                    Priority::Medium => score += weight / (*position as f64),
                }
            }
            // Blend in how well the URL + title cover the query terms. Position
            // fusion alone ranks a homepage above the page that actually answers
            // the query; this lifts the latter the way a neural ranker does.
            result.score = score
                + relevance_weight
                    * relevance(&result.url, &result.title, &result.content, query_terms);
            // Naming a site in the query means "that site's own page": lift the
            // canonical host and push mirrors down. This is the part keyword
            // and embedding ranking cannot see (official docs vs its mirror).
            // A page-2 hit is a fallback: it never outranks the same engine's
            // page-1 answer for the query.
            if result.deep_page > 0 {
                result.score *= 0.5f64.powf(f64::from(result.deep_page));
            }
            if authority_weight > 0.0 {
                let signal = authority(query_text, &result.url) - 0.5;
                result.score += authority_weight * signal;
            }
            // A Wikipedia edition or a translated page in the wrong language
            // is noise for this query; one in the query's language is wanted.
            // The lift for the query's language scales with how well the page
            // matches the query, so unrelated translated pages stay down.
            let adjustment = page_language_adjustment(query_text, &result.url);
            let adjustment = if adjustment > 0.0 {
                adjustment * relevance(&result.url, &result.title, &result.content, query_terms)
            } else {
                adjustment
            };
            let language = wikipedia_language_penalty(query_text, &result.url) + adjustment;
            if language != 0.0 {
                result.score += relevance_weight * language;
            }
        }

        self.ordered = order_results(&self.main);
    }

    /// Results sorted the way the UI shows them (upstream
    /// `get_ordered_results`).
    pub fn ordered(&self) -> &[SearchResult] {
        &self.ordered
    }

    /// Results without deduplication-group ordering, in score order.
    pub fn main_results(&self) -> &[SearchResult] {
        &self.main
    }

    /// Rough number of results (upstream reports the number of merged results).
    pub fn number_of_results(&self) -> u64 {
        self.main.len() as u64
    }

    pub fn add_unresponsive_engine(&mut self, engine: &str, error_type: &str, suspended: bool) {
        let entry = UnresponsiveEngine {
            engine: engine.to_string(),
            error_type: error_type.to_string(),
            suspended,
        };
        if !self.unresponsive_engines.contains(&entry) {
            self.unresponsive_engines.push(entry);
            self.unresponsive_engines.sort();
        }
    }

    pub fn add_timing(&mut self, engine: &str, total: f64, load: f64) {
        self.timings.push(Timing {
            engine: engine.to_string(),
            total,
            load,
        });
    }

    pub fn set_redirect_url(&mut self, url: impl Into<String>) {
        self.redirect_url = Some(url.into());
    }
}

/// Allow web links and relative Recoll paths.
fn is_web_url(value: &str) -> bool {
    match url::Url::parse(value) {
        Ok(url) => matches!(url.scheme(), "http" | "https"),
        Err(url::ParseError::RelativeUrlWithoutBase) => !value.trim().is_empty(),
        Err(_) => false,
    }
}

/// Merge `other` into `origin`, keeping the richer field of each pair.
fn merge_result(origin: &mut SearchResult, other: &SearchResult) {
    // Only upgrade to an HTTPS spelling that an engine actually returned.
    // MCP authorization still uses the exact final URL; no caller-provided
    // scheme substitution is accepted.
    if origin.url.starts_with("http://") && other.url.starts_with("https://") {
        origin.url = other.url.clone();
    }
    if !origin.engines.contains(&other.engine) {
        origin.engines.push(other.engine.clone());
    }
    if origin.title.is_empty() && !other.title.is_empty() {
        origin.title = other.title.clone();
    }
    if origin.content.len() < other.content.len() {
        origin.content = other.content.clone();
    }
    if origin.category.is_empty() && !other.category.is_empty() {
        origin.category = other.category.clone();
    }
    if origin.published_date.is_none() {
        origin.published_date = other.published_date.clone();
    }
    if origin.thumbnail.is_none() {
        origin.thumbnail = other.thumbnail.clone();
    }
    if origin.img_src.is_none() {
        origin.img_src = other.img_src.clone();
    }
    if origin.author.is_none() {
        origin.author = other.author.clone();
    }
    for (key, value) in &other.metadata {
        origin
            .metadata
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
}

fn merge_infobox(origin: &mut Infobox, other: &Infobox) {
    if origin.title.is_empty() {
        origin.title = other.title.clone();
    }
    if origin.content.is_empty() {
        origin.content = other.content.clone();
    }
    if origin.img_src.is_none() {
        origin.img_src = other.img_src.clone();
    }
    if origin.url.is_none() {
        origin.url = other.url.clone();
    }
    for url in &other.urls {
        if !origin.urls.iter().any(|existing| existing.url == url.url) {
            origin.urls.push(url.clone());
        }
    }
    for (key, value) in &other.attributes {
        origin
            .attributes
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
}

/// Group-by-category ordering, ported from `ResultContainer.get_ordered_results`.
fn order_results(results: &[SearchResult]) -> Vec<SearchResult> {
    let mut sorted: Vec<SearchResult> = results.to_vec();
    sorted.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    const MAX_COUNT: i32 = 8;
    const MAX_DISTANCE: usize = 20;

    let mut grouped: Vec<SearchResult> = Vec::with_capacity(sorted.len());
    let mut positions: HashMap<String, GroupState> = HashMap::new();

    for result in sorted {
        let category = format!(
            "{}:{}:{}",
            result.category,
            result.template.as_str(),
            if result.thumbnail.is_some() || result.img_src.is_some() {
                "img"
            } else {
                ""
            }
        );
        let group = positions.get(&category).copied();
        match group {
            Some(state)
                if state.count > 0 && grouped.len().saturating_sub(state.index) < MAX_DISTANCE =>
            {
                let index = state.index;
                grouped.insert(index, result);
                for state in positions.values_mut() {
                    if state.index >= index {
                        state.index += 1;
                    }
                }
                if let Some(state) = positions.get_mut(&category) {
                    state.count -= 1;
                }
            }
            _ => {
                grouped.push(result);
                positions.insert(
                    category,
                    GroupState {
                        index: grouped.len(),
                        count: MAX_COUNT,
                    },
                );
            }
        }
    }
    grouped
}

#[derive(Debug, Clone, Copy)]
struct GroupState {
    index: usize,
    count: i32,
}

/// Lowercase query terms (latin words of 3+, plus CJK bigrams) used by
/// [`relevance`].
pub fn query_terms(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut terms: Vec<String> = lower
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_string)
        .collect();
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

/// How many of the query terms appear in the URL + title (0.0 ..= 1.0).
///
/// A query term in the path/slug (e.g. `functions-json` for "json functions")
/// is a strong signal; the helper counts distinct terms present in the
/// lowercased URL and title.
fn relevance(url: &str, title: &str, content: &str, terms: &[String]) -> f64 {
    if terms.is_empty() {
        return 0.0;
    }
    // A bare homepage is rarely the answer to a specific query; its path has no
    // depth and it repeats none of the query in its URL. Deep pages win ties.
    let path_depth = {
        let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
        rest.split(['?', '#'])
            .next()
            .unwrap_or(rest)
            .split('/')
            .skip(1)
            .filter(|segment| !segment.is_empty())
            .count()
    };
    let depth_bonus = if path_depth == 0 { 0.0 } else { 0.35 };
    // Percent-decode so CJK paths (ja.wikipedia.org/wiki/%E5%AF%8C%E5%A3%AB...)
    // match the query's character bigrams; otherwise Japanese gold URLs score 0.
    let decoded_url = percent_encoding::percent_decode_str(url)
        .decode_utf8_lossy()
        .to_lowercase();
    let heading = format!("{decoded_url} {}", title.to_lowercase());
    let heading_hits = terms
        .iter()
        .filter(|term| heading.contains(term.as_str()))
        .count();
    let heading_coverage = heading_hits as f64 / terms.len() as f64;
    // The snippet is what the agent actually reads. A page whose text talks
    // about the query is a better answer than a page that only mentions it in
    // the URL, so blend the body coverage in at a lower weight.
    let body: String = content.chars().take(600).collect::<String>().to_lowercase();
    let body_hits = terms
        .iter()
        .filter(|term| body.contains(term.as_str()))
        .count();
    let body_coverage = body_hits as f64 / terms.len() as f64;
    // An identifier in the query (rfc 9110, oauth 6749, python 3.12) is the most
    // discriminating token there is: when the URL carries it, the page really is
    // the one being asked for.
    let id_bonus = if terms
        .iter()
        .filter(|term| term.chars().any(|c| c.is_ascii_digit()) && term.len() >= 3)
        .any(|term| decoded_url.contains(term.as_str()))
    {
        0.25
    } else {
        0.0
    };
    (0.62 * heading_coverage + 0.30 * body_coverage + depth_bonus + id_bonus).min(1.0)
}

/// A brand/site the user names in the query, with its canonical hosts.
struct Authority {
    /// Query substrings that name the site (lowercase).
    names: &'static [&'static str],
    /// Hosts that are the real, canonical source.
    canonical: &'static [&'static str],
}

/// Sites people name explicitly in a search query. Naming one is a request for
/// that site's own documentation, not for a mirror or a tutorial about it.
///
/// The list stays small and conservative: each entry is a site whose name is
/// unambiguous enough to act on.
const AUTHORITIES: &[Authority] = &[
    Authority {
        names: &["arxiv"],
        canonical: &["arxiv.org"],
    },
    Authority {
        names: &["mdn"],
        canonical: &["developer.mozilla.org"],
    },
    Authority {
        names: &["python", "python3"],
        canonical: &["docs.python.org", "python.org"],
    },
    Authority {
        names: &["python japan"],
        canonical: &["python.jp", "python.org"],
    },
    Authority {
        names: &["rust", "rustlang"],
        canonical: &["doc.rust-lang.org", "rust-lang.org"],
    },
    Authority {
        names: &["rust japan", "rust-jp"],
        canonical: &["rust-jp.rs", "doc.rust-lang.org"],
    },
    Authority {
        names: &["postgres", "postgresql"],
        canonical: &["postgresql.org"],
    },
    Authority {
        names: &["docker"],
        canonical: &["docs.docker.com", "docker.com"],
    },
    Authority {
        names: &["docker hub", "dockerhub"],
        canonical: &["hub.docker.com"],
    },
    Authority {
        names: &["linux", "kernel"],
        canonical: &["docs.kernel.org", "kernel.org"],
    },
    Authority {
        names: &["numpy"],
        canonical: &["numpy.org"],
    },
    Authority {
        names: &["react"],
        canonical: &["react.dev", "reactjs.org", "ja.react.dev"],
    },
    Authority {
        names: &["npm"],
        canonical: &["npmjs.com"],
    },
    Authority {
        names: &["crates", "crates.io"],
        canonical: &["crates.io", "docs.rs"],
    },
    Authority {
        names: &["wikipedia"],
        canonical: &["wikipedia.org"],
    },
    Authority {
        names: &["kubernetes", "k8s"],
        canonical: &["kubernetes.io"],
    },
    Authority {
        names: &["git"],
        canonical: &["git-scm.com"],
    },
    Authority {
        names: &["nginx"],
        canonical: &["nginx.org", "docs.nginx.com"],
    },
    Authority {
        names: &["fastapi"],
        canonical: &["fastapi.tiangolo.com"],
    },
    Authority {
        names: &["django"],
        canonical: &["docs.djangoproject.com", "djangoproject.com"],
    },
    Authority {
        names: &["flask"],
        canonical: &["flask.palletsprojects.com"],
    },
    Authority {
        names: &["rfc"],
        canonical: &["rfc-editor.org", "ietf.org"],
    },
    Authority {
        names: &["json schema"],
        canonical: &["json-schema.org"],
    },
    Authority {
        names: &["oauth"],
        canonical: &["rfc-editor.org", "oauth.net"],
    },
    Authority {
        names: &["man page", "man7"],
        canonical: &["man7.org", "linux.die.net"],
    },
    Authority {
        names: &["tokio"],
        canonical: &["tokio.rs"],
    },
    Authority {
        names: &["serde"],
        canonical: &["serde.rs", "crates.io", "docs.rs"],
    },
    Authority {
        names: &["sqlite"],
        canonical: &["sqlite.org"],
    },
    Authority {
        names: &["redis"],
        canonical: &["redis.io"],
    },
    Authority {
        names: &["golang"],
        canonical: &["go.dev", "pkg.go.dev"],
    },
    Authority {
        names: &["nodejs", "node.js"],
        canonical: &["nodejs.org"],
    },
    Authority {
        names: &["typescript"],
        canonical: &["typescriptlang.org"],
    },
    Authority {
        names: &["tailwind"],
        canonical: &["tailwindcss.com"],
    },
    Authority {
        names: &["vite"],
        canonical: &["vitejs.dev", "vite.dev"],
    },
    Authority {
        names: &["pytorch"],
        canonical: &["pytorch.org"],
    },
    Authority {
        names: &["tensorflow"],
        canonical: &["tensorflow.org"],
    },
    Authority {
        names: &["huggingface", "hugging face"],
        canonical: &["huggingface.co"],
    },
    Authority {
        names: &["aws", "ec2"],
        canonical: &["docs.aws.amazon.com", "aws.amazon.com"],
    },
    Authority {
        names: &["google cloud", "gcp"],
        canonical: &["cloud.google.com"],
    },
    Authority {
        names: &["cppreference"],
        canonical: &["en.cppreference.com", "cppreference.com"],
    },
    Authority {
        names: &["arch wiki", "archlinux"],
        canonical: &["wiki.archlinux.org", "archlinux.org"],
    },
    Authority {
        names: &["whatwg"],
        canonical: &["whatwg.org"],
    },
    Authority {
        names: &["jquery"],
        canonical: &["jquery.com"],
    },
    Authority {
        names: &["気象庁"],
        canonical: &["jma.go.jp"],
    },
    Authority {
        names: &["国立国会図書館"],
        canonical: &["ndlsearch.ndl.go.jp", "ndl.go.jp"],
    },
    Authority {
        names: &["gotokyo"],
        canonical: &["gotokyo.org"],
    },
    Authority {
        names: &["jr東日本", "jreast"],
        canonical: &["jreast.co.jp"],
    },
    Authority {
        names: &["e-gov", "法令"],
        canonical: &["elaws.e-gov.go.jp", "e-gov.go.jp"],
    },
];

/// Hosts that host a stale copy of another site's documentation. They are
/// mirrors of the original and must never outrank it.
const MIRRORS: &[&str] = &[
    "docker-docs.uclv.cu",
    "docker-docs.netlify.app",
    "docs.docker.com.cn",
    "postgrespro.com",
    "readthedocs.org",
    "gitmemory.com",
    "devdocs.io",
    "manpages.ubuntu.com",
];

/// Does the query look Japanese (any CJK / kana)?
fn query_is_japanese(query: &str) -> bool {
    query.chars().any(|c| {
        matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff)
    })
}

/// Penalty for a Wikipedia edition that does not match the query language.
///
/// A Czech or Gan Wikipedia article is almost never the answer to an English
/// or Japanese query; those editions only add noise to the merged results.
fn wikipedia_language_penalty(query: &str, url: &str) -> f64 {
    let Some(host) = host_of(url) else { return 0.0 };
    let Some(subdomain) = host.strip_suffix(".wikipedia.org") else {
        return 0.0;
    };
    if subdomain.contains('.') {
        return 0.0; // e.g. zh-min-nan.wikipedia.org counts as its own lang
    }
    let japanese = query_is_japanese(query);
    let matches = if japanese {
        matches!(subdomain, "ja")
    } else {
        matches!(subdomain, "en" | "simple")
    };
    if matches { 0.0 } else { -0.45 }
}

/// Language codes that documentation sites put in their URLs.
const PAGE_LANGUAGES: &[&str] = &[
    "ar", "cs", "de", "en", "es", "fa", "fr", "he", "hu", "id", "it", "ja", "ko", "nl", "pl", "pt",
    "ru", "sv", "th", "tr", "uk", "vi", "zh",
];

/// The language a URL marks its page as being in, if any: a leading path
/// segment (`/ja/`, `/ja-jp/`, `/zh_CN/`), a `translations/<lang>` segment, or
/// a language subdomain (`ja.react.dev`). Wikipedia is handled on its own.
pub fn page_language(url: &str) -> Option<&'static str> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host.ends_with(".wikipedia.org") {
        return None;
    }
    let code = |segment: &str| -> Option<&'static str> {
        let lower = segment.to_ascii_lowercase();
        let (language, region) = match lower.split_once(['-', '_']) {
            Some((language, region)) => (language.to_string(), Some(region.to_string())),
            None => (lower, None),
        };
        // A region or script is short ("jp", "hans"); anything longer is a word.
        if region
            .as_ref()
            .is_some_and(|region| !(2..=4).contains(&region.len()))
        {
            return None;
        }
        PAGE_LANGUAGES
            .iter()
            .find(|known| **known == language)
            .copied()
    };
    let segments: Vec<&str> = parsed.path_segments()?.filter(|s| !s.is_empty()).collect();
    if let Some(found) = segments.first().and_then(|segment| code(segment)) {
        return Some(found);
    }
    if let Some(index) = segments
        .iter()
        .position(|segment| *segment == "translations")
    {
        if let Some(found) = segments.get(index + 1).and_then(|segment| code(segment)) {
            return Some(found);
        }
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() >= 3 {
        // "id." and "it." are more often services than languages.
        if let Some(found) = code(labels[0]).filter(|code| !matches!(*code, "id" | "it")) {
            return Some(found);
        }
    }
    None
}

/// Score adjustment for a page whose URL names its language: a page in the
/// query's language is lifted a little, one in a third language pushed down.
/// English pages are neutral (the usual fallback for documentation).
fn page_language_adjustment(query: &str, url: &str) -> f64 {
    let Some(language) = page_language(url) else {
        return 0.0;
    };
    let wanted = if query_is_japanese(query) { "ja" } else { "en" };
    if language == wanted {
        if wanted == "en" { 0.0 } else { 0.3 }
    } else if language == "en" {
        0.0
    } else {
        -0.45
    }
}

/// Match a canonical host.
///
/// Most sites run separate services on subdomains (\`users.rust-lang.org\` is the
/// forum, not the documentation), so only the bare host and its \`www.\` variant
/// match. Wikipedia is the exception: it is partitioned by language and every
/// edition is genuinely the same encyclopaedia.
fn canonical_matches(host: &str, domain: &str) -> bool {
    if host == domain || host == format!("www.{domain}") {
        return true;
    }
    if host.strip_prefix("www.") == Some(domain) {
        return true;
    }
    const WHOLE_SITE: &[&str] = &["wikipedia.org"];
    WHOLE_SITE
        .iter()
        .any(|site| domain == *site && domain_matches(host, site))
}

/// The canonical host the query asks for, when it names a known site.
///
/// \`"python dict get method"\` -> \`Some("docs.python.org")\`. Used to run a
/// site-scoped second pass when the site's own page did not surface.
pub fn canonical_host_for_query(query: &str) -> Option<&'static str> {
    let lower = query.to_lowercase();
    let words: Vec<&str> = lower
        .split(|character: char| {
            !character.is_alphanumeric() && character != '.' && character != '-'
        })
        .collect();
    for authority in AUTHORITIES {
        let named = authority.names.iter().any(|name| {
            words.iter().any(|word| word == name)
                || (name.chars().count() >= 6 && lower.contains(name))
        });
        if named {
            // Prefer the most specific canonical host (the documentation one).
            return authority
                .canonical
                .iter()
                .max_by_key(|domain| domain.split('.').count())
                .copied();
        }
    }
    None
}

/// Pick the canonical host this query is really asking for.
///
/// A site can name several hosts, and the right one depends on the wording:
/// "docker hub" wants hub.docker.com while "docker run reference" wants
/// docs.docker.com. Score each host by how many of its distinctive labels
/// appear in the query, then prefer the more specific host on a tie.
fn best_canonical_for(query: &str, host: &str, authority: &Authority) -> Option<bool> {
    let words: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    let score = |domain: &str| -> (usize, usize) {
        let labels: Vec<&str> = domain.split('.').collect();
        let distinctive = labels[..labels.len().saturating_sub(2)]
            .iter()
            .filter(|label| words.iter().any(|word| word == *label))
            .count();
        (distinctive, labels.len())
    };
    let global = authority.canonical.iter().map(|d| score(d)).max()?;
    let matching = authority
        .canonical
        .iter()
        .filter(|domain| canonical_matches(host, domain))
        .map(|domain| score(domain))
        .max()?;
    Some(matching == global)
}

/// How authoritative a result is for this query, in 0.0 ..= 1.0.
///
/// A named site's canonical hosts score 1.0, its mirror copies score 0.0 and
/// everything else sits in the middle. This is what separates "the official
/// page" from "a tutorial that quotes it".
fn authority(query: &str, url: &str) -> f64 {
    let lower = query.to_lowercase();
    let host = match host_of(url) {
        Some(host) => host,
        None => return 0.5,
    };
    let words: Vec<&str> = lower
        .split(|character: char| {
            !character.is_alphanumeric() && character != '.' && character != '-'
        })
        .collect();
    // Several sites can be named at once ("docker hub" also names "docker"), so
    // take the most specific name that matched: the longer phrase wins.
    let mut matched: Option<(&Authority, usize)> = None;
    for authority in AUTHORITIES {
        // A short name only matches a whole word ("git" must not match "github");
        // a distinctive name may appear inside a compound ("postgresql").
        let score = authority
            .names
            .iter()
            .filter(|name| {
                words.iter().any(|word| word == *name)
                    || (name.chars().count() >= 6 && lower.contains(*name))
            })
            .map(|name| name.chars().count())
            .max();
        if let Some(score) = score {
            if matched.is_none_or(|(_, best)| score > best) {
                matched = Some((authority, score));
            }
        }
    }
    {
        let Some((authority, _)) = matched else {
            if MIRRORS.iter().any(|mirror| domain_matches(&host, mirror)) {
                return 0.2;
            }
            return 0.5;
        };
        if let Some(best) = best_canonical_for(query, &host, authority) {
            return if best { 1.0 } else { 0.85 };
        }
        if MIRRORS.iter().any(|mirror| domain_matches(&host, mirror)) {
            return 0.0;
        }
        0.4
    }
}

/// Search engines wrap outbound links in their own redirector
/// (\`google.com/goto?url=\`, \`duckduckgo.com/l/?uddg=\`, ...). Left alone, those
/// wrappers pollute the result set with links that are not the page at all.
///
/// Returns the real destination, \`None\` when the wrapper carries no usable
/// target (the result must be dropped), and the input unchanged otherwise.
pub fn unwrap_redirect(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let Ok(parsed) = url::Url::parse(trimmed) else {
        return Some(trimmed.to_string());
    };
    let host = parsed.host_str().unwrap_or_default().to_lowercase();
    let path = parsed.path().to_lowercase();

    // Which query parameter carries the destination, per wrapper.
    let wrapper = if (host.ends_with("google.") || host.contains(".google."))
        && (path.starts_with("/goto") || path.starts_with("/url"))
    {
        Some(&["url", "q", "continue"][..])
    } else if host.ends_with("duckduckgo.com") && path.starts_with("/l/") {
        Some(&["uddg"][..])
    } else if host.ends_with("bing.com") && path.starts_with("/ck/a") {
        Some(&["u", "url"][..])
    } else if host.ends_with("l.facebook.com") && path.starts_with("/l.php") {
        Some(&["u"][..])
    } else if (host.ends_with("link.zhihu.com") || host.ends_with("link.juejin.cn"))
        && path.starts_with("/")
    {
        Some(&["target"][..])
    } else {
        None
    };
    let Some(keys) = wrapper else {
        return Some(trimmed.to_string());
    };
    for (key, value) in parsed.query_pairs() {
        if keys.contains(&key.as_ref())
            && (value.starts_with("http://") || value.starts_with("https://"))
        {
            return Some(value.into_owned());
        }
    }
    // A wrapper without a target is a dead end: drop the result rather than
    // showing the redirector itself.
    None
}

/// Relevance of one result to the query terms, for callers outside this
/// module (the site-scoped second pass picks a site's best page with this).
pub fn relevance_score(result: &SearchResult, terms: &[String]) -> f64 {
    relevance(&result.url, &result.title, &result.content, terms)
}

/// The host of a URL, lowercased and without a port.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, rest)| rest)?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

/// Whether [host] is [domain] itself or one of its subdomains.
pub fn domain_matches(host: &str, domain: &str) -> bool {
    let domain = normalize_domain(domain);
    !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
}

pub(crate) fn normalize_domain(domain: &str) -> String {
    domain
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or(domain)
        .split(':')
        .next()
        .unwrap_or(domain)
        .trim_start_matches("www.")
        .to_lowercase()
}

/// Apply the optional agent-facing result filters.
pub fn filter_results(
    results: &mut Vec<SearchResult>,
    include_domains: &[String],
    exclude_domains: &[String],
    max_results: Option<usize>,
) {
    if !include_domains.is_empty() {
        results.retain(|result| {
            host_of(&result.url).is_some_and(|host| {
                include_domains
                    .iter()
                    .any(|domain| domain_matches(&host, domain))
            })
        });
    }
    if !exclude_domains.is_empty() {
        results.retain(|result| {
            host_of(&result.url).is_none_or(|host| {
                !exclude_domains
                    .iter()
                    .any(|domain| domain_matches(&host, domain))
            })
        });
    }
    if let Some(limit) = max_results {
        results.truncate(limit.max(1));
    }
}

/// The deduplication key of a URL: scheme independent, without fragment and
/// without the usual tracking parameters.
///
/// The key is assembled from the parsed URL structure, not by concatenating
/// decoded text, so values and separators stay distinguishable:
///
/// - the host keeps its port, except the https default (443). The key always
///   assumes https, so `http://e.test/p`, `https://e.test/p` and
///   `https://e.test:443/p` share one key, while `:8443` and `:9443` differ;
/// - query pairs are re-encoded as pairs, so `?a=b%26c%3Dd` (one value holding
///   `&c=d`) never collapses into `?a=b&c=d` (two parameters);
/// - query order is preserved, since servers may treat it as significant.
pub fn dedup_key(url: &str) -> String {
    let trimmed = url.trim();
    let Ok(mut parsed) = url::Url::parse(trimmed) else {
        return trimmed.to_lowercase();
    };
    parsed.set_fragment(None);
    let _ = parsed.set_scheme("https");
    let host = parsed.host_str().unwrap_or_default().to_lowercase();
    // `port()` keeps an explicit port unless it is the default for the current
    // scheme, and `set_scheme` does not renormalise, so compare explicitly.
    let authority = match parsed.port().filter(|port| *port != 443) {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    let path = parsed.path().trim_end_matches('/');
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in parsed.query_pairs() {
        if is_tracking_param(&key) {
            continue;
        }
        serializer.append_pair(&key, &value);
    }
    let query = serializer.finish();
    format!(
        "{authority}{path}{}",
        if query.is_empty() {
            String::new()
        } else {
            format!("?{query}")
        }
    )
}

/// Tracking parameters removed when deduplicating (a subset of the ClearURLs
/// rules used by the tracker remover plugin).
pub fn is_tracking_param(key: &str) -> bool {
    let key = key.to_lowercase();
    key.starts_with("utm_")
        || matches!(
            key.as_str(),
            "fbclid"
                | "gclid"
                | "dclid"
                | "msclkid"
                | "yclid"
                | "_openstat"
                | "igshid"
                | "mc_eid"
                | "mkt_tok"
                | "vero_id"
                | "wickedid"
                | "oly_anon_id"
                | "oly_enc_id"
                | "s_cid"
                | "spm"
                | "ref_src"
                | "ref_url"
                | "cmpid"
                | "campaign_id"
        )
}

/// A short human readable URL for the UI.
pub fn pretty_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or_default();
            let path = parsed.path().trim_end_matches('/');
            if path.is_empty() || path == "/" {
                host.to_string()
            } else {
                format!("{host}{path}")
            }
        }
        Err(_) => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{page_language, page_language_adjustment};

    #[test]
    fn page_languages_are_read_from_urls() {
        assert_eq!(
            page_language("https://docs.python.org/ja/3/library/"),
            Some("ja")
        );
        assert_eq!(
            page_language("https://developer.mozilla.org/en-US/docs/Web"),
            Some("en")
        );
        assert_eq!(
            page_language("https://learn.microsoft.com/ja-jp/azure/"),
            Some("ja")
        );
        assert_eq!(page_language("https://ja.react.dev/learn"), Some("ja"));
        assert_eq!(
            page_language("https://www.kernel.org/doc/html/latest/translations/zh_CN/index.html"),
            Some("zh")
        );
        assert_eq!(page_language("https://docs.python.org/3/library/"), None);
        assert_eq!(page_language("https://example.org/de-facto-standard"), None);
        assert_eq!(page_language("https://id.atlassian.com/login"), None);
        assert_eq!(page_language("https://ja.wikipedia.org/wiki/X"), None);
    }

    #[test]
    fn pages_in_the_query_language_are_preferred() {
        let ja = "https://developer.mozilla.org/ja/docs/Web/JavaScript";
        let en = "https://developer.mozilla.org/en-US/docs/Web/JavaScript";
        let fr = "https://developer.mozilla.org/fr/docs/Web/JavaScript";
        assert!(page_language_adjustment("mdn javascript 入門", ja) > 0.0);
        assert_eq!(page_language_adjustment("mdn javascript 入門", en), 0.0);
        assert!(page_language_adjustment("mdn javascript intro", ja) < 0.0);
        assert!(page_language_adjustment("mdn javascript intro", fr) < 0.0);
        assert_eq!(page_language_adjustment("mdn javascript intro", en), 0.0);
    }

    use super::*;
    use crate::engine::EngineItem;

    fn result(url: &str, title: &str) -> EngineItem {
        EngineItem::Result(SearchResult::new(url, title))
    }

    #[test]
    fn duplicate_urls_are_merged() {
        let mut container = ResultContainer::new();
        container.extend("a", 1.0, vec![result("https://example.com/x", "one")]);
        container.extend("b", 1.0, vec![result("http://example.com/x#anchor", "one")]);
        assert_eq!(container.main_results().len(), 1);
        assert_eq!(container.main_results()[0].engines.len(), 2);
    }

    #[test]
    fn executable_result_urls_are_rejected() {
        for url in [
            "javascript:void(0)",
            "JaVaScRiPt:void(0)",
            "\tjava\nscript:void(0)",
            "data:text/html,example",
            "vbscript:example",
            "https://",
            "",
        ] {
            for template in [Template::Default, Template::KeyValue] {
                let mut container = ResultContainer::new();
                let item = SearchResult::new(url, "unsafe").with_template(template);
                container.extend("a", 1.0, vec![EngineItem::Result(item)]);
                assert!(container.main_results().is_empty(), "{url:?}");
            }
        }
    }

    #[test]
    fn web_relative_and_keyvalue_results_are_preserved() {
        for url in [
            "https://example.com/a",
            "http://example.com/b",
            "/docs/a.pdf",
            "../docs/b.pdf",
            "//example.com/c",
            "?page=2",
            "#section",
        ] {
            let mut container = ResultContainer::new();
            container.extend("a", 1.0, vec![result(url, "safe")]);
            assert_eq!(container.main_results()[0].url, url);
        }

        let mut container = ResultContainer::new();
        let row = SearchResult::new("keyvalue:0", "database row")
            .with_template(Template::KeyValue)
            .with_metadata("magnetlink", "magnet:?xt=urn:btih:example");
        container.extend("db", 1.0, vec![EngineItem::Result(row)]);
        assert_eq!(container.main_results()[0].url, "keyvalue:0");
        assert_eq!(
            container.main_results()[0].metadata["magnetlink"],
            "magnet:?xt=urn:btih:example"
        );
    }

    #[test]
    fn merged_images_and_infobox_links_reject_executable_urls() {
        let mut container = ResultContainer::new();
        container.extend("a", 1.0, vec![result("https://example.com/a", "safe")]);
        let mut duplicate = SearchResult::new("https://example.com/a", "safe");
        duplicate.img_src = Some("JaVaScRiPt:void(0)".into());
        duplicate.thumbnail = Some("data:text/html,example".into());
        let infobox = Infobox {
            id: "box".into(),
            title: "reference".into(),
            ..Default::default()
        };
        container.extend("a", 1.0, vec![EngineItem::Infobox(infobox.clone())]);
        container.extend(
            "b",
            1.0,
            vec![
                EngineItem::Result(duplicate),
                EngineItem::Infobox(Infobox {
                    img_src: Some("java\tscript:void(0)".into()),
                    url: Some("javascript:void(0)".into()),
                    urls: vec![
                        InfoboxUrl {
                            title: "unsafe".into(),
                            url: "JaVaScRiPt:void(0)".into(),
                        },
                        InfoboxUrl {
                            title: "safe".into(),
                            url: "https://example.com/reference".into(),
                        },
                    ],
                    ..infobox
                }),
            ],
        );
        assert!(container.main_results()[0].img_src.is_none());
        assert!(container.main_results()[0].thumbnail.is_none());
        assert!(container.infoboxes[0].img_src.is_none());
        assert!(container.infoboxes[0].url.is_none());
        assert_eq!(container.infoboxes[0].urls.len(), 1);
        assert_eq!(
            container.infoboxes[0].urls[0].url,
            "https://example.com/reference"
        );

        let mut safe = SearchResult::new("https://example.com/a", "safe");
        safe.img_src = Some("https://example.com/image.png".into());
        safe.thumbnail = Some("/thumb.png".into());
        container.extend(
            "c",
            1.0,
            vec![
                EngineItem::Result(safe),
                EngineItem::Infobox(Infobox {
                    id: "box".into(),
                    img_src: Some("https://example.com/image.png".into()),
                    url: Some("/reference".into()),
                    ..Default::default()
                }),
            ],
        );
        assert_eq!(
            container.main_results()[0].img_src.as_deref(),
            Some("https://example.com/image.png")
        );
        assert_eq!(
            container.main_results()[0].thumbnail.as_deref(),
            Some("/thumb.png")
        );
        assert_eq!(
            container.infoboxes[0].img_src.as_deref(),
            Some("https://example.com/image.png")
        );
        assert_eq!(container.infoboxes[0].url.as_deref(), Some("/reference"));
    }

    #[test]
    fn tracking_parameters_do_not_prevent_merging() {
        let mut container = ResultContainer::new();
        container.extend("a", 1.0, vec![result("https://example.com/x?a=1", "one")]);
        container.extend(
            "b",
            1.0,
            vec![result("https://example.com/x?a=1&utm_source=news", "one")],
        );
        assert_eq!(container.main_results().len(), 1);
    }

    #[test]
    fn higher_ranked_positions_score_higher() {
        let mut container = ResultContainer::new();
        container.extend(
            "a",
            1.0,
            vec![
                result("https://example.com/first", "first"),
                result("https://example.com/second", "second"),
            ],
        );
        container.close(|_| 1.0, &[], 0.0, "", 0.0);
        let ordered = container.ordered();
        assert_eq!(ordered[0].url, "https://example.com/first");
        assert!(ordered[0].score > ordered[1].score);
    }

    #[test]
    fn engine_weight_multiplies_the_score() {
        let mut container = ResultContainer::new();
        container.extend("a", 0.5, vec![result("https://example.com/a", "a")]);
        container.extend("b", 2.0, vec![result("https://example.com/b", "b")]);
        container.close(
            |engine| if engine == "a" { 0.5 } else { 2.0 },
            &[],
            0.0,
            "",
            0.0,
        );
        // `ordered` is the ranking the UI shows: engine weight wins.
        assert_eq!(container.ordered()[0].url, "https://example.com/b");
    }

    #[test]
    fn weights_supplied_with_results_override_registry_defaults() {
        let mut container = ResultContainer::new();
        container.extend(
            "routed",
            2.0,
            vec![result("https://example.com/routed", "routed")],
        );
        container.extend(
            "official site",
            0.5,
            vec![result("https://example.com/official", "official")],
        );
        container.close(|_| 1.0, &[], 0.0, "", 0.0);
        assert_eq!(container.ordered()[0].score, 2.0);
        assert_eq!(container.ordered()[1].score, 0.5);
    }

    #[test]
    fn repeated_urls_from_one_source_cast_one_vote() {
        let mut container = ResultContainer::new();
        container.extend(
            "a",
            1.0,
            vec![
                result("https://example.com/page", "page"),
                result("http://example.com/page?utm_source=a", "page again"),
            ],
        );
        container.extend_from_page(
            "a",
            1.0,
            vec![result("https://example.com/page", "page again")],
            1,
        );
        container.close(|_| 1.0, &[], 0.0, "", 0.0);
        let result = &container.ordered()[0];
        assert_eq!(result.engines, ["a"]);
        assert_eq!(result.positions, [1]);
        assert_eq!(result.score, 1.0);
    }

    #[test]
    fn independent_sources_still_contribute_their_best_positions() {
        let mut container = ResultContainer::new();
        container.extend_from_page(
            "a",
            1.0,
            vec![result("https://example.com/page", "page")],
            1,
        );
        container.extend("a", 1.0, vec![result("https://example.com/page", "page")]);
        container.extend("b", 1.0, vec![result("https://example.com/page", "page")]);
        container.close(|_| 1.0, &[], 0.0, "", 0.0);
        let result = &container.ordered()[0];
        assert_eq!(result.engines, ["a", "b"]);
        assert_eq!(result.positions, [1, 1]);
        assert_eq!(result.deep_page, 0);
        assert_eq!(result.score, 4.0);
    }

    #[test]
    fn query_specific_weights_do_not_leak_between_results_of_one_source() {
        for reverse in [false, true] {
            let mut container = ResultContainer::new();
            let mut passes = vec![
                (
                    1.0,
                    vec![
                        result("https://example.com/plain", "plain"),
                        result("https://example.com/shared", "shared"),
                    ],
                ),
                (
                    2.0,
                    vec![
                        result("https://example.com/shared", "shared"),
                        result("https://example.com/routed", "routed"),
                    ],
                ),
            ];
            if reverse {
                passes.reverse();
            }
            for (weight, items) in passes {
                container.extend("source", weight, items);
            }
            container.close(|_| 1.0, &[], 0.0, "", 0.0);
            let scores: std::collections::BTreeMap<_, _> = container
                .ordered()
                .iter()
                .map(|result| (result.url.as_str(), result.score))
                .collect();
            assert_eq!(scores["https://example.com/plain"], 1.0);
            assert_eq!(scores["https://example.com/shared"], 2.0);
            assert_eq!(scores["https://example.com/routed"], 1.0);
        }
    }

    #[test]
    fn suggestions_answers_and_corrections_are_collected() {
        let mut container = ResultContainer::new();
        container.extend(
            "a",
            1.0,
            vec![
                EngineItem::Suggestion("did you mean rust".into()),
                EngineItem::Answer("42".into()),
                EngineItem::Correction("corrected".into()),
                result("https://example.com/x", "x"),
            ],
        );
        assert!(container.suggestions.contains("did you mean rust"));
        assert_eq!(container.answers, vec!["42".to_string()]);
        assert!(container.corrections.contains("corrected"));
    }

    #[test]
    fn results_of_the_same_category_stay_close_together() {
        let mut container = ResultContainer::new();
        let mut items = Vec::new();
        for index in 0..10 {
            let mut image = SearchResult::new(format!("https://img.example.com/{index}"), "img");
            image.category = "images".into();
            image.template = Template::Images;
            items.push(EngineItem::Result(image));
            let mut web = SearchResult::new(format!("https://web.example.com/{index}"), "web");
            web.category = "general".into();
            web.score = 100.0 - index as f64;
            items.push(EngineItem::Result(web));
        }
        container.extend("a", 1.0, items);
        container.close(|_| 1.0, &[], 0.0, "", 0.0);
        let ordered = container.ordered();
        // Within the first 8 entries of the images group every second entry is an
        // image result (upstream groups results of one category).
        assert!(ordered[..8].iter().all(|r| r.url.contains("example.com")));
    }

    #[test]
    fn dedup_key_ignores_scheme_fragment_and_tracking() {
        assert_eq!(
            dedup_key("http://Example.com/a/?utm_source=x#frag"),
            dedup_key("https://example.com/a")
        );
    }

    #[test]
    fn query_delimiters_inside_a_value_do_not_collide_with_parameter_boundaries() {
        // `b%26c%3Dd` is one value that contains "&c=d", not two parameters.
        assert_eq!(
            dedup_key("https://e.test/p?a=b%26c%3Dd"),
            "e.test/p?a=b%26c%3Dd"
        );
        assert_eq!(dedup_key("https://e.test/p?a=b&c=d"), "e.test/p?a=b&c=d");
        assert_ne!(
            dedup_key("https://e.test/p?a=b%26c%3Dd"),
            dedup_key("https://e.test/p?a=b&c=d")
        );
    }

    #[test]
    fn the_port_is_part_of_the_identity() {
        assert_eq!(dedup_key("https://e.test:8443/p"), "e.test:8443/p");
        assert_eq!(dedup_key("https://e.test:9443/p"), "e.test:9443/p");
        assert_ne!(
            dedup_key("https://e.test:8443/p"),
            dedup_key("https://e.test:9443/p")
        );
    }

    #[test]
    fn the_https_default_port_is_elided() {
        // Spec: the key is scheme independent and always assumes https, so the
        // explicit default port and the absent port produce the same key.
        assert_eq!(dedup_key("https://e.test:443/p"), "e.test/p");
        assert_eq!(dedup_key("http://e.test/p"), "e.test/p");
        assert_eq!(dedup_key("https://e.test:8443/p"), "e.test:8443/p");
    }

    #[test]
    fn query_parameter_order_is_significant() {
        assert_eq!(dedup_key("https://e.test/p?a=1&b=2"), "e.test/p?a=1&b=2");
        assert_eq!(dedup_key("https://e.test/p?b=2&a=1"), "e.test/p?b=2&a=1");
        assert_ne!(
            dedup_key("https://e.test/p?a=1&b=2"),
            dedup_key("https://e.test/p?b=2&a=1")
        );
    }

    #[test]
    fn duplicate_results_keep_the_returned_https_url_in_either_order() {
        for urls in [
            ["http://github.com/trending", "https://github.com/trending"],
            ["https://github.com/trending", "http://github.com/trending"],
        ] {
            let mut container = ResultContainer::new();
            container.extend("a", 1.0, vec![result(urls[0], "trending")]);
            container.extend("b", 1.0, vec![result(urls[1], "trending")]);
            assert_eq!(container.main_results().len(), 1);
            assert_eq!(
                container.main_results()[0].url,
                "https://github.com/trending"
            );
        }
    }

    #[test]
    fn canonical_subsites_do_not_lend_authority_to_neighbours() {
        // docs.rust-lang.org is exact: the forum next door is not "the docs"
        assert_eq!(
            authority("rust traits", "https://doc.rust-lang.org/book/"),
            1.0
        );
        assert!(authority("rust traits", "https://users.rust-lang.org/t/x") < 1.0);
        // a whole-site canonical still covers its subdomains
        assert_eq!(
            authority("numpy indexing", "https://numpy.org/doc/stable/"),
            1.0
        );
        assert_eq!(
            authority("wikipedia tokyo", "https://ja.wikipedia.org/wiki/東京"),
            1.0
        );
    }

    #[test]
    fn wrong_language_wikipedia_is_penalised() {
        // English query: en/simple fine, cs/gan are noise
        assert_eq!(
            wikipedia_language_penalty("tokyo wikipedia", "https://en.wikipedia.org/wiki/Tokyo"),
            0.0
        );
        assert!(
            wikipedia_language_penalty("tokyo wikipedia", "https://cs.wikipedia.org/wiki/Tokyo")
                < 0.0
        );
        assert!(
            wikipedia_language_penalty("tokyo wikipedia", "https://gan.wikipedia.org/wiki/Tokyo")
                < 0.0
        );
        // Japanese query: ja fine, en is noise
        assert_eq!(
            wikipedia_language_penalty("富士山 wikipedia", "https://ja.wikipedia.org/wiki/富士山"),
            0.0
        );
        assert!(
            wikipedia_language_penalty(
                "富士山 wikipedia",
                "https://en.wikipedia.org/wiki/Mount_Fuji"
            ) < 0.0
        );
        // non-wikipedia hosts are untouched
        assert_eq!(
            wikipedia_language_penalty("tokyo", "https://tokyo.rs/"),
            0.0
        );
    }

    #[test]
    fn redirect_wrappers_are_unwrapped_or_dropped() {
        assert_eq!(
            unwrap_redirect("https://www.google.com/goto?url=https%3A%2F%2Ftokio.rs%2F"),
            Some("https://tokio.rs/".to_string())
        );
        assert_eq!(
            unwrap_redirect("https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&rut=x"),
            Some("https://example.com/a".to_string())
        );
        // a wrapper with no destination is dropped
        assert_eq!(unwrap_redirect("https://www.google.com/goto?url="), None);
        // an ordinary URL is untouched
        assert_eq!(
            unwrap_redirect("https://tokio.rs/tokio/tutorial"),
            Some("https://tokio.rs/tokio/tutorial".to_string())
        );
    }

    #[test]
    fn canonical_host_for_query_finds_the_named_site() {
        assert_eq!(
            canonical_host_for_query("python dict get method"),
            Some("docs.python.org")
        );
        assert_eq!(
            canonical_host_for_query("rust traits"),
            Some("doc.rust-lang.org")
        );
        assert_eq!(
            canonical_host_for_query("docker compose reference"),
            Some("docs.docker.com")
        );
        assert_eq!(canonical_host_for_query("some random topic"), None);
    }

    #[test]
    fn identifiers_in_the_query_boost_the_matching_url() {
        let terms = query_terms("http semantics rfc 9110");
        let exact = relevance(
            "https://www.rfc-editor.org/rfc/rfc9110",
            "RFC 9110: HTTP Semantics",
            "",
            &terms,
        );
        let other = relevance(
            "https://www.rfc-editor.org/rfc/rfc2616",
            "RFC 2616: HTTP/1.1",
            "",
            &terms,
        );
        assert!(exact > other, "{exact} should beat {other}");
    }

    #[test]
    fn the_canonical_host_follows_the_query_wording() {
        // "docker hub" asks for the hub, "docker run reference" for the docs
        assert_eq!(
            authority("docker hub", "https://hub.docker.com/_/nginx"),
            1.0
        );
        assert!(authority("docker hub", "https://docs.docker.com/engine/") < 1.0);
        assert_eq!(
            authority("docker run reference", "https://docs.docker.com/reference/"),
            1.0
        );
        assert!(authority("docker run reference", "https://hub.docker.com/_/nginx") < 1.0);
    }

    #[test]
    fn authority_lifts_canonical_and_demotes_mirrors() {
        // naming arxiv: the canonical host wins, wikipedia loses
        assert_eq!(
            authority(
                "attention is all you need arxiv",
                "https://arxiv.org/abs/1706.03762"
            ),
            1.0
        );
        assert!(
            authority(
                "attention is all you need arxiv",
                "https://en.wikipedia.org/wiki/Attention"
            ) < 0.5
        );
        // naming docker: official docs win, a mirror is pushed to the floor
        assert_eq!(
            authority(
                "docker compose file reference",
                "https://docs.docker.com/reference/compose-file/"
            ),
            1.0
        );
        assert_eq!(
            authority(
                "docker compose file reference",
                "https://docker-docs.uclv.cu/compose/compose-file/"
            ),
            0.0
        );
        // a named site in the list is lifted to its canonical host
        assert_eq!(authority("tokio async runtime", "https://tokio.rs/"), 1.0);
        // no listed site named: everything sits in the middle
        assert_eq!(authority("some random topic", "https://example.com/"), 0.5);
        // a mirror of an unnamed site is still demoted
        assert_eq!(authority("some docs", "https://docker-docs.uclv.cu/x"), 0.2);
    }

    #[test]
    fn pretty_url_strips_the_scheme() {
        assert_eq!(pretty_url("https://example.com/a/b/"), "example.com/a/b");
        assert_eq!(pretty_url("https://example.com/"), "example.com");
    }

    fn sample(url: &str) -> SearchResult {
        SearchResult {
            url: url.to_string(),
            title: url.to_string(),
            ..SearchResult::default()
        }
    }

    #[test]
    fn host_of_handles_port_userinfo_and_case() {
        assert_eq!(
            host_of("https://WWW.Example.com:8443/a"),
            Some("www.example.com".into())
        );
        assert_eq!(
            host_of("https://user:pw@example.com/a"),
            Some("example.com".into())
        );
        assert_eq!(host_of("not a url"), None);
    }

    #[test]
    fn domain_matches_subdomains_but_not_lookalikes() {
        assert!(domain_matches("example.com", "example.com"));
        assert!(domain_matches("docs.example.com", "example.com"));
        assert!(domain_matches(
            "docs.example.com",
            "https://www.example.com/path"
        ));
        assert!(!domain_matches("notexample.com", "example.com"));
        assert!(!domain_matches("example.com.evil.test", "example.com"));
    }

    #[test]
    fn filter_results_applies_include_exclude_and_limit() {
        let mut results = vec![
            sample("https://keep.test/a"),
            sample("https://docs.keep.test/b"),
            sample("https://drop.test/c"),
            sample("https://other.test/d"),
        ];
        filter_results(&mut results, &["keep.test".into()], &[], None);
        assert_eq!(results.len(), 2);
        filter_results(&mut results, &[], &["docs.keep.test".into()], None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://keep.test/a");
        filter_results(&mut results, &[], &[], Some(0));
        assert_eq!(results.len(), 1, "a limit below one keeps one result");
    }

    #[test]
    fn raw_content_is_serialised_only_when_present() {
        let mut result = sample("https://example.com/");
        let plain = serde_json::to_value(&result).expect("json");
        assert!(plain.get("raw_content").is_none());
        result.raw_content = Some("page text".into());
        let rich = serde_json::to_value(&result).expect("json");
        assert_eq!(rich["raw_content"], "page text");
    }
}
