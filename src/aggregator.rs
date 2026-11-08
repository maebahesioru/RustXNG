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
use crate::network::{Network, SolverAttempt};
use crate::plugins::{PluginContext, PluginManager, UserInfo};
use crate::query::{Query, TimeRange};
use crate::results::{Infobox, ResultContainer, SearchResult, Timing, UnresponsiveEngine};
use crate::settings::Settings;

/// How much of the deadline must be left before an engine is allowed to spend
/// a solver tab. A tab needs about three seconds to load a walled page and hand
/// the results back; below this an attempt can only time out, so the engine
/// reports its own failure instead and the search keeps its speed.
const SOLVER_MIN_BUDGET: Duration = Duration::from_millis(3_500);

/// The same floor for a search that names its engines.
///
/// A named engine is already being waited for, so a wall that takes a second to
/// answer still leaves a tab worth taking. yandex answers its challenge page
/// about 1.5s into a 4s budget: the general floor rejected that before the
/// browser was ever asked, which is why every bang search reported "Solver
/// busy" while the same solver cleared the same page moments earlier.
const SOLVER_MIN_BUDGET_EXPLICIT: Duration = Duration::from_millis(1_200);

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
    /// How the selected engines are started.
    pub strategy: crate::settings::SearchStrategy,
    /// Set once the site-scoped second pass already ran, so it cannot recurse.
    pub site_retry_done: bool,
    /// Fail the request when explicit engine names do not exist, instead of
    /// SearXNG-compatible silent fallback to the default set.
    pub strict_engines: bool,
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
            strategy: crate::settings::SearchStrategy::Parallel,
            site_retry_done: false,
            strict_engines: false,
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
}

/// Open the configured local index, if any.
fn open_local_index(settings: &Settings) -> Option<Arc<crate::localindex::LocalIndex>> {
    let path = settings.search.local_index_path.trim();
    if path.is_empty() {
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
        let explicit: Vec<String> = if !request.engines.is_empty() {
            request.engines.clone()
        } else {
            request.query.engine_ref_names()
        };
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
                if request.disabled_engines.contains(&name) {
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
        if selected.is_empty() && !explicit.is_empty() {
            return run(&[]);
        }
        selected
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
        if request.strict_engines {
            let explicit = request.explicit_engines();
            let unknown: Vec<&str> = explicit
                .iter()
                .filter(|name| self.registry.get(name).is_none())
                .map(String::as_str)
                .collect();
            if !unknown.is_empty() {
                response.error = Some(format!("unknown engines: {}", unknown.join(", "),));
                return response;
            }
        }
        if engines.is_empty() {
            response.error = Some("no engine selected for this request".to_string());
            return response;
        }
        // Some upstream online processors only apply when the query carries
        // their own syntax (for example currency conversion or a translation
        // language pair). Skip those engines before requests and failure
        // accounting instead of reporting an unrelated general query as an
        // engine failure.
        engines.retain(|engine| engine.is_applicable(&request.query));

        // 全 engine の完了を待つ（join_all）。
        //
        // 以前はここで timeout_limit を engine ごとに一律適用していたため、
        // **速い engine が締め切りに巻き込まれて結果を捨てていた**（実測: 45ms で
        // 終わる engine が 102ms の締め切りで Timeout 扱いになり 0 件）。
        // 締め切りは「検索全体」に対してかけ、完了済みの結果は必ず活かす。
        let deadline = Instant::now()
            + Duration::from_secs_f64(
                request
                    .timeout_limit
                    .unwrap_or(self.settings.search.timeout_limit)
                    .max(0.1),
            );
        let outcomes = match request.strategy {
            crate::settings::SearchStrategy::Parallel => {
                futures::future::join_all(
                    engines
                        .iter()
                        .map(|engine| self.run_engine(engine, &request, deadline)),
                )
                .await
            }
            crate::settings::SearchStrategy::Staged => {
                self.run_staged(&engines, &request, deadline).await
            }
        };

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
        if deep_pages > 0 && request.engines.len() != 1 && Instant::now() < deadline {
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
                deeper.pageno = request.pageno + extra;
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
        let relevance_weight = if request.engines.len() == 1 {
            0.0
        } else {
            self.settings.search.relevance_weight
        };
        // Canonical-site boost: naming a site means "that site's own page".
        let query_text = request.query.query_text();
        let authority_weight = if request.engines.len() == 1 {
            0.0
        } else {
            self.settings.search.authority_weight
        };
        // The local index is just another source of pages.
        if self.settings.search.local_index_merge {
            if let Some(index) = self.local_index.as_ref() {
                if request.engines.len() != 1 {
                    let text = request.query.query_text();
                    if let Ok(hits) = index.search(&text, 20) {
                        let items: Vec<EngineItem> = hits
                            .into_iter()
                            .map(|hit| {
                                let mut result = SearchResult::new(hit.url, hit.title);
                                result.score = f64::from(hit.score);
                                EngineItem::Result(result)
                            })
                            .collect();
                        container.extend("local_index", 1.0, items);
                    }
                }
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

        // Semantic re-ranking (embeddings) for merged multi-engine results.
        // Matches neural rankers like Exa; single-engine keeps upstream order.
        if request.engines.len() != 1 && !self.settings.search.embedding_url.is_empty() {
            results = self.rerank_by_embedding(&request.query, results).await;
        }

        // Canonical re-ranking (opt-in): let the top pages' own
        // <link rel="canonical"> declarations consolidate mirror copies
        // into the original page. Single-engine keeps upstream order,
        // like the other re-ranking stages.
        if request.engines.len() != 1 && self.settings.search.canonical_rerank {
            results = self.rerank_by_canonical(results).await;
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
            self.attach_raw_content(&mut results).await;
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
            network: Some(self.network.clone()),
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
            tracing::debug!(engine = %name, remaining = ?remaining, reason = ?reason, "engine suspended, skipping");
            return EngineOutcome {
                engine: name,
                weight,
                items: Err(kind),
                elapsed: Duration::ZERO,
                suspended: true,
            };
        }

        let started = Instant::now();
        // 明示指定された engine（engines=... か !bang）は全体締切の縛りから外し、
        // 自身の timeout までは待つ。既定の timeout_limit は検索全体の応答時間を
        // 守るための値なので、索引走査に数秒かかる engine（anaconda は 3〜10 秒）を
        // 名指しで呼んだときまで打ち切ると、その engine は永久に 0 件になる。
        let explicit = request
            .engines
            .iter()
            .any(|wanted| wanted.eq_ignore_ascii_case(&name))
            || request
                .query
                .engine_ref_names()
                .iter()
                .any(|wanted| wanted.eq_ignore_ascii_case(&name));
        let deadline = engine_deadline(explicit, engine.timeout(), deadline, Instant::now());
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
        let items = self
            .run_engine_inner(engine, request, timeout, explicit)
            .await;
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
                // A tab that was not free says nothing about the engine.
                if !matches!(*kind, FailureKind::SolverBusy | FailureKind::SolverSkipped) {
                    self.states.record_failure(
                        &name,
                        *kind,
                        &self.settings.search,
                        self.settings.solver.suspension_cap(),
                    );
                }
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

    /// How long this engine may wait for a solver tab, and whether it has
    /// earned the run of them.
    ///
    /// Returns None when there is no time left for a tab worth taking.
    ///
    /// An engine the browser has already served waits out the whole remaining
    /// budget and may take any tab: that tab is known to turn into results.
    /// Everything else waits a bounded turn and is confined to the one tab set
    /// aside for candidates. Refusing those outright kept them from ever
    /// proving themselves, while queueing all of them freely let a dozen
    /// hopeless challenges hold every tab on every search.
    fn solver_turn(
        &self,
        engine: &str,
        remaining: Duration,
        explicit: bool,
    ) -> Option<(Duration, bool)> {
        let floor = if explicit {
            SOLVER_MIN_BUDGET_EXPLICIT
        } else {
            SOLVER_MIN_BUDGET
        };
        if remaining < floor {
            return None;
        }
        if self.states.solver_is_useful(engine) {
            Some((remaining, true))
        } else {
            // The trial tab gets the configured budget, not a hard-coded one:
            // a page that takes 4s in the production VM timed out a 3s trial
            // while the same engine answered in 1.5s from a faster host.
            Some((
                remaining.min(Duration::from_millis(self.settings.solver.budget_ms)),
                false,
            ))
        }
    }

    async fn run_engine_inner(
        &self,
        engine: &EngineHandle,
        request: &SearchRequest,
        timeout: Duration,
        explicit: bool,
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
                Ok(Ok(Err(error))) => {
                    tracing::debug!(engine = %engine.name(), error = %error, "engine failed");
                    Err(FailureKind::from(&error))
                }
                Ok(Err(_)) => Err(FailureKind::Other),
                Err(_) => Err(FailureKind::Timeout),
            };
        }
        let engine_request = engine
            .build_request(&request.query, request.pageno)
            .map_err(|error| {
                tracing::debug!(engine = %engine.name(), error = %error, "engine build_request failed");
                FailureKind::from(&error)
            })?;
        let started = Instant::now();
        // Saved before the fetch consumes the request: a CAPTCHA that only the
        // engine can recognise is retried through the solver with this URL.
        let target_url = engine_request.url.clone();
        tracing::debug!(engine = %engine.name(), url = %target_url, "engine request");
        let is_get = engine_request.method == crate::engine::Method::Get;
        // Set once a browser tab stood in for the plain request, so a parse
        // failure afterwards is known to be about the engine, not the wall.
        let mut via_solver = false;
        let mut response = match self
            .network
            .fetch(engine_request, timeout, Some(&self.cache), engine.proxy())
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::debug!(engine = %engine.name(), error = %error, "engine fetch failed");
                let kind = error.failure_kind();
                // A wall the network layer cannot recognise from the body (a
                // bare 403/429, a body without a marker) still opens in a
                // browser. One retry through the solver, then give up.
                let retryable = matches!(
                    kind,
                    FailureKind::AccessDenied | FailureKind::Captcha | FailureKind::TooManyRequests
                );
                let remaining = timeout.saturating_sub(started.elapsed());
                // API endpoints answer JSON; the browser cannot improve on
                // that and every attempt costs one of the few solver tabs.
                if !is_get
                    || !retryable
                    || !error.serves_html()
                    || self.states.solver_cannot_help(engine.name())
                {
                    return Err(kind);
                }
                if remaining.is_zero() {
                    // The request itself used up the budget, so no tab ever got
                    // a chance to answer. Nothing was attempted, and suspending
                    // the engine here would take a good engine away for nothing.
                    return Err(FailureKind::SolverSkipped);
                }
                let Some((budget, proven)) = self.solver_turn(engine.name(), remaining, explicit)
                else {
                    return Err(FailureKind::SolverSkipped);
                };
                match self.network.solve_page(&target_url, budget, proven, engine.solver_dwell()).await {
                    SolverAttempt::Solved(page) => {
                        via_solver = true;
                        page
                    }
                    SolverAttempt::Busy => return Err(FailureKind::SolverBusy),
                    SolverAttempt::Hopeless => {
                        self.states.mark_solver_cannot_help(engine.name());
                        tracing::debug!(
                            engine = %engine.name(),
                            "the solver is refused as well; no further attempts"
                        );
                        return Err(kind);
                    }
                }
            }
        };
        // A shell page is not a challenge - nothing in the response says the
        // results are missing - but only a browser can fill it. The engine
        // recognises its own shell and the same URL goes to the solver with the
        // settle time the engine asks for.
        if !via_solver
            && is_get
            && engine.needs_render(&response)
            && engine.solver_dwell().is_some()
            && !self.states.solver_cannot_help(engine.name())
        {
            let remaining = timeout.saturating_sub(started.elapsed());
            if !remaining.is_zero() {
                if let Some((budget, proven)) = self.solver_turn(engine.name(), remaining, explicit)
                {
                    match self
                        .network
                        .solve_page(&target_url, budget, proven, engine.solver_dwell())
                        .await
                    {
                        SolverAttempt::Solved(page) => {
                            via_solver = true;
                            response = page;
                        }
                        SolverAttempt::Busy => return Err(FailureKind::SolverBusy),
                        SolverAttempt::Hopeless => {
                            self.states.mark_solver_cannot_help(engine.name());
                            return Err(FailureKind::AccessDenied);
                        }
                    }
                }
            }
        }
        // A wall the body itself gives away - a 200 interstitial that looks
        // like a page, or a 403 the fetch already turned into a response - is
        // retried in the browser here, where the engine's remaining budget and
        // the breaker are known.
        if !via_solver
            && is_get
            && crate::solver::looks_like_challenge(response.status, &response.body)
            && !self.states.solver_cannot_help(engine.name())
        {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(FailureKind::SolverSkipped);
            }
            let Some((budget, proven)) = self.solver_turn(engine.name(), remaining, explicit)
            else {
                return Err(FailureKind::SolverSkipped);
            };
            match self.network.solve_page(&target_url, budget, proven, engine.solver_dwell()).await {
                SolverAttempt::Solved(page) => {
                    via_solver = true;
                    response = page;
                }
                SolverAttempt::Busy => return Err(FailureKind::SolverBusy),
                SolverAttempt::Hopeless => {
                    self.states.mark_solver_cannot_help(engine.name());
                    tracing::debug!(
                        engine = %engine.name(),
                        "the solver is refused as well; no further attempts"
                    );
                    return Err(FailureKind::AccessDenied);
                }
            }
        }
        if via_solver && crate::solver::looks_like_challenge(response.status, &response.body) {
            // The browser came back with the same wall: the site refuses it as
            // well, and further tabs would only be spent on it.
            self.states.mark_solver_cannot_help(engine.name());
            tracing::debug!(
                engine = %engine.name(),
                "the solver got the same challenge page; no further attempts"
            );
            return Err(FailureKind::AccessDenied);
        }
        // A shell page that loads its results from a fragment: follow it with
        // the time the first request left over.
        if let Some(followup) = engine.followup_request(&response, &request.query).map_err(
            |error| {
                tracing::debug!(engine = %engine.name(), error = %error, "engine followup failed");
                FailureKind::from(&error)
            },
        )? {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(FailureKind::Timeout);
            }
            response = self
                .network
                .fetch(followup, remaining, Some(&self.cache), engine.proxy())
                .await
                .map_err(|error| {
                    tracing::debug!(engine = %engine.name(), error = %error, "engine followup fetch failed");
                    error.failure_kind()
                })?;
        }
        let items = match engine.parse(&response, &request.query) {
            Ok(items) => items,
            Err(error) => {
                // The engine itself decided the body is a verification wall.
                // A real browser passes where a plain client does not, so the
                // same URL goes to the solver once and the page it brings back
                // is parsed again.
                if !(is_get
                    && response.serves_html()
                    && matches!(error, crate::engine::EngineError::Captcha)
                    && !self.states.solver_cannot_help(engine.name()))
                {
                    tracing::debug!(engine = %engine.name(), error = %error, "engine parse failed");
                    return Err(FailureKind::from(&error));
                }
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(FailureKind::SolverSkipped);
                }
                let Some((budget, proven)) = self.solver_turn(engine.name(), remaining, explicit)
                else {
                    return Err(FailureKind::SolverSkipped);
                };
                match self.network.solve_page(&target_url, budget, proven, engine.solver_dwell()).await {
                    SolverAttempt::Solved(page) => match engine.parse(&page, &request.query) {
                        Ok(items) => {
                            // The browser earned this engine its results, so
                            // from now on it may queue for a tab.
                            via_solver = true;
                            items
                        }
                        Err(error) => {
                            // The browser got a fresh page and the engine still
                            // could not read it: this engine speaks JSON, or the
                            // wall is not a challenge. Stop spending a tab on it.
                            self.states.mark_solver_cannot_help(engine.name());
                            tracing::debug!(
                                engine = %engine.name(),
                                error = %error,
                                "engine parse failed after the solver cleared the page"
                            );
                            return Err(FailureKind::from(&error));
                        }
                    },
                    SolverAttempt::Busy => return Err(FailureKind::SolverBusy),
                    SolverAttempt::Hopeless => {
                        self.states.mark_solver_cannot_help(engine.name());
                        tracing::debug!(
                            engine = %engine.name(),
                            "the solver is refused as well; no further attempts"
                        );
                        return Err(FailureKind::from(&error));
                    }
                }
            }
        };
        if via_solver {
            self.states.mark_solver_useful(engine.name());
        }
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
        if !self.settings.search.site_scoped_retry || request.engines.len() == 1 {
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

    /// Consolidate the top results by their pages' canonical declarations.
    ///
    /// Each checked page is fetched once, through the same SSRF guard and
    /// cache as the raw-content fetch. The score changes themselves live
    /// in `apply_canonical_votes`, which is pure and unit-tested; a fetch
    /// failure simply leaves that result unchecked (neutral).
    async fn rerank_by_canonical(&self, mut results: Vec<SearchResult>) -> Vec<SearchResult> {
        let settings = &self.settings.search;
        let topk = results.len().min(settings.canonical_topk);
        if topk < 2 {
            return results;
        }
        let canonicals = futures::future::join_all(
            results
                .iter()
                .take(topk)
                .map(|result| self.page_canonical(&result.url)),
        )
        .await;
        for (result, canonical) in results.iter_mut().take(topk).zip(&canonicals) {
            if canonical.is_some() {
                result.canonical_url = canonical.clone();
            }
        }
        apply_canonical_votes(results, topk, &canonicals, settings.canonical_weight)
    }

    /// The canonical URL a page declares for itself, `None` when the page
    /// cannot be fetched or declares none.
    async fn page_canonical(&self, url: &str) -> Option<String> {
        let html = self.fetch_page_html(url).await?;
        crate::content::canonical_url(&html, url)
    }

    /// Fetch and parse one result page, refusing non-public URLs.
    ///
    /// Result URLs come from third-party engines, so treat them as untrusted:
    /// an engine (or a poisoned result) must not make us fetch localhost or
    /// the cloud metadata endpoint on its behalf.
    async fn fetch_page_html(&self, url: &str) -> Option<scraper::Html> {
        if !crate::web::image_proxy::is_public_url(url) {
            tracing::debug!(url, "refusing to fetch a non-public page");
            return None;
        }
        let request = crate::engine::EngineRequest::get(url)
            .header("Accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8");
        let response = self
            .network
            .fetch(request, Duration::from_secs(4), Some(&self.cache), None)
            .await
            .ok()?;
        response.html().ok()
    }

    async fn attach_raw_content(&self, results: &mut [SearchResult]) {
        let limit = self.settings.search.raw_content_limit.max(1);
        let count = results.len().min(limit);
        if count == 0 {
            return;
        }
        let texts = futures::future::join_all(
            results
                .iter()
                .take(count)
                .map(|result| self.page_text(&result.url)),
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
    async fn page_text(&self, url: &str) -> Option<crate::fetch::PageContent> {
        match self
            .page_reader
            .read(url, self.settings.search.raw_content_chars.max(1))
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

/// How many canonical votes for one target are counted at most. A page in
/// the checked set can only be boosted so far by other pages pointing at
/// it, so a cluster of copies cannot manufacture unlimited score.
const MAX_CANONICAL_VOTES: usize = 3;

/// Normalise a URL for canonical matching: lowercase host, no fragment,
/// no trailing slash (except the root), no default port. Deliberately
/// conservative — two URLs that fail to normalise to the same string are
/// simply not merged, which is the safe failure mode.
fn normalise_url_for_canonical_match(raw: &str) -> Option<String> {
    let mut parsed = url::Url::parse(raw.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.set_fragment(None);
    if (parsed.scheme() == "http" && parsed.port() == Some(80))
        || (parsed.scheme() == "https" && parsed.port() == Some(443))
    {
        let _ = parsed.set_port(None);
    }
    let mut out = parsed.to_string();
    if out.ends_with('/') && parsed.path() != "/" {
        out.pop();
    }
    Some(out)
}

/// Apply the canonical declarations of the first `topk` results.
///
/// `canonicals[i]` is the canonical URL result `i` declares for itself
/// (already resolved to an absolute URL), or `None` when the page was not
/// fetched or declares none. Rules, in order:
///
/// - Self-canonical, no declaration, or a target outside the checked set:
///   neutral. A copy is better than nothing when the original page is not
///   among the results, so a lone voter is never punished.
/// - A result whose canonical target *is* in the checked set is a copy:
///   it is removed and merged into the target (engines are unioned), and
///   the target gains `weight` per merged copy, capped at
///   `MAX_CANONICAL_VOTES` votes. This applies across hosts (a mirror
///   pointing at the official docs) and within one host (a page that
///   canonicalises to its own newer path).
/// - Results beyond `topk` are never touched, and their relative order
///   is preserved; the checked head is re-sorted by score at the end.
///
/// The hand-maintained `AUTHORITIES` boost in `ResultContainer::close`
/// has already run at this point and is *not* overridden: this stage only
/// consolidates copies, it never demotes a result the list lifted unless
/// that result's own page declares it is a copy of another result.
fn apply_canonical_votes(
    results: Vec<SearchResult>,
    topk: usize,
    canonicals: &[Option<String>],
    weight: f64,
) -> Vec<SearchResult> {
    let topk = topk.min(results.len()).min(canonicals.len());
    if topk < 2 || weight <= 0.0 {
        return results;
    }
    let (head, tail) = results.split_at(topk);
    let mut head: Vec<SearchResult> = head.to_vec();
    // Normalised URL -> index of the first result with that URL.
    let mut by_url: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (index, result) in head.iter().enumerate() {
        if let Some(key) = normalise_url_for_canonical_match(&result.url) {
            by_url.entry(key).or_insert(index);
        }
    }
    let mut votes: Vec<usize> = vec![0; head.len()];
    let mut merged_away: Vec<bool> = vec![false; head.len()];
    for (index, canonical) in canonicals.iter().take(topk).enumerate() {
        let Some(canonical) = canonical else { continue };
        let Some(target_key) = normalise_url_for_canonical_match(canonical) else {
            continue;
        };
        let Some(&target) = by_url.get(&target_key) else {
            continue;
        };
        if target == index {
            continue; // self-canonical: no signal
        }
        // This result declares itself a copy of `target`: consolidate.
        merged_away[index] = true;
        votes[target] += 1;
        let engines = head[index].engines.clone();
        for engine in engines {
            if !head[target].engines.contains(&engine) {
                head[target].engines.push(engine);
            }
        }
    }
    for (index, result) in head.iter_mut().enumerate() {
        if votes[index] > 0 {
            result.score += weight * votes[index].min(MAX_CANONICAL_VOTES) as f64;
        }
    }
    let mut head: Vec<SearchResult> = head
        .into_iter()
        .zip(&merged_away)
        .filter(|(_, away)| !**away)
        .map(|(result, _)| result)
        .collect();
    head.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    head.extend(tail.iter().cloned());
    head
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
    match crate::quality::inspect(query, &items) {
        None => Ok(items),
        // A duplicate-heavy answer is often an index that lists the same
        // document under several URLs (erowid's scan list does exactly this).
        // Collapse the copies and keep what is left when it still looks like a
        // real answer; fabricated and unrelated answers are still dropped.
        Some(crate::quality::Suspicion::Repeated) => {
            let total = items.len();
            let sample: Vec<String> = items
                .iter()
                .take(3)
                .filter_map(|item| match item {
                    crate::engine::EngineItem::Result(result) => Some(format!(
                        "{} => {}",
                        result.title.chars().take(40).collect::<String>(),
                        result.url
                    )),
                    _ => None,
                })
                .collect();
            let kept = dedupe_repeated_results(items);
            if kept.len() < 3 || crate::quality::inspect(query, &kept).is_some() {
                tracing::warn!(
                    %engine,
                    total,
                    kept = kept.len(),
                    ?sample,
                    reason = "repeated results",
                    "discarding a suspicious engine response"
                );
                return Err(FailureKind::Suspicious);
            }
            tracing::debug!(
                %engine,
                kept = kept.len(),
                "keeping a duplicate-heavy engine response after dedupe"
            );
            Ok(kept)
        }
        Some(suspicion) => {
            tracing::warn!(
                %engine,
                reason = suspicion.reason(),
                "discarding a suspicious engine response"
            );
            Err(FailureKind::Suspicious)
        }
    }
}

/// Drop entries that repeat the same title.
///
/// Pages that list one document several times still carry distinct URLs and
/// may even vary the snippet, so the copies survive the merge; collapsing them
/// here keeps the first copy and lets the sanity check judge a deduplicated
/// answer. The title is the field the repeated-results check looks at, so it is
/// the field to collapse on.
fn dedupe_repeated_results(
    items: Vec<crate::engine::EngineItem>,
) -> Vec<crate::engine::EngineItem> {
    use std::collections::HashSet;

    let mut seen: HashSet<String> = HashSet::new();
    items
        .into_iter()
        .filter(|item| match item {
            crate::engine::EngineItem::Result(result) => seen.insert(
                result
                    .title
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase(),
            ),
            _ => true,
        })
        .collect()
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
            let mut text = crate::engine::error_type_label(engine.error_type.as_str()).to_string();
            if engine.suspended && text != "Suspended" {
                text = format!("Suspended: {text}");
            }
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
        "solver busy" => FailureKind::SolverBusy,
        "solver skipped" => FailureKind::SolverSkipped,
        "http error" => FailureKind::Http,
        "parsing error" => FailureKind::Parse,
        "suspicious response" => FailureKind::Suspicious,
        _ => FailureKind::Other,
    }
}

/// How long an engine may run, given the whole-search deadline.
///
/// The engine's own timeout is the ceiling; the search deadline may cut it short.
/// Returns `None` when the deadline has already passed (do not run at all).
/// 明示指定（engines=... / !bang）された engine は検索全体の締切ではなく
/// 自身の timeout まで待つ。索引走査に数秒かかる engine を名指ししたときに
/// 締切で必ず 0 件になるのを避けるための例外。
fn engine_deadline(
    explicit: bool,
    engine_timeout: Duration,
    search_deadline: Instant,
    now: Instant,
) -> Instant {
    if explicit {
        search_deadline.max(now + engine_timeout)
    } else {
        search_deadline
    }
}

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

    #[test]
    fn a_named_engine_may_take_a_tab_the_general_floor_would_refuse() {
        let searcher = searcher(vec![]);
        // A wall that answers after 1.5s of a 4s budget leaves 2.4s: too little
        // for the general floor, but the tab the search is already waiting for.
        let left = Duration::from_millis(2_400);
        assert!(searcher.solver_turn("yandex", left, false).is_none());
        let (budget, proven) = searcher
            .solver_turn("yandex", left, true)
            .expect("a named engine waits for the tab");
        assert_eq!(budget, left);
        assert!(!proven, "a first attempt is confined to the trial tab");
        // Even a named engine stops once the tab could not finish in time.
        assert!(
            searcher
                .solver_turn("yandex", Duration::from_millis(900), true)
                .is_none()
        );
    }

    #[tokio::test]
    async fn raw_content_refuses_private_result_urls() {
        let searcher = searcher(vec![]);
        let mut results = vec![SearchResult::new(
            "http://127.0.0.1:9/private",
            "poisoned result",
        )];
        searcher.attach_raw_content(&mut results).await;
        assert!(results[0].raw_content.is_none());
        assert!(results[0].raw_content_truncated.is_none());
    }

    #[tokio::test]
    async fn site_retry_preserves_query_options_and_engine_selection() {
        let selected = FakeEngine::build("selected", &["general"], 0, true);
        let other = FakeEngine::build("other", &["general"], 0, true);
        let mut searcher = searcher(vec![selected.clone(), other.clone()]);
        Arc::make_mut(&mut searcher.settings)
            .search
            .site_scoped_retry = true;
        let mut original = query("!selected :ja rust traits", &searcher.registry);
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
        assert!(other.queries.lock().unwrap().is_empty());
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

    fn scored(url: &str, score: f64) -> SearchResult {
        let mut result = SearchResult::new(url, "title");
        result.score = score;
        result.engines = vec!["engine".to_string()];
        result
    }

    #[test]
    fn canonical_mirror_is_merged_into_its_target() {
        let results = vec![
            scored("https://mirror.example.com/guide", 10.0),
            scored("https://docs.example.com/guide", 9.0),
        ];
        let canonicals = vec![
            Some("https://docs.example.com/guide".to_string()),
            Some("https://docs.example.com/guide".to_string()),
        ];
        let out = apply_canonical_votes(results, 2, &canonicals, 4.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://docs.example.com/guide");
        // One merged copy = one vote of weight.
        assert_eq!(out[0].score, 13.0);
    }

    #[test]
    fn canonical_self_declaration_is_neutral() {
        let results = vec![
            scored("https://a.example.com/x", 10.0),
            scored("https://b.example.com/y", 9.0),
        ];
        let canonicals = vec![
            Some("https://a.example.com/x".to_string()),
            Some("https://b.example.com/y".to_string()),
        ];
        let out = apply_canonical_votes(results, 2, &canonicals, 4.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].score, 10.0);
        assert_eq!(out[1].score, 9.0);
    }

    #[test]
    fn canonical_target_outside_the_checked_set_is_neutral() {
        // The original page is not among the results: the copy is the
        // only accessible version and must not be punished.
        let results = vec![
            scored("https://mirror.example.com/guide", 10.0),
            scored("https://other.example.com/z", 9.0),
        ];
        let canonicals = vec![Some("https://docs.example.com/guide".to_string()), None];
        let out = apply_canonical_votes(results, 2, &canonicals, 4.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].url, "https://mirror.example.com/guide");
        assert_eq!(out[0].score, 10.0);
    }

    #[test]
    fn canonical_votes_are_capped() {
        let mut results = vec![scored("https://docs.example.com/guide", 1.0)];
        let mut canonicals = vec![Some("https://docs.example.com/guide".to_string())];
        for i in 0..6 {
            results.push(scored(&format!("https://m{i}.example.com/guide"), 0.5));
            canonicals.push(Some("https://docs.example.com/guide".to_string()));
        }
        let out = apply_canonical_votes(results, 7, &canonicals, 4.0);
        assert_eq!(out.len(), 1);
        // 6 copies but only MAX_CANONICAL_VOTES votes count.
        assert_eq!(out[0].score, 1.0 + 4.0 * MAX_CANONICAL_VOTES as f64);
    }

    #[test]
    fn canonical_stage_never_touches_results_beyond_topk() {
        let results = vec![
            scored("https://a.example.com/x", 10.0),
            scored("https://docs.example.com/guide", 9.0),
            scored("https://mirror.example.com/guide", 8.0),
        ];
        // The mirror is result #3 and outside the checked top 2, even
        // though a canonical entry is (incorrectly) supplied for it.
        let canonicals = vec![
            None,
            None,
            Some("https://docs.example.com/guide".to_string()),
        ];
        let out = apply_canonical_votes(results, 2, &canonicals, 4.0);
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].url, "https://mirror.example.com/guide");
        assert_eq!(out[2].score, 8.0);
    }

    #[test]
    fn canonical_match_ignores_fragment_and_trailing_slash() {
        let results = vec![
            scored("https://mirror.example.com/guide", 10.0),
            scored("https://docs.example.com/guide/", 9.0),
        ];
        let canonicals = vec![Some("https://docs.example.com/guide#top".to_string()), None];
        let out = apply_canonical_votes(results, 2, &canonicals, 4.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].score, 13.0);
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

    /// An engine named explicitly (`engines=...` or `!bang`) is not cut by the
    /// search deadline: it gets its own timeout instead. Measured: anaconda's
    /// search API needs 3-10 s, so under the default 2.5 s deadline a named
    /// request always returned zero results.
    #[test]
    fn an_explicit_engine_gets_its_own_timeout() {
        let now = Instant::now();
        let deadline = engine_deadline(
            true,
            Duration::from_secs(12),
            now + Duration::from_millis(300),
            now,
        );
        assert_eq!(deadline, now + Duration::from_secs(12));
        assert_eq!(
            engine_budget(Duration::from_secs(12), deadline, now),
            Some(Duration::from_secs(12))
        );
    }

    #[test]
    fn a_non_explicit_engine_keeps_the_search_deadline() {
        let now = Instant::now();
        let search_deadline = now + Duration::from_millis(300);
        assert_eq!(
            engine_deadline(false, Duration::from_secs(12), search_deadline, now),
            search_deadline
        );
    }

    /// The explicit path must never shorten a generous search deadline.
    #[test]
    fn an_explicit_engine_never_shortens_the_search_deadline() {
        let now = Instant::now();
        let search_deadline = now + Duration::from_secs(30);
        assert_eq!(
            engine_deadline(true, Duration::from_secs(12), search_deadline, now),
            search_deadline
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
    async fn strict_engines_fails_on_unknown_names() {
        let searcher = searcher(vec![FakeEngine::build("web engine", &["general"], 1, true)]);
        let mut request = SearchRequest::new(query("cats", &searcher.registry));
        request.engines = vec!["web engine".to_string(), "nope".to_string()];
        request.strict_engines = true;
        let response = searcher.search(request).await;
        assert_eq!(response.error.as_deref(), Some("unknown engines: nope"),);
    }

    #[tokio::test]
    async fn strict_engines_passes_when_all_names_exist() {
        let searcher = searcher(vec![FakeEngine::build("web engine", &["general"], 1, true)]);
        let mut request = SearchRequest::new(query("cats", &searcher.registry));
        request.engines = vec!["web engine".to_string()];
        request.strict_engines = true;
        let response = searcher.search(request).await;
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn unknown_engine_names_fall_back_without_strict() {
        let searcher = searcher(vec![FakeEngine::build("web engine", &["general"], 1, true)]);
        let mut request = SearchRequest::new(query("cats", &searcher.registry));
        request.engines = vec!["nope".to_string()];
        let response = searcher.search(request).await;
        assert!(response.error.is_none());
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
            None,
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
    use super::*;

    #[test]
    fn cosine_similarity_behaves_as_expected() {
        assert!((dot(&[1.0f32, 0.0], &[1.0f32, 0.0]) - 1.0).abs() < 1e-6);
        assert!((dot(&[1.0f32, 0.0], &[0.0f32, 1.0])).abs() < 1e-6);
        assert_eq!(dot(&[], &[1.0]), 0.0);
        assert_eq!(dot(&[1.0f32], &[1.0, 0.0]), 0.0);
    }
}
