//! Search orchestration, ported from `searx/search/processors/online.py` and
//! `searx/search/__init__.py`: engine selection, concurrent execution,
//! per engine timeouts, suspensions, caching and result merging.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::engine::state::{EngineSnapshot, EngineStates};
use crate::engine::{EngineHandle, EngineItem, EngineRegistry, FailureKind};
use crate::metrics::Metrics;
use crate::network::Network;
use crate::plugins::{PluginContext, PluginManager, UserInfo};
use crate::query::{Query, TimeRange};
use crate::results::{Infobox, ResultContainer, SearchResult, Timing, UnresponsiveEngine};
use crate::settings::Settings;

/// Everything the searcher needs to know about one search request.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub query: Query,
    pub pageno: u32,
    pub categories: Vec<String>,
    pub engines: Vec<String>,
    pub disabled_engines: Vec<String>,
    pub time_range: Option<TimeRange>,
    pub safesearch: u8,
    pub language: String,
    pub timeout_limit: Option<f64>,
    pub enabled_plugins: Vec<String>,
    pub disabled_plugins: Vec<String>,
    pub locale: String,
    pub user: UserInfo,
    /// Keep at most this many results (agent-facing extension).
    pub max_results: Option<usize>,
    /// Keep only results whose host matches one of these domains.
    pub include_domains: Vec<String>,
    /// Drop results whose host matches one of these domains.
    pub exclude_domains: Vec<String>,
    /// Fetch the page text for the top results and return it as raw_content.
    pub include_raw_content: bool,
    /// Explicit Accept-Language preference for fetched page content.
    pub accept_language: crate::fetch::AcceptLanguage,
    /// How the selected engines are started.
    pub strategy: crate::settings::SearchStrategy,
    /// Set once the site-scoped second pass already ran, so it cannot recurse.
    pub site_retry_done: bool,
}

impl SearchRequest {
    /// A request with the defaults for a query, used by the API and tests.
    pub fn new(query: Query) -> Self {
        Self {
            pageno: 1,
            categories: query.selected_categories(),
            engines: Vec::new(),
            disabled_engines: query.disabled_engines.clone(),
            time_range: None,
            safesearch: query.safesearch,
            language: query.engine_language(),
            timeout_limit: query.timeout_limit,
            enabled_plugins: Vec::new(),
            disabled_plugins: Vec::new(),
            locale: query.locale.clone(),
            user: UserInfo::default(),
            max_results: None,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
            include_raw_content: false,
            accept_language: crate::fetch::AcceptLanguage::default(),
            strategy: crate::settings::SearchStrategy::Parallel,
            site_retry_done: false,
            query,
        }
    }

    fn explicit_engines(&self) -> Vec<String> {
        if self.engines.is_empty() {
            self.query.engine_ref_names()
        } else {
            self.engines.clone()
        }
    }

    fn is_single_source(&self) -> bool {
        let engines = self.explicit_engines();
        engines
            .first()
            .is_some_and(|first| engines.iter().all(|name| name.eq_ignore_ascii_case(first)))
    }

    fn is_general_search(&self) -> bool {
        self.engines.is_empty()
            && !self.query.specific
            && self.query.engine_refs.is_empty()
            && self.categories.iter().all(|category| {
                crate::categories::normalize(category) == crate::categories::GENERAL
            })
    }
}

/// The result of a search, mirroring the fields of upstream's JSON response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub search_query: String,
    pub number_of_results: u64,
    pub results: Vec<SearchResult>,
    /// Upstream's JSON API emits an array of `{"answer": ...}` objects.
    #[serde(serialize_with = "serialize_answers")]
    pub answers: Vec<String>,
    pub corrections: Vec<String>,
    pub infoboxes: Vec<Infobox>,
    pub suggestions: Vec<String>,
    /// Upstream's JSON API emits `[engine, translated error]` pairs.
    #[serde(serialize_with = "serialize_unresponsive")]
    pub unresponsive_engines: Vec<UnresponsiveEngine>,
    pub timings: Vec<Timing>,
    pub redirect_url: Option<String>,
    pub engine_data: BTreeMap<String, BTreeMap<String, String>>,
    pub engines: Vec<String>,
    pub pageno: u32,
    pub paging: bool,
    /// Set when the request itself could not be served (no engine, ...).
    pub error: Option<String>,
}

impl SearchResponse {
    pub fn new(query: &Query) -> Self {
        Self {
            query: query.full_query(),
            search_query: query.query_text(),
            number_of_results: 0,
            results: Vec::new(),
            answers: Vec::new(),
            corrections: Vec::new(),
            infoboxes: Vec::new(),
            suggestions: Vec::new(),
            unresponsive_engines: Vec::new(),
            timings: Vec::new(),
            redirect_url: None,
            engine_data: BTreeMap::new(),
            engines: Vec::new(),
            pageno: 1,
            paging: false,
            error: None,
        }
    }
}

/// The outcome of one engine run.
struct EngineOutcome {
    engine: String,
    weight: f64,
    items: Result<Vec<EngineItem>, FailureKind>,
    elapsed: Duration,
    suspended: bool,
}

/// Runs searches against the configured engines.
pub struct Searcher {
    pub settings: Arc<Settings>,
    pub registry: Arc<EngineRegistry>,
    pub network: Arc<Network>,
    /// Checked, bounded page fetcher shared by optional raw-content requests.
    pub page_reader: crate::fetch::PageReader,
    pub cache: Arc<Cache>,
    pub states: Arc<EngineStates>,
    pub metrics: Arc<Metrics>,
    pub plugins: Arc<PluginManager>,
    /// Our own BM25 index, when one is configured and opened.
    pub local_index: Option<Arc<crate::localindex::LocalIndex>>,
    /// The Wikipedia index built from the dump, when configured.
    pub wiki_index: Option<Arc<crate::localindex::LocalIndex>>,
}

/// Open the configured local index, if any.
fn open_local_index(settings: &Settings) -> Option<Arc<crate::localindex::LocalIndex>> {
    open_index_at(settings.search.local_index_path.trim())
}

/// Extra weight for a source the query names ("... arxiv", "... npm").
const ROUTED_WEIGHT: f64 = 2.0;

/// Longest wait for the Wikidata official-site lookup.
const OFFICIAL_SITE_BUDGET: Duration = Duration::from_millis(1500);

/// Extra time for a local index on a cold disk cache.
const INDEX_GRACE: Duration = Duration::from_millis(400);
const LOCAL_PAGE_SIZE: usize = 20;
static INDEX_SEARCH_SLOTS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(4)));

struct IndexFailure {
    kind: FailureKind,
    message: String,
}

/// Query a local index off the async workers and give up at the deadline.
async fn bounded_index_search(
    index: &Arc<crate::localindex::LocalIndex>,
    text: String,
    loose: bool,
    include_domains: Vec<String>,
    exclude_domains: Vec<String>,
    pageno: u32,
    deadline: Instant,
) -> Result<Vec<crate::localindex::Hit>, IndexFailure> {
    let index = Arc::clone(index);
    let expires = tokio::time::Instant::from_std(deadline + INDEX_GRACE);
    let permit =
        match tokio::time::timeout_at(expires, Arc::clone(&INDEX_SEARCH_SLOTS).acquire_owned())
            .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(error)) => {
                return Err(IndexFailure {
                    kind: FailureKind::Other,
                    message: error.to_string(),
                });
            }
            Err(_) => {
                return Err(IndexFailure {
                    kind: FailureKind::Timeout,
                    message: "local search timed out waiting for capacity".to_string(),
                });
            }
        };
    if tokio::time::Instant::now() >= expires {
        return Err(IndexFailure {
            kind: FailureKind::Timeout,
            message: "local search timed out waiting for capacity".to_string(),
        });
    }
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let start = (pageno.saturating_sub(1) as usize).saturating_mul(LOCAL_PAGE_SIZE);
        let end = start.saturating_add(LOCAL_PAGE_SIZE);
        let end = if end <= crate::localindex::MAX_SEARCH_WINDOW {
            end.saturating_add(1)
                .min(crate::localindex::MAX_SEARCH_WINDOW)
        } else {
            end
        };
        index.search_range(&text, start..end, &include_domains, &exclude_domains, loose)
    });
    match tokio::time::timeout_at(expires, task).await {
        Ok(Ok(Ok(hits))) => Ok(hits),
        Ok(Ok(Err(message))) => Err(IndexFailure {
            kind: FailureKind::Other,
            message,
        }),
        Ok(Err(error)) => Err(IndexFailure {
            kind: FailureKind::Other,
            message: format!("local search task failed: {error}"),
        }),
        Err(_) => Err(IndexFailure {
            kind: FailureKind::Timeout,
            message: "local search timed out".to_string(),
        }),
    }
}

/// At most this many hits per host from a multi-host index. A single-host
/// index (the encyclopedia) is exempt: every hit is the same host.
const MAX_PER_HOST: usize = 3;

/// Keep at most MAX hits per host, best first.
fn cap_per_host(hits: Vec<crate::localindex::Hit>, max: usize) -> Vec<crate::localindex::Hit> {
    let hosts: std::collections::HashSet<_> = hits
        .iter()
        .filter_map(|hit| {
            url::Url::parse(&hit.url)
                .ok()?
                .host_str()
                .map(str::to_owned)
        })
        .collect();
    if hosts.len() <= 1 {
        return hits;
    }
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    hits.into_iter()
        .filter(|hit| {
            let host = url::Url::parse(&hit.url)
                .ok()
                .and_then(|parsed| parsed.host_str().map(str::to_string))
                .unwrap_or_default();
            let count = counts.entry(host).or_insert(0);
            if *count >= max {
                return false;
            }
            *count += 1;
            true
        })
        .collect()
}

fn open_wiki_index(settings: &Settings) -> Option<Arc<crate::localindex::LocalIndex>> {
    open_index_at(settings.search.wikipedia_index_path.trim())
}

fn is_local_source(name: &str) -> bool {
    name.eq_ignore_ascii_case("local_index") || name.eq_ignore_ascii_case("wikipedia_index")
}

fn open_index_at(path: &str) -> Option<Arc<crate::localindex::LocalIndex>> {
    if path.is_empty() {
        return None;
    }
    if !std::path::Path::new(path).exists() {
        // The shipped settings name an index most installs never build.
        tracing::debug!(path, "local index not built; skipping it");
        return None;
    }
    match crate::localindex::LocalIndex::open(std::path::Path::new(path)) {
        Ok(index) => {
            tracing::info!(
                path,
                documents = index.document_count(),
                "local index opened"
            );
            Some(Arc::new(index))
        }
        Err(error) => {
            tracing::warn!(path, %error, "local index could not be opened");
            None
        }
    }
}

impl Searcher {
    pub fn new(mut settings: Settings) -> Result<Self, String> {
        settings.ensure_secret_key()?;
        let registry = EngineRegistry::from_settings(&settings)?;
        let network = Network::from_settings(&settings).map_err(|error| error.to_string())?;
        let cache = match settings.cache.backend.as_str() {
            "valkey" | "redis" => Cache::valkey(
                &settings.cache.url,
                Duration::from_secs(settings.cache.ttl.max(1)),
            ),
            _ => Cache::memory(
                Duration::from_secs(settings.cache.ttl.max(1)),
                settings.cache.max_bytes.max(1024),
            ),
        };
        let page_reader =
            crate::fetch::PageReader::from_settings(&settings.outgoing, &settings.fetch, &[])?
                .with_renderer(settings.search.page_renderer_command.clone())
                .with_cache_ttl(Duration::from_secs(settings.cache.ttl.max(1)));
        Ok(Self {
            page_reader,
            local_index: open_local_index(&settings),
            wiki_index: open_wiki_index(&settings),
            plugins: Arc::new(PluginManager::from_settings(&settings)),
            settings: Arc::new(settings),
            registry: Arc::new(registry),
            network: Arc::new(network),
            cache: Arc::new(cache),
            states: Arc::new(EngineStates::new()),
            metrics: Arc::new(Metrics::new()),
        })
    }

    /// Engines that answer a request, in registry order.
    pub fn select_engines(&self, request: &SearchRequest) -> Vec<EngineHandle> {
        let explicit = request.explicit_engines();
        let wanted_categories: Vec<String> = if !request.categories.is_empty() {
            request.categories.clone()
        } else {
            vec![crate::categories::GENERAL.to_string()]
        };

        // SearXNG ignores unknown engine names: fall back to the default set
        // when nothing matched, instead of failing the request.
        let run = |explicit: &[String]| {
            let mut selected = Vec::new();
            for engine in self.registry.iter() {
                let name = engine.name().to_string();
                if request
                    .disabled_engines
                    .iter()
                    .any(|disabled| disabled.eq_ignore_ascii_case(&name))
                {
                    continue;
                }
                if !explicit.is_empty() {
                    if explicit
                        .iter()
                        .any(|wanted| wanted.eq_ignore_ascii_case(&name))
                    {
                        selected.push(engine.clone());
                    }
                    continue;
                }
                let matches = engine.categories().iter().any(|category| {
                    let normalised = crate::categories::normalize(category);
                    wanted_categories
                        .iter()
                        .any(|wanted| crate::categories::normalize(wanted) == normalised)
                });
                if matches {
                    selected.push(engine.clone());
                }
            }
            selected
        };
        let selected = run(&explicit);
        if selected.is_empty()
            && !explicit.is_empty()
            && !explicit.iter().any(|name| is_local_source(name))
        {
            return run(&[]);
        }
        selected
    }

    fn index_is_selected(&self, request: &SearchRequest, name: &str) -> bool {
        if request
            .disabled_engines
            .iter()
            .any(|disabled| disabled.eq_ignore_ascii_case(name))
        {
            return false;
        }
        let explicit = request.explicit_engines();
        if !explicit.is_empty() {
            return explicit
                .iter()
                .any(|selected| selected.eq_ignore_ascii_case(name));
        }
        self.settings.search.local_index_merge && request.is_general_search()
    }

    /// Specialised engines a general search names by their own name
    /// ("attention is all you need arxiv", "tokio-rs tokio github"), with the
    /// request they should get: the same query without the site's name.
    /// Only engines of developer and science categories are routed, so a
    /// common word that happens to be an engine name does not add requests.
    fn site_routes(
        &self,
        request: &SearchRequest,
        selected: &[EngineHandle],
    ) -> Option<(Vec<EngineHandle>, SearchRequest)> {
        const ROUTABLE: &[&str] = &[
            "it",
            "repos",
            "packages",
            "q&a",
            "science",
            "scientific publications",
        ];
        const MAX_ROUTED: usize = 2;
        if !request.is_general_search() {
            return None;
        }
        let words: Vec<String> = request
            .query
            .user_query_parts
            .iter()
            .map(|word| word.to_lowercase())
            .collect();
        let mut routed: Vec<EngineHandle> = Vec::new();
        let mut matched: Vec<usize> = Vec::new();
        for engine in self.registry.iter() {
            if routed.len() >= MAX_ROUTED {
                break;
            }
            let name = engine.name().to_lowercase();
            if request
                .disabled_engines
                .iter()
                .any(|disabled| disabled.eq_ignore_ascii_case(&name))
                || selected.iter().any(|known| known.name() == engine.name())
                || !engine.categories().iter().any(|category| {
                    ROUTABLE.contains(&crate::categories::normalize(category).as_str())
                })
            {
                continue;
            }
            let name_words: Vec<&str> = name.split_whitespace().collect();
            if name_words.is_empty() || name.chars().count() < 3 || words.len() <= name_words.len()
            {
                continue;
            }
            let found = words
                .windows(name_words.len())
                .position(|window| window.iter().zip(&name_words).all(|(a, b)| a == b));
            if let Some(start) = found {
                routed.push(engine.clone());
                matched.extend(start..start + name_words.len());
            }
        }
        if routed.is_empty() {
            return None;
        }
        // Words that only say what kind of thing is wanted ("serde crate",
        // "react package") match unrelated entries inside the source itself.
        const KIND_WORDS: &[&str] = &[
            "crate",
            "crates",
            "package",
            "packages",
            "repo",
            "repository",
            "library",
            "paper",
        ];
        let kept: Vec<String> = request
            .query
            .user_query_parts
            .iter()
            .enumerate()
            .filter(|(index, _)| !matched.contains(index))
            .map(|(_, word)| word.clone())
            .collect();
        let without_kind: Vec<String> = kept
            .iter()
            .filter(|word| !KIND_WORDS.contains(&word.to_lowercase().as_str()))
            .cloned()
            .collect();
        let mut routed_request = request.clone();
        routed_request.query.user_query_parts = if without_kind.is_empty() {
            kept
        } else {
            without_kind
        };
        tracing::debug!(
            engines = ?routed.iter().map(|engine| engine.name().to_string()).collect::<Vec<_>>(),
            "query names a specialised source"
        );
        Some((routed, routed_request))
    }

    /// Run a search: this is the entry point used by the web layer.
    pub async fn search(&self, mut request: SearchRequest) -> SearchResponse {
        Metrics::incr(&self.metrics.searches, 1);
        let context = PluginContext {
            search_query: request.query.query_text(),
            locale: if request.locale.is_empty() {
                self.settings.search.default_lang.clone()
            } else {
                request.locale.clone()
            },
            base_url: self.settings.base_url(),
            user: request.user.clone(),
        };
        self.plugins.pre_search(
            &mut request.query,
            &context,
            &request.enabled_plugins,
            &request.disabled_plugins,
        );

        let mut response = SearchResponse::new(&request.query);
        response.pageno = request.pageno;

        // External bang: hand the query over to another service.
        if let Some(bang) = request.query.external_bang.clone() {
            if let Some(definition) = crate::external_bangs::lookup(&bang) {
                response.redirect_url = Some(crate::external_bangs::expand_url(
                    definition.url,
                    &request.query.query_text(),
                ));
                response.engines = Vec::new();
                return response;
            }
        }
        let mut engines = self.select_engines(&request);
        let local_index = self
            .local_index
            .as_ref()
            .filter(|_| self.index_is_selected(&request, "local_index"));
        let wiki_index = self
            .wiki_index
            .as_ref()
            .filter(|_| self.index_is_selected(&request, "wikipedia_index"));
        if engines.is_empty() && local_index.is_none() && wiki_index.is_none() {
            response.error = Some("no engine selected for this request".to_string());
            return response;
        }
        // Some upstream online processors only apply when the query carries
        // their own syntax (for example currency conversion or a translation
        // language pair). Skip those engines before requests and failure
        // accounting instead of reporting an unrelated general query as an
        // engine failure.
        engines.retain(|engine| engine.is_applicable(&request.query));

        // All providers share one deadline; completed results are retained.
        let deadline = Instant::now()
            + Duration::from_secs_f64(
                request
                    .timeout_limit
                    .unwrap_or(self.settings.search.timeout_limit)
                    .max(0.1),
            );
        // A general search that names a specialised source ("... arxiv",
        // "... github") also asks that source, without the site's name.
        let routed = self.site_routes(&request, &engines);
        let mut index_tasks = tokio::task::JoinSet::new();
        for (source, index, loose) in [
            ("local_index", local_index, false),
            ("wikipedia_index", wiki_index, true),
        ] {
            if let Some(index) = index {
                let index = Arc::clone(index);
                let text = if loose {
                    crate::engine::util::search_keywords(
                        &crate::engines::wikipedia::without_site_name(&request.query.query_text()),
                    )
                } else {
                    request.query.query_text()
                };
                let include_domains = request.include_domains.clone();
                let exclude_domains = request.exclude_domains.clone();
                let pageno = request.pageno;
                index_tasks.spawn(async move {
                    (
                        source,
                        bounded_index_search(
                            &index,
                            text,
                            loose,
                            include_domains,
                            exclude_domains,
                            pageno,
                            deadline,
                        )
                        .await,
                    )
                });
            }
        }
        // The official-site lookup starts as soon as Wikipedia has answered,
        // not after the slowest engine.
        let official_tasks = std::sync::Mutex::new(tokio::task::JoinSet::new());
        let start_official = |outcome: &EngineOutcome| {
            if let Ok(mut tasks) = official_tasks.lock() {
                self.start_official_lookup(&request, outcome, &mut tasks);
            }
        };
        let main = async {
            match request.strategy {
                crate::settings::SearchStrategy::Parallel => {
                    futures::future::join_all(engines.iter().map(|engine| async {
                        let outcome = self.run_engine(engine, &request, deadline).await;
                        start_official(&outcome);
                        outcome
                    }))
                    .await
                }
                crate::settings::SearchStrategy::Staged => {
                    self.run_staged(&engines, &request, deadline).await
                }
            }
        };
        let extra = async {
            match &routed {
                Some((routed_engines, routed_request)) => {
                    futures::future::join_all(
                        routed_engines
                            .iter()
                            .map(|engine| self.run_engine(engine, routed_request, deadline)),
                    )
                    .await
                }
                None => Vec::new(),
            }
        };
        let (mut outcomes, extra_outcomes) = futures::join!(main, extra);
        // The query named these sources: their own results are what was asked for.
        outcomes.extend(extra_outcomes.into_iter().map(|mut outcome| {
            outcome.weight *= ROUTED_WEIGHT;
            outcome
        }));

        // Staged searches did not start the lookup while running.
        for outcome in &outcomes {
            start_official(outcome);
        }
        let mut official_tasks = official_tasks
            .into_inner()
            .unwrap_or_else(|error| error.into_inner());

        // Remember which engines answered before the outcomes are consumed;
        // the deep-page pass only asks those again.
        let healthy: Vec<String> = outcomes
            .iter()
            .filter(|outcome| outcome.items.is_ok())
            .map(|outcome| outcome.engine.clone())
            .collect();

        let mut container = ResultContainer::new();
        for outcome in outcomes {
            response.engines.push(outcome.engine.clone());
            match outcome.items {
                Ok(items) => container.extend(&outcome.engine, outcome.weight, items),
                Err(kind) => {
                    container.add_unresponsive_engine(
                        &outcome.engine,
                        kind.error_type(),
                        outcome.suspended,
                    );
                }
            }
            container.add_timing(&outcome.engine, outcome.elapsed.as_secs_f64(), 0.0);
        }

        // Extra result pages: coverage, not ranking, is what limits recall for
        // documentation questions, so page 2 is worth fetching when asked for.
        let deep_pages = self.settings.search.deep_pages;
        if deep_pages > 0 && !request.is_single_source() && Instant::now() < deadline {
            let paging: Vec<EngineHandle> = engines
                .iter()
                .filter(|engine| engine.paging() && healthy.contains(&engine.name().to_string()))
                .cloned()
                .collect();
            for extra in 1..=deep_pages {
                if paging.is_empty() || Instant::now() >= deadline {
                    break;
                }
                let mut deeper = request.clone();
                deeper.pageno = request.pageno.saturating_add(extra);
                let page_outcomes = futures::future::join_all(
                    paging
                        .iter()
                        .map(|engine| self.run_engine(engine, &deeper, deadline)),
                )
                .await;
                for outcome in page_outcomes {
                    if let Ok(items) = outcome.items {
                        container.extend_from_page(&outcome.engine, outcome.weight, items, extra);
                    }
                }
            }
        }

        let query_terms = crate::results::query_terms(&request.query.query_text());
        // Relevance re-ranking only applies when the response merges several
        // engines (the general search). A single pinned engine keeps the
        // engine's own order, which preserves exact SearXNG overlap@10.
        let relevance_weight = if request.is_single_source() {
            0.0
        } else {
            self.settings.search.relevance_weight
        };
        // Canonical-site boost: naming a site means "that site's own page".
        let query_text = request.query.query_text();
        let authority_weight = if request.is_single_source() {
            0.0
        } else {
            self.settings.search.authority_weight
        };
        let mut indexed_hits = BTreeMap::new();
        while let Some(outcome) = index_tasks.join_next().await {
            if let Ok((source, hits)) = outcome {
                indexed_hits.insert(source, hits);
            }
        }
        let mut index_errors = Vec::new();
        for (source, outcome) in indexed_hits {
            response.engines.push(source.to_string());
            let mut hits = match outcome {
                Ok(hits) => hits,
                Err(error) => {
                    tracing::debug!(source, message = %error.message, "local index search failed");
                    container.add_unresponsive_engine(source, error.kind.error_type(), false);
                    index_errors.push(format!("{source}: {}", error.message));
                    continue;
                }
            };
            container.paging |= hits.len() > LOCAL_PAGE_SIZE;
            hits.truncate(LOCAL_PAGE_SIZE);
            if source == "local_index" && !request.is_single_source() {
                hits = cap_per_host(hits, MAX_PER_HOST);
            }
            let items: Vec<EngineItem> = hits
                .into_iter()
                .map(|hit| {
                    let url = if source == "wikipedia_index" {
                        crate::official::article_of(&hit.url)
                            .map(|article| crate::official::article_iri(&article))
                            .unwrap_or(hit.url)
                    } else {
                        hit.url
                    };
                    let mut result = SearchResult::new(url, hit.title);
                    result.content = hit.snippet;
                    EngineItem::Result(result)
                })
                .collect();
            container.extend(source, 1.0, items);
        }
        if engines.is_empty() && container.main_results().is_empty() && !index_errors.is_empty() {
            response.error = Some(index_errors.join("; "));
        }

        if !official_tasks.is_empty() {
            // It started with Wikipedia's answer; give it a little more time,
            // never past the search deadline.
            let wait =
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(
                        self.settings.search.official_sites_wait_ms,
                    ));
            let found = tokio::time::timeout(wait, official_tasks.join_next()).await;
            if let Ok(Some(Ok(found))) = found {
                let items: Vec<EngineItem> = found.into_iter().map(EngineItem::Result).collect();
                response.engines.push("official site".to_string());
                container.extend(
                    "official site",
                    self.settings.search.official_sites_weight,
                    items,
                );
            } else {
                official_tasks.abort_all();
            }
        }

        container.close(
            |engine| self.registry.weight_of(engine),
            &query_terms,
            relevance_weight,
            &query_text,
            authority_weight,
        );

        let mut results = container.ordered().to_vec();
        for result in &mut results {
            self.plugins.on_result(
                result,
                &context,
                &request.enabled_plugins,
                &request.disabled_plugins,
            );
        }

        // Local-only searches never contact an embeddings service.
        if !engines.is_empty()
            && !request.is_single_source()
            && !self.settings.search.embedding_url.is_empty()
        {
            results = self.rerank_by_embedding(&request.query, results).await;
        }

        // Site-scoped second pass: when the query names a known site but that
        // site barely shows up, search within it explicitly. Metasearch engines
        // answer "python dict get method" with a tutorial page; asking for a
        // site-scoped query surfaces the reference page itself.
        if !request.site_retry_done {
            results = self.site_scoped_retry(&request, results).await;
        }

        crate::results::filter_results(
            &mut results,
            &request.include_domains,
            &request.exclude_domains,
            request.max_results,
        );
        if request.include_raw_content {
            self.attach_raw_content(&mut results, &request.accept_language)
                .await;
        }

        response.number_of_results = container.number_of_results();
        response.paging = container.paging;
        response.answers = container.answers.clone();
        response.corrections = container.corrections.iter().cloned().collect();
        response.infoboxes = container.infoboxes.clone();
        response.suggestions = container.suggestions.iter().cloned().collect();
        response.unresponsive_engines = container.unresponsive_engines.clone();
        response.timings = container.timings.clone();
        response.engine_data = container.engine_data.clone();
        response.results = results;

        if request.query.redirect_to_first_result {
            if let Some(first) = response.results.first() {
                response.redirect_url = Some(first.url.clone());
            }
        }

        self.plugins.post_search(
            &mut response,
            &context,
            &request.enabled_plugins,
            &request.disabled_plugins,
        );
        // Upstream answerers (statistics, random, weather) answer in front of
        // the results; they run after the plugins like upstream's answerers.
        let answer_context = crate::answerers::AnswerContext {
            locale: context.locale.clone(),
            base_url: context.base_url.clone(),
            network: (!engines.is_empty()).then(|| Arc::clone(&self.network)),
        };
        let answers = crate::answerers::ask(
            &crate::answerers::all(),
            &response.search_query,
            &answer_context,
        )
        .await;
        response.answers.extend(answers);
        Metrics::incr(&self.metrics.results, response.results.len() as u64);
        response
    }

    /// Start the official-site lookup for the articles Wikipedia returned.
    fn start_official_lookup(
        &self,
        request: &SearchRequest,
        outcome: &EngineOutcome,
        tasks: &mut tokio::task::JoinSet<Vec<SearchResult>>,
    ) {
        if !self.settings.search.official_sites
            || !request.is_general_search()
            || outcome.engine != "wikipedia"
            || !tasks.is_empty()
        {
            return;
        }
        let Ok(items) = &outcome.items else {
            return;
        };
        let articles: Vec<crate::official::Article> = items
            .iter()
            .filter_map(|item| match item {
                EngineItem::Result(result) => crate::official::article_of(&result.url),
                _ => None,
            })
            .take(crate::official::MAX_ARTICLES)
            .collect();
        if articles.is_empty() {
            return;
        }
        let network = Arc::clone(&self.network);
        let kinds = crate::official::kinds_for(&request.query.query_text());
        let locale = request.language.clone();
        let cache = Arc::clone(&self.cache);
        let reader = self.page_reader.clone();
        tasks.spawn(async move {
            crate::official::lookup_cached(
                &network,
                &cache,
                &reader,
                &articles,
                &kinds,
                OFFICIAL_SITE_BUDGET,
                &locale,
            )
            .await
        });
    }

    async fn run_engine(
        &self,
        engine: &EngineHandle,
        request: &SearchRequest,
        deadline: Instant,
    ) -> EngineOutcome {
        let name = engine.name().to_string();
        let weight = engine.weight();

        if let Some((remaining, reason)) = self.states.suspension(&name) {
            let kind = failure_kind_from_error_type(reason.as_deref().unwrap_or("suspended"));
            tracing::debug!(engine = %name, remaining = ?remaining, "engine suspended, skipping");
            return EngineOutcome {
                engine: name,
                weight,
                items: Err(kind),
                elapsed: Duration::ZERO,
                suspended: true,
            };
        }

        let started = Instant::now();
        // engine ごとの timeout は残り時間で上限を切る。締め切りを過ぎていれば走らせない。
        let Some(timeout) = engine_budget(engine.timeout(), deadline, Instant::now()) else {
            return EngineOutcome {
                engine: name,
                weight,
                items: Err(FailureKind::Timeout),
                elapsed: Duration::ZERO,
                suspended: false,
            };
        };

        Metrics::incr(&self.metrics.engine_requests, 1);
        let items = self.run_engine_inner(engine, request, timeout).await;
        let elapsed = started.elapsed();

        match &items {
            Ok(items) => {
                self.states
                    .record_success(&name, items.len(), elapsed, &self.settings.search)
            }
            Err(kind) => {
                Metrics::incr(&self.metrics.engine_errors, 1);
                if *kind == FailureKind::Timeout {
                    Metrics::incr(&self.metrics.engine_timeouts, 1);
                }
                self.states
                    .record_failure(&name, *kind, &self.settings.search);
            }
        }

        EngineOutcome {
            engine: name,
            weight,
            items,
            elapsed,
            suspended: false,
        }
    }

    async fn run_engine_inner(
        &self,
        engine: &EngineHandle,
        request: &SearchRequest,
        timeout: Duration,
    ) -> Result<Vec<EngineItem>, FailureKind> {
        // Database backed engines do their work on a blocking thread; the HTTP
        // path below only applies to network engines.
        if engine.is_offline() {
            let handle = Arc::clone(engine);
            let query = request.query.clone();
            let pageno = request.pageno;
            let joined = tokio::time::timeout(
                timeout,
                tokio::task::spawn_blocking(move || handle.search_offline(&query, pageno)),
            )
            .await;
            return match joined {
                Ok(Ok(Ok(items))) => check_quality(engine.name(), &request.query, items),
                Ok(Ok(Err(error))) => Err(FailureKind::from(&error)),
                Ok(Err(_)) => Err(FailureKind::Other),
                Err(_) => Err(FailureKind::Timeout),
            };
        }
        let engine_request = engine
            .build_request(&request.query, request.pageno)
            .map_err(|error| FailureKind::from(&error))?;
        let response = self
            .network
            .fetch(engine_request, timeout, Some(&self.cache), engine.proxy())
            .await
            .map_err(|error| {
                tracing::debug!(engine = %engine.name(), %error, "engine request failed");
                error.failure_kind()
            })?;
        let items = engine.parse(&response, &request.query).map_err(|error| {
            tracing::debug!(engine = %engine.name(), %error, "engine response rejected");
            FailureKind::from(&error)
        })?;
        check_quality(engine.name(), &request.query, items)
    }

    /// Start engines tier by tier and stop once enough results were collected.
    ///
    /// SearXNG issue #6825: querying every engine at once is what makes many
    /// engines suspend together. The staged strategy starts a smaller tier,
    /// checks whether the results are already enough and only then continues.
    async fn run_staged(
        &self,
        engines: &[EngineHandle],
        request: &SearchRequest,
        deadline: Instant,
    ) -> Vec<EngineOutcome> {
        let tier = self.settings.search.staged_tier.max(1);
        let target = self.settings.search.staged_min_results.max(1);
        let ordered = self.order_engines_by_health(engines);
        let mut outcomes = Vec::new();
        let mut collected = 0usize;
        for chunk in ordered.chunks(tier) {
            if Instant::now() >= deadline {
                break;
            }
            let tier_outcomes = futures::future::join_all(
                chunk
                    .iter()
                    .map(|engine| self.run_engine(engine, request, deadline)),
            )
            .await;
            for outcome in &tier_outcomes {
                if let Ok(items) = &outcome.items {
                    collected += items.len();
                }
            }
            outcomes.extend(tier_outcomes);
            if collected >= target {
                break;
            }
        }
        outcomes
    }

    /// Order engines for the staged strategy.
    ///
    /// Proven engines (requests with results) go first, fastest first; engines
    /// that have not been tried yet keep their registry order; engines that
    /// failed or are suspended go last.
    fn order_engines_by_health(&self, engines: &[EngineHandle]) -> Vec<EngineHandle> {
        use std::collections::HashMap;
        let mut ranks: HashMap<String, (u8, u64)> = HashMap::new();
        for snapshot in self.states.snapshot() {
            let stats = &snapshot.stats;
            let rank = if snapshot.suspended {
                3
            } else if stats.requests == 0 {
                1
            } else if stats.results > 0 {
                0
            } else {
                2
            };
            let millis = if stats.requests == 0 {
                0
            } else {
                (stats.average_time() * 1000.0) as u64
            };
            ranks.insert(snapshot.engine, (rank, millis));
        }
        let mut ordered: Vec<EngineHandle> = engines.to_vec();
        ordered.sort_by_key(|engine| ranks.get(engine.name()).copied().unwrap_or((1, 0)));
        ordered
    }

    /// Fetch readable page text for the first few results (raw content).
    /// Run a site-scoped search when the site named in the query is missing
    /// from the first pass, and splice its best pages into the results.
    async fn site_scoped_retry(
        &self,
        request: &SearchRequest,
        mut results: Vec<SearchResult>,
    ) -> Vec<SearchResult> {
        if !self.settings.search.site_scoped_retry || !request.is_general_search() {
            return results;
        }
        let text = request.query.query_text();
        let Some(host) = crate::results::canonical_host_for_query(&text) else {
            return results;
        };
        // Only the top of the list matters: a docs page buried at rank 40 does
        // not answer the query, even if the site is technically present.
        let present = results
            .iter()
            .take(3)
            .filter(|result| {
                crate::results::host_of(&result.url)
                    .is_some_and(|h| h == host || h.ends_with(&format!(".{host}")))
            })
            .count();
        if present >= 2 {
            return results;
        }

        let scoped = format!("site:{host} {text}");
        let mut second = request.clone();
        second.query = request.query.with_query(&scoped);
        second.site_retry_done = true;
        second.timeout_limit = Some(
            request
                .timeout_limit
                .unwrap_or(self.settings.search.timeout_limit)
                .min(2.0),
        );
        second.include_raw_content = false;
        second.max_results = None;
        let response = Box::pin(self.search(second)).await;
        tracing::debug!(
            host,
            present,
            second_results = response.results.len(),
            "site-scoped second pass ran"
        );

        let reference = results.first().map(|result| result.score).unwrap_or(1.0);
        let terms = crate::results::query_terms(&text);
        // Pick the pages of this site that best match the original query, not
        // merely the ones the scoped pass happened to rank first.
        let mut candidates: Vec<SearchResult> = response
            .results
            .into_iter()
            .filter(|candidate| {
                crate::results::host_of(&candidate.url)
                    .is_some_and(|h| h == host || h.ends_with(&format!(".{host}")))
            })
            .collect();
        candidates.sort_by(|a, b| {
            crate::results::relevance_score(b, &terms)
                .partial_cmp(&crate::results::relevance_score(a, &terms))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut added = 0usize;
        let mut by_key: std::collections::HashMap<String, usize> = results
            .iter()
            .enumerate()
            .map(|(index, result)| (crate::results::dedup_key(&result.url), index))
            .collect();
        for candidate in candidates {
            if added >= 3 {
                break;
            }
            let key = crate::results::dedup_key(&candidate.url);
            if let Some(index) = by_key.get(&key).copied() {
                results[index].score = results[index].score.max(reference * 0.95);
                continue;
            }
            let mut promoted = candidate;
            promoted.score = reference * (0.95 - 0.10 * added as f64);
            by_key.insert(key, results.len());
            results.push(promoted);
            added += 1;
        }
        if added > 0 {
            tracing::debug!(host, added, "site-scoped second pass added pages");
            results.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        results
    }

    /// Re-rank the top results by cosine similarity between the query and each
    /// result's title + snippet, using an OpenAI-compatible embeddings endpoint.
    ///
    /// Returns the input unchanged when the endpoint fails (degrades to the
    /// position + relevance ranking).
    async fn rerank_by_embedding(
        &self,
        query: &Query,
        mut results: Vec<SearchResult>,
    ) -> Vec<SearchResult> {
        let settings = &self.settings.search;
        let topk = results.len().min(settings.embedding_topk);
        if topk < 2 {
            return results;
        }
        let mut texts = vec![query.query_text()];
        for result in &results[..topk] {
            let snippet: String = result.content.chars().take(240).collect();
            texts.push(format!("{} {snippet}", result.title));
        }
        let vectors = match self.embed(&texts).await {
            Ok(vectors) if vectors.len() == texts.len() => vectors,
            Ok(_) | Err(_) => {
                tracing::warn!("embedding re-ranking skipped (bad response)");
                return results;
            }
        };
        let query_vec = &vectors[0];
        for (index, result) in results.iter_mut().take(topk).enumerate() {
            let cosine = dot(query_vec, &vectors[index + 1]);
            result.score += settings.embedding_weight * cosine;
        }
        results[..topk].sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    /// Ask the embeddings endpoint for one vector per input text.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let base = self.settings.search.embedding_url.trim_end_matches('/');
        let url = if base.ends_with("/embeddings") {
            base.to_string()
        } else if base.ends_with("/v1") {
            format!("{base}/embeddings")
        } else {
            format!("{base}/v1/embeddings")
        };
        let body = serde_json::json!({
            "model": self.settings.search.embedding_model,
            "input": texts,
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| error.to_string())?;
        let response = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let payload: serde_json::Value =
            response.json().await.map_err(|error| error.to_string())?;
        if !status.is_success() {
            return Err(format!("embeddings http {}", status.as_u16()));
        }
        let mut data = payload
            .get("data")
            .and_then(|value| value.as_array())
            .cloned()
            .ok_or_else(|| "embeddings: missing data".to_string())?;
        data.sort_by_key(|item| item.get("index").and_then(|v| v.as_u64()).unwrap_or(0));
        data.iter()
            .map(|item| {
                item.get("embedding")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_f64())
                            .map(|v| v as f32)
                            .collect()
                    })
                    .ok_or_else(|| "embeddings: missing vector".to_string())
            })
            .collect()
    }

    async fn attach_raw_content(
        &self,
        results: &mut [SearchResult],
        language: &crate::fetch::AcceptLanguage,
    ) {
        let limit = self.settings.search.raw_content_limit.max(1);
        let count = results.len().min(limit);
        if count == 0 {
            return;
        }
        let texts = futures::future::join_all(
            results
                .iter()
                .take(count)
                .map(|result| self.page_text(&result.url, language)),
        )
        .await;
        for (result, page) in results.iter_mut().take(count).zip(texts) {
            if let Some(page) = page {
                result.raw_content = Some(page.text.to_string());
                result.raw_content_truncated = Some(page.source_truncated);
            }
        }
    }

    /// Optional page reads use the same DNS, redirect, body and concurrency
    /// checks as MCP. Unchecked engine-response cache entries are not reused.
    async fn page_text(
        &self,
        url: &str,
        language: &crate::fetch::AcceptLanguage,
    ) -> Option<crate::fetch::PageContent> {
        match self
            .page_reader
            .read_with_language(url, self.settings.search.raw_content_chars.max(1), language)
            .await
        {
            Ok(page) => Some(page),
            Err(error) => {
                tracing::debug!(url, %error, "raw content unavailable");
                None
            }
        }
    }

    /// Engine statistics for `/stats`.
    pub fn engine_snapshots(&self) -> Vec<EngineSnapshot> {
        self.states.snapshot()
    }
}

/// Reject a parsed response that looks fabricated or unrelated.
///
/// Upstream SearXNG issue #6358: a blocked upstream can answer with HTTP 200
/// and placeholder content that still parses. Dropping such an engine is
/// better than poisoning the merged result set.
fn check_quality(
    engine: &str,
    query: &crate::query::Query,
    items: Vec<crate::engine::EngineItem>,
) -> Result<Vec<crate::engine::EngineItem>, FailureKind> {
    if let Some(suspicion) = crate::quality::inspect(query, &items) {
        tracing::warn!(
            %engine,
            reason = suspicion.reason(),
            "discarding a suspicious engine response"
        );
        return Err(FailureKind::Suspicious);
    }
    Ok(items)
}

/// `[{"answer": "..."}]`, the shape upstream uses for answers.
fn serialize_answers<S>(answers: &[String], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;
    #[derive(Serialize)]
    struct Answer<'a> {
        answer: &'a str,
    }
    answers
        .iter()
        .map(|answer| Answer { answer })
        .collect::<Vec<_>>()
        .serialize(serializer)
}

/// `[[engine, "Translated error"]]`, the shape upstream uses.
/// The label shown for an engine that did not answer (JSON, feeds and HTML).
pub fn unresponsive_text(error_type: &str, suspended: bool) -> String {
    let mut text = match error_type {
        "timeout" => "Timeout".to_string(),
        "access denied" => "Access denied".to_string(),
        "captcha" => "CAPTCHA".to_string(),
        "too many requests" => "Too many requests".to_string(),
        "http error" => "HTTP error".to_string(),
        "parsing error" => "Parsing error".to_string(),
        "suspended" => "Suspended".to_string(),
        "resting" => "Resting: no results lately".to_string(),
        _ => "Unexpected error".to_string(),
    };
    if suspended && text != "Suspended" && error_type != "resting" {
        text = format!("Suspended: {text}");
    }
    text
}

fn serialize_unresponsive<S>(
    engines: &[UnresponsiveEngine],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;
    let mut pairs: Vec<(String, String)> = engines
        .iter()
        .map(|engine| {
            let text = unresponsive_text(&engine.error_type, engine.suspended);
            (engine.engine.clone(), text)
        })
        .collect();
    pairs.sort();
    pairs.serialize(serializer)
}

/// Cosine similarity of two equally sized vectors.
fn dot(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (x, y) in a.iter().zip(b) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

fn failure_kind_from_error_type(error_type: &str) -> FailureKind {
    match error_type {
        "timeout" => FailureKind::Timeout,
        "access denied" => FailureKind::AccessDenied,
        "captcha" => FailureKind::Captcha,
        "too many requests" => FailureKind::TooManyRequests,
        "http error" => FailureKind::Http,
        "parsing error" => FailureKind::Parse,
        "suspicious response" => FailureKind::Suspicious,
        "resting" => FailureKind::Resting,
        _ => FailureKind::Other,
    }
}

/// How long an engine may run, given the whole-search deadline.
///
/// The engine's own timeout is the ceiling; the search deadline may cut it short.
/// Returns `None` when the deadline has already passed (do not run at all).
fn engine_budget(engine_timeout: Duration, deadline: Instant, now: Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        return None;
    }
    Some(engine_timeout.min(remaining))
}

#[cfg(test)]
mod orchestration_tests {
    use super::*;
    use crate::engine::{Engine, EngineError, EngineMeta, EngineRequest, EngineResponse};

    type ApplicabilityPredicate = Box<dyn Fn(&Query) -> bool + Send + Sync>;

    /// An engine that never touches the network: it either fails while
    /// building the request or returns synthetic results.
    struct FakeEngine {
        meta: EngineMeta,
        results: usize,
        build_error: bool,
        queries: std::sync::Mutex<Vec<Query>>,
        applicable: Option<ApplicabilityPredicate>,
    }

    impl FakeEngine {
        fn build(name: &str, categories: &[&str], results: usize, build_error: bool) -> Arc<Self> {
            Self::build_with_predicate(name, categories, results, build_error, None)
        }

        fn build_with_predicate(
            name: &str,
            categories: &[&str],
            results: usize,
            build_error: bool,
            applicable: Option<ApplicabilityPredicate>,
        ) -> Arc<Self> {
            let config = crate::settings::EngineConfig::new(name, "fake", name, categories);
            let mut meta = EngineMeta::from_config(
                &config,
                crate::engine::EngineDefaults {
                    categories: &["general"],
                    shortcut: "fake",
                    ..Default::default()
                },
            );
            meta.categories = categories.iter().map(|c| (*c).to_string()).collect();
            std::sync::Arc::new(Self {
                meta,
                results,
                build_error,
                queries: std::sync::Mutex::new(Vec::new()),
                applicable,
            })
        }
    }

    impl Engine for FakeEngine {
        fn meta(&self) -> &EngineMeta {
            &self.meta
        }
        fn is_applicable(&self, query: &Query) -> bool {
            self.applicable
                .as_ref()
                .map(|predicate| predicate(query))
                .unwrap_or(true)
        }
        fn is_offline(&self) -> bool {
            true
        }
        fn search_offline(
            &self,
            query: &Query,
            pageno: u32,
        ) -> Result<Vec<EngineItem>, EngineError> {
            self.build_request(query, pageno)?;
            self.parse(
                &EngineResponse::new(200, "https://fake.invalid/", Vec::new()),
                query,
            )
        }
        fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
            self.queries.lock().unwrap().push(query.clone());
            if self.build_error {
                return Err(EngineError::Message("boom".into()));
            }
            Ok(EngineRequest::get(format!(
                "https://fake.invalid/?q={}",
                crate::engine::util::encode(&query.query_text())
            )))
        }
        fn parse(
            &self,
            _response: &EngineResponse,
            _query: &Query,
        ) -> Result<Vec<EngineItem>, EngineError> {
            Ok((0..self.results)
                .map(|index| {
                    EngineItem::Result(SearchResult::new(
                        format!("https://example.com/{}/{index}", self.meta.name),
                        format!("{} result {index}", self.meta.name),
                    ))
                })
                .collect())
        }
    }

    fn searcher(engines: Vec<EngineHandle>) -> Searcher {
        let settings = Settings::defaults();
        let mut registry = EngineRegistry::default();
        for handle in engines {
            registry.push(handle);
        }
        let network = Network::from_settings(&settings).unwrap();
        Searcher {
            page_reader: crate::fetch::PageReader::new(&settings.outgoing, &[]).unwrap(),
            local_index: None,
            wiki_index: None,
            plugins: Arc::new(PluginManager::new()),
            settings: Arc::new(settings),
            registry: Arc::new(registry),
            network: Arc::new(network),
            cache: Arc::new(Cache::default()),
            states: Arc::new(EngineStates::new()),
            metrics: Arc::new(Metrics::new()),
        }
    }

    fn query(text: &str, registry: &EngineRegistry) -> Query {
        Query::parse(text, registry, &[])
    }

    fn searcher_for_routes(engine: EngineHandle) -> Searcher {
        searcher(vec![engine])
    }

    fn with_local_index(engines: Vec<EngineHandle>) -> (tempfile::TempDir, Searcher) {
        let directory = tempfile::tempdir().unwrap();
        let index = crate::localindex::LocalIndex::open_for_write(directory.path()).unwrap();
        index
            .add(&crate::localindex::Page {
                url: "https://docs.example/needle",
                title: "Needle",
                body: "The needle reference page.",
                lang: "en",
                snippet: "",
            })
            .unwrap();
        index.commit().unwrap();
        let mut searcher = searcher(engines);
        searcher.local_index = Some(Arc::new(index));
        (directory, searcher)
    }

    #[tokio::test]
    async fn a_local_index_can_search_without_remote_engines() {
        let (_directory, searcher) = with_local_index(vec![]);
        let response = searcher
            .search(SearchRequest::new(query("needle", &searcher.registry)))
            .await;
        assert!(response.error.is_none());
        assert_eq!(response.engines, ["local_index"]);
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].url, "https://docs.example/needle");
    }

    #[tokio::test]
    async fn local_search_waits_for_capacity_only_until_its_deadline() {
        let (_directory, searcher) = with_local_index(vec![]);
        let occupied = Arc::clone(&INDEX_SEARCH_SLOTS)
            .acquire_many_owned(4)
            .await
            .unwrap();
        let deadline = Instant::now() - INDEX_GRACE + Duration::from_millis(20);
        let error = bounded_index_search(
            searcher.local_index.as_ref().unwrap(),
            "needle".into(),
            false,
            Vec::new(),
            Vec::new(),
            1,
            deadline,
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, FailureKind::Timeout);
        drop(occupied);
        let hits = bounded_index_search(
            searcher.local_index.as_ref().unwrap(),
            "needle".into(),
            false,
            Vec::new(),
            Vec::new(),
            1,
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .map_err(|error| error.message)
        .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn local_pages_are_disjoint_and_out_of_range_errors_are_visible() {
        let (_directory, searcher) = with_local_index(vec![]);
        let index = searcher.local_index.as_ref().unwrap();
        for number in 1..45 {
            index
                .add(&crate::localindex::Page {
                    url: &format!("https://docs.example/needle/{number}"),
                    title: "Needle reference",
                    body: "The needle reference page.",
                    lang: "en",
                    snippet: "",
                })
                .unwrap();
        }
        index.commit().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for (page, expected, more) in [(1, 20, true), (2, 20, true), (3, 5, false)] {
            let mut request = SearchRequest::new(query("!local_index needle", &searcher.registry));
            request.pageno = page;
            let response = searcher.search(request).await;
            assert!(response.error.is_none());
            assert_eq!(response.results.len(), expected);
            assert_eq!(response.paging, more);
            for result in response.results {
                assert!(seen.insert(result.url), "page {page} repeated a result");
            }
        }
        let mut request = SearchRequest::new(query("!local_index needle", &searcher.registry));
        request.pageno = u32::MAX;
        let response = searcher.search(request).await;
        assert!(response.error.as_deref().unwrap().contains("window"));
        assert_eq!(response.unresponsive_engines.len(), 1);
        assert_eq!(response.unresponsive_engines[0].engine, "local_index");
        assert!(!response.paging);
    }

    #[tokio::test]
    async fn local_source_selection_never_falls_back_to_remote_engines() {
        let remote = FakeEngine::build("remote", &["general"], 1, false);
        let (_directory, mut searcher) = with_local_index(vec![remote.clone()]);
        Arc::make_mut(&mut searcher.settings)
            .search
            .local_index_merge = false;
        for bang in [false, true] {
            let text = if bang {
                "!local_index needle"
            } else {
                "needle"
            };
            let mut request = SearchRequest::new(query(text, &searcher.registry));
            if !bang {
                request.engines = vec!["LOCAL_INDEX".to_string()];
            }
            let response = searcher.search(request).await;
            assert!(response.error.is_none());
            assert_eq!(response.engines, ["local_index"]);
            assert_eq!(response.results.len(), 1);
        }
        assert!(remote.queries.lock().unwrap().is_empty());

        searcher.local_index = None;
        let request = SearchRequest::new(query("!local_index needle", &searcher.registry));
        let response = searcher.search(request).await;
        assert!(response.error.is_some());
        assert!(remote.queries.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn combining_local_indexes_does_not_contact_embeddings() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let app = axum::Router::new().route(
            "/v1/embeddings",
            axum::routing::post(move || {
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { axum::Json(serde_json::json!({"data": []})) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (_directory, mut searcher) = with_local_index(vec![]);
        let index = searcher.local_index.as_ref().unwrap();
        index
            .add(&crate::localindex::Page {
                url: "https://docs.example/other",
                title: "Needle notes",
                body: "More notes about the needle reference.",
                lang: "en",
                snippet: "",
            })
            .unwrap();
        index.commit().unwrap();
        searcher.wiki_index = Some(Arc::clone(index));
        Arc::make_mut(&mut searcher.settings).search.embedding_url = format!("http://{address}");
        let mut request = SearchRequest::new(query("needle", &searcher.registry));
        request.engines = vec!["local_index".into(), "wikipedia_index".into()];
        let response = searcher.search(request).await;
        server.abort();
        assert_eq!(response.results.len(), 2);
        assert_eq!(response.engines, ["local_index", "wikipedia_index"]);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn explicit_remote_sources_do_not_merge_unrequested_indexes() {
        let first = FakeEngine::build("first", &["general"], 1, false);
        let second = FakeEngine::build("second", &["general"], 1, false);
        let (_directory, searcher) = with_local_index(vec![first, second]);
        let mut request = SearchRequest::new(query("needle", &searcher.registry));
        request.engines = vec!["first".to_string(), "second".to_string()];
        let response = searcher.search(request).await;
        assert_eq!(response.engines, ["first", "second"]);
        assert_eq!(response.results.len(), 2);
        assert!(
            response
                .results
                .iter()
                .all(|result| result.engine != "local_index")
        );
    }

    #[tokio::test]
    async fn engine_bangs_preserve_source_order_and_exclude_local_results() {
        let remote = FakeEngine::build("remote", &["general"], 2, false);
        let (_directory, searcher) = with_local_index(vec![remote]);
        let request = SearchRequest::new(query("!remote needle", &searcher.registry));
        let response = searcher.search(request).await;
        assert_eq!(response.engines, ["remote"]);
        assert_eq!(response.results.len(), 2);
        assert_eq!(response.results[0].score, 1.0);
        assert_eq!(response.results[1].score, 0.5);
    }

    #[tokio::test]
    async fn category_searches_do_not_merge_general_indexes() {
        let images = FakeEngine::build("images", &["images"], 1, false);
        let (_directory, searcher) = with_local_index(vec![images]);
        let mut request = SearchRequest::new(query("needle", &searcher.registry));
        request.categories = vec!["images".to_string()];
        let response = searcher.search(request).await;
        assert_eq!(response.engines, ["images"]);
        assert_eq!(response.results.len(), 1);
    }

    #[tokio::test]
    async fn a_routed_source_gets_its_requested_weight() {
        let web = FakeEngine::build("web", &["general"], 1, false);
        let arxiv = FakeEngine::build("arxiv", &["science"], 1, false);
        let mut searcher = searcher(vec![web, arxiv]);
        let settings = Arc::make_mut(&mut searcher.settings);
        settings.search.relevance_weight = 0.0;
        settings.search.authority_weight = 0.0;
        let request = SearchRequest::new(query("arxiv needle", &searcher.registry));
        let response = searcher.search(request).await;
        assert_eq!(response.results[0].engine, "arxiv");
        assert_eq!(response.results[0].score, ROUTED_WEIGHT);
        assert_eq!(response.results[1].score, 1.0);
    }

    #[tokio::test]
    async fn official_lookups_are_started_once_and_respect_source_selection() {
        let searcher = searcher(vec![]);
        let outcome = EngineOutcome {
            engine: "wikipedia".to_string(),
            weight: 1.0,
            items: Ok(vec![EngineItem::Result(SearchResult::new(
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
                "Rust",
            ))]),
            elapsed: Duration::ZERO,
            suspended: false,
        };
        let request = SearchRequest::new(query("rust", &searcher.registry));
        let mut tasks = tokio::task::JoinSet::new();
        searcher.start_official_lookup(&request, &outcome, &mut tasks);
        searcher.start_official_lookup(&request, &outcome, &mut tasks);
        assert_eq!(tasks.len(), 1);
        drop(tasks);

        let mut request = request;
        request.engines = vec!["wikipedia".to_string(), "wikidata".to_string()];
        let mut tasks = tokio::task::JoinSet::new();
        searcher.start_official_lookup(&request, &outcome, &mut tasks);
        assert!(tasks.is_empty());
    }

    #[test]
    fn a_named_specialised_source_is_asked_without_its_name() {
        let general = FakeEngine::build("web", &["general"], 1, false);
        let arxiv = FakeEngine::build("arxiv", &["science"], 1, false);
        let steam = FakeEngine::build("steam", &["games"], 1, false);
        let searcher = searcher(vec![general.clone(), arxiv, steam]);
        let selected = vec![general as EngineHandle];

        let request =
            SearchRequest::new(query("attention is all you need arxiv", &searcher.registry));
        let (routed, routed_request) = searcher.site_routes(&request, &selected).unwrap();
        assert_eq!(routed.len(), 1);
        assert_eq!(routed[0].name(), "arxiv");
        assert_eq!(
            routed_request.query.query_text(),
            "attention is all you need"
        );
        // Every other engine still gets the query as typed.
        assert_eq!(
            request.query.query_text(),
            "attention is all you need arxiv"
        );

        // Only developer/science sources are routed, and a lone name is not.
        let crates = FakeEngine::build("crates.io", &["it", "packages"], 1, false);
        let searcher_with_crates = searcher_for_routes(crates);
        let request = SearchRequest::new(query(
            "crates.io serde crate",
            &searcher_with_crates.registry,
        ));
        let (_, routed_request) = searcher_with_crates.site_routes(&request, &[]).unwrap();
        assert_eq!(routed_request.query.query_text(), "serde");

        let request = SearchRequest::new(query("steam engine history", &searcher.registry));
        assert!(searcher.site_routes(&request, &selected).is_none());
        let request = SearchRequest::new(query("arxiv", &searcher.registry));
        assert!(searcher.site_routes(&request, &selected).is_none());
    }

    #[test]
    fn local_index_diversity_keeps_order_and_exempts_single_host_indexes() {
        let hit = |host: &str, number| crate::localindex::Hit {
            url: format!("https://{host}/{number}"),
            title: String::new(),
            snippet: String::new(),
            score: 1.0,
        };
        let same = (0..5).map(|i| hit("docs.example", i)).collect();
        assert_eq!(cap_per_host(same, 3).len(), 5);
        let mut mixed: Vec<_> = (0..5).map(|i| hit("docs.example", i)).collect();
        mixed.push(hit("reference.example", 0));
        let kept = cap_per_host(mixed, 3);
        assert_eq!(kept.len(), 4);
        assert_eq!(kept[0].url, "https://docs.example/0");
        assert_eq!(kept[2].url, "https://docs.example/2");
        assert_eq!(kept[3].url, "https://reference.example/0");
    }

    #[tokio::test]
    async fn raw_content_uses_and_isolates_request_languages() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                loop {
                    let count = stream.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let is_robots = request.starts_with("GET /robots.txt ");
                tx.send(request).unwrap();
                let body = if is_robots {
                    ""
                } else {
                    "<html><body>localized fixture text</body></html>"
                };
                let status = if is_robots { "404 Not Found" } else { "200 OK" };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let mut searcher = searcher(vec![]);
        searcher.page_reader = crate::fetch::PageReader::new(
            &searcher.settings.outgoing,
            &[("example.org".into(), address)],
        )
        .unwrap();
        let url = "http://example.org/article";
        let mut english = vec![SearchResult::new(url, "fixture")];
        searcher
            .attach_raw_content(
                &mut english,
                &crate::fetch::AcceptLanguage::parse("en-US").unwrap(),
            )
            .await;
        assert!(english[0].raw_content.is_some());
        let mut japanese = vec![SearchResult::new(url, "fixture")];
        searcher
            .attach_raw_content(
                &mut japanese,
                &crate::fetch::AcceptLanguage::parse("ja-JP").unwrap(),
            )
            .await;
        assert!(japanese[0].raw_content.is_some());

        let _robots = rx.recv().await.unwrap();
        let english_request = rx.recv().await.unwrap();
        let japanese_request = rx.recv().await.unwrap();
        assert!(
            english_request
                .to_ascii_lowercase()
                .contains("accept-language: en-us")
        );
        assert!(
            japanese_request
                .to_ascii_lowercase()
                .contains("accept-language: ja-jp")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn raw_content_refuses_private_result_urls() {
        let searcher = searcher(vec![]);
        let mut results = vec![SearchResult::new(
            "http://127.0.0.1:9/private",
            "poisoned result",
        )];
        searcher
            .attach_raw_content(&mut results, &crate::fetch::AcceptLanguage::default())
            .await;
        assert!(results[0].raw_content.is_none());
        assert!(results[0].raw_content_truncated.is_none());
    }

    #[tokio::test]
    async fn site_retry_preserves_query_options() {
        let selected = FakeEngine::build("selected", &["general"], 0, true);
        let mut searcher = searcher(vec![selected.clone()]);
        Arc::make_mut(&mut searcher.settings)
            .search
            .site_scoped_retry = true;
        let mut original = query(":ja rust traits", &searcher.registry);
        original.safesearch = 2;
        original.time_range = Some(TimeRange::Week);
        original.locale = "ja-JP".into();
        let request = SearchRequest::new(original.clone());

        searcher.site_scoped_retry(&request, Vec::new()).await;

        let queries = selected.queries.lock().unwrap();
        assert_eq!(queries.len(), 1);
        let retried = &queries[0];
        assert_eq!(retried.query_text(), "site:doc.rust-lang.org rust traits");
        assert_eq!(retried.safesearch, original.safesearch);
        assert_eq!(retried.time_range, original.time_range);
        assert_eq!(retried.locale, original.locale);
        assert_eq!(retried.languages, original.languages);
        assert_eq!(retried.engine_refs, original.engine_refs);
    }

    #[tokio::test]
    async fn site_retry_does_not_expand_explicit_source_queries() {
        let selected = FakeEngine::build("selected", &["general"], 0, true);
        let mut searcher = searcher(vec![selected.clone()]);
        Arc::make_mut(&mut searcher.settings)
            .search
            .site_scoped_retry = true;
        let request = SearchRequest::new(query("!selected rust traits", &searcher.registry));
        searcher.site_scoped_retry(&request, Vec::new()).await;
        assert!(selected.queries.lock().unwrap().is_empty());
    }

    #[test]
    fn engines_of_the_selected_category_are_used() {
        let searcher = searcher(vec![
            FakeEngine::build("web engine", &["general"], 1, true),
            FakeEngine::build("image engine", &["images"], 1, true),
        ]);
        let request = SearchRequest::new(query("cats", &searcher.registry));
        let selected: Vec<String> = searcher
            .select_engines(&request)
            .iter()
            .map(|engine| engine.name().to_string())
            .collect();
        assert_eq!(selected, vec!["web engine".to_string()]);
    }

    #[test]
    fn a_bang_selects_a_single_engine() {
        let searcher = searcher(vec![
            FakeEngine::build("web engine", &["general"], 1, true),
            FakeEngine::build("image engine", &["images"], 1, true),
        ]);
        let registry = searcher.registry.clone();
        let request = SearchRequest::new(query("!image_engine cats", &registry));
        let selected: Vec<String> = searcher
            .select_engines(&request)
            .iter()
            .map(|engine| engine.name().to_string())
            .collect();
        assert_eq!(selected, vec!["image engine".to_string()]);
    }

    /// The engine's own timeout is the ceiling when the deadline is far away.
    #[test]
    fn engine_budget_uses_the_engine_timeout_when_the_deadline_is_far() {
        let now = Instant::now();
        let budget = engine_budget(Duration::from_secs(4), now + Duration::from_secs(30), now);
        assert_eq!(budget, Some(Duration::from_secs(4)));
    }

    /// The search deadline cuts a slow engine short, so one slow engine cannot
    /// hold the whole request open.
    #[test]
    fn engine_budget_is_cut_by_the_search_deadline() {
        let now = Instant::now();
        let budget = engine_budget(
            Duration::from_secs(20),
            now + Duration::from_millis(300),
            now,
        );
        assert_eq!(budget, Some(Duration::from_millis(300)));
    }

    /// A request whose deadline has already passed must not start engines at all.
    /// Before this, a short `timeout_limit` was applied to every engine, so engines
    /// that had already answered in time were also reported as timeouts and their
    /// results were dropped (measured: a 45 ms engine failed under a 100 ms limit).
    #[test]
    fn engine_budget_is_none_after_the_deadline() {
        let now = Instant::now();
        assert_eq!(
            engine_budget(Duration::from_secs(4), now, now + Duration::from_millis(1)),
            None
        );
    }

    #[tokio::test]
    async fn failing_engines_are_reported_as_unresponsive() {
        let searcher = searcher(vec![FakeEngine::build("broken", &["general"], 0, true)]);
        let request = SearchRequest::new(query("cats", &searcher.registry));
        let response = searcher.search(request).await;
        assert!(response.results.is_empty());
        assert_eq!(response.unresponsive_engines.len(), 1);
        assert_eq!(response.unresponsive_engines[0].engine, "broken");
    }

    #[tokio::test]
    async fn inapplicable_engines_are_skipped_without_failure() {
        let searcher = searcher(vec![
            FakeEngine::build_with_predicate(
                "syntax engine",
                &["general"],
                1,
                true,
                Some(Box::new(|query| query.query_text().starts_with("run:"))),
            ),
            FakeEngine::build("web engine", &["general"], 0, true),
        ]);
        let request = SearchRequest::new(query("ordinary query", &searcher.registry));
        let response = searcher.search(request).await;
        assert!(!response.engines.contains(&"syntax engine".to_string()));
        assert!(
            response
                .unresponsive_engines
                .iter()
                .all(|engine| engine.engine != "syntax engine")
        );
    }

    #[tokio::test]
    async fn matching_syntax_keeps_the_engine_in_the_run_set() {
        let searcher = searcher(vec![FakeEngine::build_with_predicate(
            "syntax engine",
            &["general"],
            0,
            true,
            Some(Box::new(|query| query.query_text().starts_with("run:"))),
        )]);
        let request = SearchRequest::new(query("run: value", &searcher.registry));
        let response = searcher.search(request).await;
        assert_eq!(response.engines, vec!["syntax engine".to_string()]);
        assert_eq!(response.unresponsive_engines.len(), 1);
    }

    #[tokio::test]
    async fn no_engine_selected_is_an_error() {
        let searcher = searcher(vec![FakeEngine::build(
            "image engine",
            &["images"],
            1,
            true,
        )]);
        let mut request = SearchRequest::new(query("cats", &searcher.registry));
        request.categories = vec!["videos".to_string()];
        let response = searcher.search(request).await;
        assert!(response.error.is_some());
    }

    #[tokio::test]
    async fn external_bangs_redirect() {
        let searcher = searcher(vec![FakeEngine::build("web engine", &["general"], 1, true)]);
        let request = SearchRequest::new(query("!!w cats", &searcher.registry));
        let response = searcher.search(request).await;
        assert_eq!(
            response.redirect_url.as_deref(),
            Some("https://en.wikipedia.org/wiki/Special:Search?search=cats")
        );
    }

    #[tokio::test]
    async fn suspended_engines_are_skipped() {
        let searcher = searcher(vec![FakeEngine::build("broken", &["general"], 0, true)]);
        searcher.states.record_failure(
            "broken",
            FailureKind::TooManyRequests,
            &searcher.settings.search,
        );
        let request = SearchRequest::new(query("cats", &searcher.registry));
        let response = searcher.search(request).await;
        assert_eq!(response.unresponsive_engines.len(), 1);
        assert!(response.unresponsive_engines[0].suspended);
        assert_eq!(
            response.unresponsive_engines[0].error_type,
            "too many requests"
        );
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn unresponsive_labels_distinguish_resting_from_suspended() {
        assert_eq!(
            unresponsive_text("resting", true),
            "Resting: no results lately"
        );
        assert_eq!(
            unresponsive_text("parsing error", true),
            "Suspended: Parsing error"
        );
        assert_eq!(unresponsive_text("timeout", false), "Timeout");
        assert_eq!(
            unresponsive_text("anything else", false),
            "Unexpected error"
        );
    }

    use super::*;

    #[test]
    fn cosine_similarity_behaves_as_expected() {
        assert!((dot(&[1.0f32, 0.0], &[1.0f32, 0.0]) - 1.0).abs() < 1e-6);
        assert!((dot(&[1.0f32, 0.0], &[0.0f32, 1.0])).abs() < 1e-6);
        assert_eq!(dot(&[], &[1.0]), 0.0);
        assert_eq!(dot(&[1.0f32], &[1.0, 0.0]), 0.0);
    }
}
