//! HTTP layer, ported from `searx/network/`.
//!
//! Responsibilities kept from upstream: rotating user agents, per request
//! timeout, proxies, a redirect limit, retries on transport errors and a
//! response cache.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use sha2::{Digest, Sha256};
use tracing::debug;

use crate::cache::{Cache, CachedResponse};
use crate::engine::{EngineRequest, EngineResponse, FailureKind, Method};
use crate::settings::Settings;
use crate::solver::Solver;

const USER_AGENT_OS: &[&str] = &[
    "Windows NT 10.0; Win64; x64",
    "X11; Linux x86_64",
    "Macintosh; Intel Mac OS X 10.15",
    "X11; Ubuntu; Linux x86_64",
];
const USER_AGENT_VERSIONS: &[&str] = &["157.0", "156.0", "155.0", "154.0"];

/// A shipped browser signature, used only to avoid an Anubis deep grade.
const ANUBIS_PLAIN_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const USER_AGENT_TEMPLATE: &str =
    "Mozilla/5.0 ({os}; rv:{version}) Gecko/20100101 Firefox/{version}";

#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("request timed out")]
    Timeout,
    #[error("response exceeds the {0} byte decoded body limit")]
    TooLarge(usize),
    #[error("http status {status}")]
    Status {
        status: u16,
        body: String,
        response: Box<EngineResponse>,
    },
    #[error("transport error: {0}")]
    Transport(String),
    #[error("request could not be built: {0}")]
    Build(String),
}

impl NetworkError {
    /// Map a network error onto the framework failure kinds.
    pub fn failure_kind(&self) -> FailureKind {
        match self {
            Self::Timeout => FailureKind::Timeout,
            Self::TooLarge(_) => FailureKind::Other,
            Self::Build(_) => FailureKind::Other,
            Self::Transport(_) => FailureKind::Other,
            Self::Status { status, body, .. } => classify_status(*status, body),
        }
    }

    /// Whether the failing response is a page a browser could render. A JSON
    /// API that answers 403 stays a dead end even with a browser.
    pub fn serves_html(&self) -> bool {
        match self {
            Self::Status { response, .. } => response.serves_html(),
            _ => false,
        }
    }
}

/// Classify an HTTP status (with a body excerpt) the way upstream does.
pub fn classify_status(status: u16, body: &str) -> FailureKind {
    let lowered = body.to_lowercase();
    let captcha = lowered.contains("captcha")
        || lowered.contains("anomaly")
        || lowered.contains("unusual traffic")
        || lowered.contains("are you a robot");
    match status {
        403 if captcha => FailureKind::Captcha,
        401..=403 => FailureKind::AccessDenied,
        429 => FailureKind::TooManyRequests,
        500..=599 => FailureKind::Http,
        _ => FailureKind::Http,
    }
}

/// The HTTP client used by every engine.
pub struct Network {
    client: reqwest::Client,
    /// Direct-only client for third-party media; no engine host exemptions.
    media_client: reqwest::Client,
    media_proxy_blocked: bool,
    /// Clients for engines that need their own proxy, keyed by proxy URL.
    proxied: std::sync::Mutex<std::collections::HashMap<String, reqwest::Client>>,
    /// Settings kept so a proxied client can be rebuilt on demand.
    outgoing: crate::settings::Outgoing,
    user_agents: Vec<String>,
    suffix: String,
    retries: u32,
    counter: AtomicUsize,
    /// Cookies won by solving an Anubis challenge, keyed by host.
    anubis_cookies: std::sync::Mutex<BTreeMap<String, Vec<(String, String)>>>,
    /// The user agent that won those cookies, keyed by host. Anubis binds its
    /// auth cookie to the client signature, so every later request to that host
    /// has to repeat the exact same one or the guard page comes back.
    anubis_agents: std::sync::Mutex<BTreeMap<String, String>>,
    /// Client that never follows redirects; a challenge round trip needs the
    /// "Set-Cookie" of the intermediate response.
    bare: reqwest::Client,
    /// Optional browser solver for engines behind a WAF or a CAPTCHA.
    solver: Option<std::sync::Arc<Solver>>,
}

/// What the browser could do about a blocked page.
pub enum SolverAttempt {
    /// The browser opened the page and handed back the rendered HTML.
    Solved(EngineResponse),
    /// No tab was free inside the engine budget. Not the site's fault.
    Busy,
    /// The site refuses the browser itself, so a tab cannot ever help.
    Hopeless,
}

/// Whether the solver message says the site bans the browser's address.
///
/// FlareSolverr answers `status: "error"` both for a page it could not load and
/// for a hard block, but only the second one makes further tabs pointless.
fn refuses(error: &crate::solver::SolverError) -> bool {
    // A hard block arrives in two shapes: FlareSolverr's own JSON error
    // (`status: "error"`, mapped to Failed) and a non-200 reply to the API
    // call itself, whose body still carries the same sentence (mapped to
    // Transport by `ask`). Both mean every further tab is wasted.
    let message = match error {
        crate::solver::SolverError::Failed(message) => message,
        crate::solver::SolverError::Transport(message) => message,
        _ => return false,
    };
    let message = message.to_ascii_lowercase();
    message.contains("banned") || message.contains("blocked")
}

/// Build a client from the outgoing settings, optionally through another proxy.
fn build_client(
    outgoing: &crate::settings::Outgoing,
    extra_proxy: Option<reqwest::Proxy>,
) -> Result<reqwest::Client, NetworkError> {
    build_client_with(
        outgoing,
        extra_proxy,
        reqwest::redirect::Policy::limited(outgoing.max_redirects),
    )
}

/// ["build_client"] with an explicit redirect policy.
fn build_client_with(
    outgoing: &crate::settings::Outgoing,
    extra_proxy: Option<reqwest::Proxy>,
    redirect: reqwest::redirect::Policy,
) -> Result<reqwest::Client, NetworkError> {
    let mut builder = reqwest::Client::builder()
        .redirect(redirect)
        // Some sites answer the first request with a 302 back to the same URL
        // that only plants cookies (goodreads does this). Without a jar the hop
        // repeats until the redirect limit and the page never arrives.
        .cookie_store(true)
        .timeout(Duration::from_secs_f64(
            outgoing.max_request_timeout.max(1.0),
        ))
        // The connect budget must not be the binding limit: a few upstreams
        // (directory.fsf.org) take 6-12 s just for TCP+TLS, and the old 5 s
        // cap turned every such engine into a permanent timeout. The overall
        // request timeout below still bounds the whole call, and every engine
        // is additionally wrapped in its own per-engine budget.
        .connect_timeout(Duration::from_secs_f64(
            outgoing.max_request_timeout.clamp(5.0, 30.0),
        ))
        .pool_max_idle_per_host(outgoing.pool_maxsize)
        .danger_accept_invalid_certs(!outgoing.verify)
        .user_agent(default_user_agent());
    if let Some(path) = &outgoing.ca_bundle {
        let pem = std::fs::read(path)
            .map_err(|e| NetworkError::Build(format!("cannot read CA bundle {path}: {e}")))?;
        let certificates = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|e| NetworkError::Build(format!("invalid CA bundle: {e}")))?;
        if certificates.is_empty() {
            return Err(NetworkError::Build(
                "CA bundle contains no certificates".into(),
            ));
        }
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    if !outgoing.enable_http2 {
        builder = builder.http1_only();
    }
    if let Some(proxy) = extra_proxy {
        builder = builder.proxy(proxy);
    } else {
        for (scheme, proxy) in &outgoing.proxies {
            let proxy = reqwest::Proxy::all(proxy)
                .map_err(|error| NetworkError::Build(error.to_string()))?;
            tracing::debug!(scheme, "configured proxy");
            builder = builder.proxy(proxy);
        }
    }
    builder
        .build()
        .map_err(|error| NetworkError::Build(error.to_string()))
}
/// Build the optional browser solver, or None when it is switched off.
fn build_solver(settings: &crate::settings::SolverSettings) -> Option<std::sync::Arc<Solver>> {
    if !settings.enabled {
        return None;
    }
    match Solver::new(settings) {
        Ok(solver) => {
            debug!(endpoint = %settings.endpoint, "browser solver enabled");
            let solver = std::sync::Arc::new(solver);
            // A browser that is only started by the first blocked search answers
            // too late for the engine budget, so start it now.
            solver.prewarm();
            Some(solver)
        }
        Err(error) => {
            tracing::warn!(%error, "browser solver disabled: it could not be built");
            None
        }
    }
}

impl Network {
    pub fn from_settings(settings: &Settings) -> Result<Self, NetworkError> {
        let outgoing = &settings.outgoing;
        let client = build_client(outgoing, None)?;
        let (media_client, media_proxy_blocked) =
            crate::mcp::page::public_client(outgoing, &[]).map_err(NetworkError::Build)?;
        Ok(Self {
            client,
            media_client,
            media_proxy_blocked,
            proxied: std::sync::Mutex::new(std::collections::HashMap::new()),
            outgoing: outgoing.clone(),
            user_agents: build_user_agents(),
            suffix: outgoing.useragent_suffix.clone(),
            retries: outgoing.retries,
            counter: AtomicUsize::new(0),
            anubis_cookies: std::sync::Mutex::new(BTreeMap::new()),
            anubis_agents: std::sync::Mutex::new(BTreeMap::new()),
            bare: build_client_with(outgoing, None, reqwest::redirect::Policy::none())?,
            solver: build_solver(&settings.solver),
        })
    }

    /// The client for one request, honouring a per-engine proxy.
    ///
    /// Engines share the default client; a per-engine proxy gets its own so a
    /// blocked engine can be routed elsewhere without moving the rest.
    fn client_for(&self, proxy: Option<&str>) -> Result<reqwest::Client, NetworkError> {
        let Some(proxy) = proxy.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(self.client.clone());
        };
        if let Ok(cache) = self.proxied.lock() {
            if let Some(client) = cache.get(proxy) {
                return Ok(client.clone());
            }
        }
        let configured =
            reqwest::Proxy::all(proxy).map_err(|error| NetworkError::Build(error.to_string()))?;
        let client = build_client(&self.outgoing, Some(configured))?;
        if let Ok(mut cache) = self.proxied.lock() {
            cache.insert(proxy.to_string(), client.clone());
        }
        Ok(client)
    }

    /// Cookies remembered for the host of `url`.
    fn jar_cookies(&self, url: &str) -> Vec<(String, String)> {
        let host = url_host(url);
        self.anubis_cookies
            .lock()
            .ok()
            .and_then(|jar| jar.get(&host).cloned())
            .unwrap_or_default()
    }

    /// Solve the Anubis challenge a guarded site served and remember the cookies.
    ///
    /// The flow mirrors `anubis`'s own JavaScript: the guard page carries a
    /// `challenge` object, the browser hashes `randomData` with an increasing
    /// nonce until the digest has enough leading zeros, then trades that proof
    /// for a cookie at `pass-challenge`.
    async fn solve_anubis(&self, url: &str, deadline: &tokio::time::Instant) -> bool {
        let host = url_host(url);
        if host.is_empty()
            || self
                .jar_cookies(url)
                .iter()
                .any(|(name, _)| name.contains("anubis-auth"))
        {
            return false;
        }
        let started = std::time::Instant::now();
        // Anubis binds the cookie it hands out to the client signature, so the
        // guard page, the proof and every later request have to repeat one and
        // the same user agent. The rotated pool is intentionally ahead of the
        // released browsers and earns grade 6 there (16x the hashes) where a
        // shipped signature earns grade 2, so use the plain one and stay on it
        // for this host.
        let user_agent = ANUBIS_PLAIN_USER_AGENT.to_string();
        let Some((body, verification)) = self.anubis_guard(url, deadline, &user_agent).await else {
            debug!(url, "anubis: the guard page could not be fetched");
            return false;
        };
        let Some(challenge) = anubis_challenge(body.as_bytes()) else {
            debug!(
                url,
                bytes = body.len(),
                "anubis: the guard page carried no supported challenge"
            );
            return false;
        };
        let Some((nonce, digest)) = solve_anubis_pow(&challenge, *deadline) else {
            debug!(
                url,
                difficulty = challenge.difficulty,
                "anubis: no nonce found"
            );
            return false;
        };
        let origin = url_origin(url);
        let Ok(mut pass) = url::Url::parse(&format!(
            "{origin}/.within.website/x/cmd/anubis/api/pass-challenge"
        )) else {
            return false;
        };
        pass.query_pairs_mut()
            .append_pair("id", &challenge.id)
            .append_pair("response", &digest)
            .append_pair("nonce", &nonce.to_string())
            .append_pair("redir", url)
            .append_pair("elapsedTime", &started.elapsed().as_millis().to_string());
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let mut request = self
            .bare
            .get(pass)
            .timeout(remaining)
            .header(reqwest::header::USER_AGENT, user_agent.as_str());
        if !verification.is_empty() {
            let cookie = verification
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                request = request.header(reqwest::header::COOKIE, value);
            }
        }
        let Ok(response) = request.send().await else {
            debug!(url, "anubis: pass-challenge could not be sent");
            return false;
        };
        let mut cookies = set_cookie_pairs(response.headers());
        if cookies.is_empty() {
            debug!(
                url,
                status = response.status().as_u16(),
                "anubis: pass-challenge returned no cookie"
            );
            return false;
        }
        cookies.extend(verification);
        if let Ok(mut jar) = self.anubis_cookies.lock() {
            jar.insert(host.clone(), cookies);
        }
        if let Ok(mut agents) = self.anubis_agents.lock() {
            agents.insert(host, user_agent);
        }
        true
    }

    /// Fetch an Anubis guard page, keeping its cookies and body.
    async fn anubis_guard(
        &self,
        url: &str,
        deadline: &tokio::time::Instant,
        user_agent: &str,
    ) -> Option<(String, Vec<(String, String)>)> {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let response = self
            .bare
            .get(url)
            .timeout(remaining)
            .header(reqwest::header::USER_AGENT, user_agent)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!(
                url,
                status = response.status().as_u16(),
                "anubis: the guard page was not a success"
            );
            return None;
        }
        let cookies = set_cookie_pairs(response.headers());
        let body = response.text().await.ok()?;
        Some((body, cookies))
    }

    /// A different user agent on every call, like upstream's rotation.
    pub fn user_agent(&self) -> String {
        if self.user_agents.is_empty() {
            return default_user_agent();
        }
        let index = self.counter.fetch_add(1, Ordering::Relaxed) % self.user_agents.len();
        format!("{} {}", self.user_agents[index], self.suffix)
            .trim()
            .to_string()
    }

    fn default_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );
        headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.7"),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("dnt"),
            HeaderValue::from_static("1"),
        );
        if let Ok(value) = HeaderValue::from_str(&self.user_agent()) {
            headers.insert(reqwest::header::USER_AGENT, value);
        }
        headers
    }

    /// Replay the cookies the browser solver won for this host.
    ///
    /// A cleared wall stays open for hours through its clearance cookie, and
    /// that cookie answers a plain request in milliseconds. Without replaying
    /// it every search pays for a browser tab all over again.
    fn with_solver_cookies(&self, mut request: EngineRequest) -> EngineRequest {
        let Some(solver) = self.solver.as_ref() else {
            return request;
        };
        for (name, value) in solver.jar_cookies(&request.url) {
            if !request
                .cookies
                .iter()
                .any(|(existing, _)| existing == &name)
            {
                request.cookies.push((name, value));
            }
        }
        request
    }

    /// Fetch a request, using the cache when one is given.
    pub async fn fetch(
        &self,
        request: EngineRequest,
        timeout: Duration,
        cache: Option<&Cache>,
        proxy: Option<&str>,
    ) -> Result<EngineResponse, NetworkError> {
        self.fetch_inner(request, timeout, cache, proxy, None).await
    }

    /// Fetch an uncached image with a body limit.
    pub(crate) async fn fetch_image(
        &self,
        request: EngineRequest,
        timeout: Duration,
        max_bytes: usize,
    ) -> Result<EngineResponse, NetworkError> {
        self.fetch_inner(request, timeout, None, None, Some(max_bytes))
            .await
    }

    async fn fetch_inner(
        &self,
        request: EngineRequest,
        timeout: Duration,
        cache: Option<&Cache>,
        proxy: Option<&str>,
        max_bytes: Option<usize>,
    ) -> Result<EngineResponse, NetworkError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let client = if max_bytes.is_some() {
            if self.media_proxy_blocked {
                return Err(NetworkError::Build(
                    "media reads require direct connections; configured proxies cannot validate destinations".into(),
                ));
            }
            let url = reqwest::Url::parse(&request.url)
                .map_err(|error| NetworkError::Build(error.to_string()))?;
            if !crate::mcp::page::is_fetchable(&url) {
                return Err(NetworkError::Build(
                    "only public http(s) media can be fetched".into(),
                ));
            }
            self.media_client.clone()
        } else {
            self.client_for(proxy)?
        };
        let request = self.with_solver_cookies(request);
        let mut headers: Vec<_> = request
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_bytes()))
            .collect();
        headers.sort_by_key(|(name, _)| *name);
        let identity = serde_json::to_vec(&(
            request.method,
            &request.url,
            headers,
            &request.cookies,
            &request.form,
            &request.json,
            proxy.map(str::trim),
        ))
        .map_err(|error| NetworkError::Build(error.to_string()))?;
        let cache_key = format!("{:x}", Sha256::digest(identity));
        if let Some(cache) = cache {
            if let Some(cached) = tokio::time::timeout_at(deadline, cache.get(&cache_key))
                .await
                .map_err(|_| NetworkError::Timeout)?
            {
                debug!(url = %request.url, "cache hit");
                return Ok(EngineResponse {
                    status: cached.status,
                    final_url: cached.final_url,
                    headers: cached.headers,
                    body: cached.body.as_ref().clone(),
                });
            }
        }

        let mut attempt = 0;
        loop {
            attempt += 1;
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(NetworkError::Timeout);
            }
            let outcome = self
                .send(client.clone(), &request, remaining, max_bytes)
                .await;
            match outcome {
                Ok(response) => {
                    let marker = String::from_utf8_lossy(&response.body);
                    tracing::debug!(url = %request.url, status = response.status, bytes = response.body.len(), anubis_marker = marker.contains("anubis_challenge"), anubis_parsed = anubis_challenge(&response.body).is_some(), "fetch response");
                    if marker.contains("anubis_challenge") && std::env::var("RUSTXNG_DUMP").is_ok()
                    {
                        let _ = std::fs::write("/tmp/anubis_body.html", &response.body);
                    }
                    // Anubis guarded sites answer 200 with a small proof of work
                    // page instead of results. Solve it, keep the cookies and
                    // replay the request once.
                    let guard =
                        response.status == 200 && anubis_challenge(&response.body).is_some();
                    if guard
                        && attempt <= 2
                        && self.solve_anubis(&response.final_url, &deadline).await
                    {
                        debug!(url = %request.url, attempt, "solved an anubis challenge");
                        continue;
                    }
                    if guard {
                        debug!(
                            url = %request.url,
                            attempt,
                            bytes = response.body.len(),
                            cookies = self.jar_cookies(&response.final_url).len(),
                            "anubis: the guard page survived the replay"
                        );
                    }
                    // The cookie that used to open this host has expired, or the
                    // site rotated its challenge: replaying it only answers a
                    // refusal, so drop it and let the browser win a fresh one.
                    if crate::solver::looks_like_challenge(response.status, &response.body) {
                        if let Some(solver) = self.solver.as_ref() {
                            solver.forget_cookies(&response.final_url);
                        }
                    }
                    // A WAF or CAPTCHA wall is not a page, but who retries it
                    // is the aggregator: only it knows which engine is asking,
                    // how much budget is left and which engines a tab cannot
                    // help at all. Solving here as well spent the engine's
                    // whole budget before the retry could even be decided.
                    if let Some(cache) = cache {
                        if response.status == 200
                            && !response.body.is_empty()
                            && !guard
                            && !crate::solver::looks_like_challenge(response.status, &response.body)
                            && tokio::time::timeout_at(
                                deadline,
                                cache.set(
                                    &cache_key,
                                    CachedResponse {
                                        status: response.status,
                                        final_url: response.final_url.clone(),
                                        headers: response.headers.clone(),
                                        body: std::sync::Arc::new(response.body.clone()),
                                    },
                                ),
                            )
                            .await
                            .is_err()
                        {
                            debug!(url = %request.url, "cache write timed out");
                        }
                    }
                    // A 429 usually clears within a second. Retrying once keeps
                    // a good engine (Yahoo! JAPAN, Bing) alive instead of
                    // suspending it for minutes.
                    if response.status == 429 && attempt <= self.retries {
                        let backoff = Duration::from_millis(700);
                        if tokio::time::Instant::now() + backoff < deadline {
                            debug!(url = %request.url, "rate limited, retrying after a pause");
                            tokio::time::sleep(backoff).await;
                            continue;
                        }
                    }
                    if response.status >= 400 {
                        return Err(NetworkError::Status {
                            status: response.status,
                            body: String::from_utf8_lossy(
                                &response.body[..response.body.len().min(4096)],
                            )
                            .to_string(),
                            response: Box::new(response),
                        });
                    }
                    return Ok(response);
                }
                Err(error) if attempt <= self.retries && is_retryable(&error) => {
                    // never retry a timeout: the engine deadline already elapsed
                    if matches!(error, NetworkError::Timeout) {
                        return Err(error);
                    }
                    let backoff = Duration::from_millis(250 * attempt as u64);
                    if tokio::time::Instant::now() + backoff >= deadline {
                        return Err(error);
                    }
                    debug!(url = %request.url, %error, "retrying request");
                    tokio::time::sleep(backoff).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Ask the browser solver for a page, reusing the session it warmed up.
    ///
    /// The solve is spawned so the caller keeps its own deadline, and the task
    /// is aborted when that deadline passes. Leaving it running looked like a
    /// warm-up but was the opposite: a detached solve keeps its tab occupied for
    /// up to `max_timeout_ms`, so the next search finds every tab busy instead
    /// of warm.
    ///
    /// `proven` confines an engine the browser has never served to the one tab
    /// set aside for trying new engines.
    pub async fn solve_page(&self, url: &str, budget: Duration, proven: bool) -> SolverAttempt {
        let Some(solver) = self.solver.clone() else {
            return SolverAttempt::Busy;
        };
        let budget = solver.budget().min(budget);
        if budget < Duration::from_millis(150) {
            debug!(url, "solver skipped: engine budget already spent");
            return SolverAttempt::Busy;
        }
        debug!(
            url,
            budget_ms = budget.as_millis(),
            "solver: clearing a challenge"
        );
        let target = url.to_string();
        // A search's budget is far too small for a page load, which is why a
        // wall met mid-search is never cleared in time. Hand it to the
        // background instead: the tab finishes on its own, keeps the clearance
        // cookie, and the next search reads the site without a browser at all.
        let warmer = solver.clone();
        let warm_target = target.clone();
        let may_warm = budget < Duration::from_secs(20);
        // The wait is bounded by the same budget the caller is about to spend
        // anyway: a tab that frees up in time rescues the engine, and one that
        // does not costs nothing that was not already lost.
        let handle =
            tokio::spawn(async move { solver.solve_within(&target, budget, proven).await });
        let canceller = handle.abort_handle();
        match tokio::time::timeout(budget, handle).await {
            Ok(Ok(Ok(solved))) => {
                debug!(
                    url,
                    status = solved.status,
                    len = solved.body.len(),
                    head = %solved.body.chars().take(80).collect::<String>(),
                    "solver: solved"
                );
                SolverAttempt::Solved(EngineResponse {
                    status: solved.status,
                    final_url: solved.url,
                    headers: BTreeMap::new(),
                    body: solved.body.into_bytes(),
                })
            }
            Ok(Ok(Err(error))) => {
                debug!(url, %error, "solver could not clear the challenge");
                if refuses(&error) {
                    SolverAttempt::Hopeless
                } else {
                    if may_warm {
                        warmer.warm_in_background(&warm_target);
                    }
                    SolverAttempt::Busy
                }
            }
            Ok(Err(error)) => {
                debug!(url, %error, "solver task failed");
                SolverAttempt::Busy
            }
            Err(_) => {
                // Free the tab for the next search instead of letting the page
                // load run to its own timeout with nobody waiting for it.
                canceller.abort();
                debug!(
                    url,
                    "solver exceeded the engine budget; the tab is released"
                );
                if may_warm {
                    warmer.warm_in_background(&warm_target);
                }
                SolverAttempt::Busy
            }
        }
    }

    /// The user agent that already won this host past Anubis, if any.
    fn pinned_agent(&self, url: &str) -> Option<String> {
        let host = url_host(url);
        if host.is_empty() {
            return None;
        }
        self.anubis_agents
            .lock()
            .ok()
            .and_then(|agents| agents.get(&host).cloned())
    }

    async fn send(
        &self,
        client: reqwest::Client,
        request: &EngineRequest,
        timeout: Duration,
        max_bytes: Option<usize>,
    ) -> Result<EngineResponse, NetworkError> {
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut headers = self.default_headers();
        for (name, value) in request.headers.iter() {
            headers.insert(name.clone(), value.clone());
        }
        // A solved Anubis session only survives while the signature stays the
        // same, so that decision outranks whatever the engine asked for.
        if let Some(agent) = self.pinned_agent(&request.url) {
            if let Ok(value) = HeaderValue::from_str(&agent) {
                headers.insert(reqwest::header::USER_AGENT, value);
            }
        }
        let mut cookies = request.cookies.clone();
        for (name, value) in self.jar_cookies(&request.url) {
            if !cookies.iter().any(|(existing, _)| existing == &name) {
                cookies.push((name, value));
            }
        }
        if !cookies.is_empty() {
            let cookie = cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                headers.insert(reqwest::header::COOKIE, value);
            }
        }

        let mut builder = client.request(method, &request.url).headers(headers);
        if let Some(json) = &request.json {
            builder = builder.json(json);
        } else if request.method == Method::Post {
            builder = builder.form(&request.form);
        }

        let send = async {
            let mut response = builder.send().await.map_err(|error| {
                if error.is_timeout() {
                    NetworkError::Timeout
                } else {
                    NetworkError::Transport(error.to_string())
                }
            })?;
            let status = response.status().as_u16();
            let final_url = response.url().to_string();
            let mut headers = BTreeMap::new();
            for (name, value) in response.headers().iter() {
                if let Ok(value) = value.to_str() {
                    headers.insert(name.as_str().to_lowercase(), value.to_string());
                }
            }
            let body = if let Some(limit) = max_bytes {
                let mut body = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|error| NetworkError::Transport(error.to_string()))?
                {
                    if chunk.len() > limit - body.len() {
                        return Err(NetworkError::TooLarge(limit));
                    }
                    body.extend_from_slice(&chunk);
                }
                body
            } else {
                response
                    .bytes()
                    .await
                    .map_err(|error| NetworkError::Transport(error.to_string()))?
                    .to_vec()
            };
            Ok(EngineResponse {
                status,
                final_url,
                headers,
                body,
            })
        };

        match tokio::time::timeout(timeout, send).await {
            Ok(result) => result,
            Err(_) => Err(NetworkError::Timeout),
        }
    }
}

/// An Anubis proof of work challenge, as embedded in a guard page.
struct AnubisChallenge {
    difficulty: usize,
    random_data: String,
    id: String,
}

/// Pull the `#anubis_challenge` JSON blob out of a response body.
fn anubis_challenge(body: &[u8]) -> Option<AnubisChallenge> {
    let text = std::str::from_utf8(body).ok()?;
    let start = text.find("id=\"anubis_challenge\"")?;
    let rest = &text[start..];
    let open = rest.find('>')? + 1;
    let close = rest[open..].find("</script>")? + open;
    let json: serde_json::Value = serde_json::from_str(&rest[open..close]).ok()?;
    let rules = json.get("rules")?;
    let challenge = json.get("challenge")?;
    let algorithm = rules
        .get("algorithm")
        .and_then(|value| value.as_str())
        .unwrap_or("fast");
    let difficulty = rules.get("difficulty")?.as_u64().unwrap_or(0) as usize;
    // "slow" rules hash far more than once per nonce, and a deep challenge
    // would blow the engine's time budget; leave those to the browser.
    if algorithm != "fast" || difficulty == 0 || difficulty > 7 {
        return None;
    }
    Some(AnubisChallenge {
        difficulty,
        random_data: challenge.get("randomData")?.as_str()?.to_string(),
        id: challenge.get("id")?.as_str()?.to_string(),
    })
}

/// Find the nonce whose SHA-256 digest has `difficulty` leading hex zeros.
///
/// Anubis grades user agents it distrusts with a deeper challenge (a fake
/// browser version earns grade 6 instead of 2), and a single-core sweep
/// cannot finish 16^6 hashes inside an engine's time budget. The sweep is
/// therefore fanned out over the available cores and abandoned at `deadline`.
fn solve_anubis_pow(
    challenge: &AnubisChallenge,
    deadline: tokio::time::Instant,
) -> Option<(u64, String)> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return None;
    }
    let stop_at = std::time::Instant::now() + remaining;
    let bytes = challenge.difficulty / 2;
    let odd = challenge.difficulty % 2 == 1;
    // A digest starts with the requested hex zeros when its leading whole
    // bytes are zero and, for an odd grade, the high nibble of the next byte.
    let matches = move |digest: &[u8]| {
        digest[..bytes].iter().all(|byte| *byte == 0) && (!odd || digest[bytes] & 0xF0 == 0)
    };
    let workers = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .clamp(1, 8) as u64;
    let found = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (sender, receiver) = std::sync::mpsc::channel::<(u64, String)>();
    let mut handles = Vec::with_capacity(workers as usize);
    for worker in 0..workers {
        let sender = sender.clone();
        let data = challenge.random_data.clone();
        let found = found.clone();
        handles.push(std::thread::spawn(move || {
            let mut nonce = worker;
            let mut checked = 0u64;
            while nonce < 5_000_000 && !found.load(std::sync::atomic::Ordering::Relaxed) {
                let mut hasher = Sha256::new();
                hasher.update(data.as_bytes());
                hasher.update(nonce.to_string().as_bytes());
                let digest = hasher.finalize();
                if matches(&digest) {
                    found.store(true, std::sync::atomic::Ordering::Relaxed);
                    let _ = sender.send((nonce, format!("{digest:x}")));
                    return;
                }
                nonce += workers;
                checked += 1;
                if checked % 4096 == 0 && std::time::Instant::now() >= stop_at {
                    return;
                }
            }
        }));
    }
    drop(sender);
    let answer = receiver
        .recv_timeout(stop_at.saturating_duration_since(std::time::Instant::now()))
        .ok();
    found.store(true, std::sync::atomic::Ordering::Relaxed);
    for handle in handles {
        let _ = handle.join();
    }
    answer
}

/// The "name=value" pairs of a response's "Set-Cookie" headers.
fn set_cookie_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for header in headers.get_all(reqwest::header::SET_COOKIE).iter() {
        let Ok(text) = header.to_str() else {
            continue;
        };
        let pair = text.split(';').next().unwrap_or("").trim();
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name.trim().is_empty() {
            continue;
        }
        pairs.push((name.trim().to_string(), value.trim().to_string()));
    }
    pairs
}

/// The "scheme://authority" prefix of an absolute URL.
fn url_origin(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return String::new();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    format!("{scheme}://{authority}")
}

/// The host of an absolute URL, without scheme, port or path.
fn url_host(url: &str) -> String {
    let origin = url_origin(url);
    let Some((_, authority)) = origin.split_once("://") else {
        return String::new();
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, rest)| rest);
    host.split(':').next().unwrap_or("").to_string()
}

fn is_retryable(error: &NetworkError) -> bool {
    match error {
        NetworkError::Transport(_) => true,
        // a timeout already consumed the whole engine budget
        NetworkError::Timeout => false,
        NetworkError::Status { status, .. } => *status == 429 || *status >= 500,
        NetworkError::Build(_) | NetworkError::TooLarge(_) => false,
    }
}

/// The default user agent, mirroring upstream's `searxng_useragent`.
pub fn default_user_agent() -> String {
    build_user_agents()
        .into_iter()
        .next()
        .unwrap_or_else(|| "rustxng".to_string())
}

/// Build the user agent pool like upstream's `data/useragents.json`.
fn build_user_agents() -> Vec<String> {
    let mut agents = Vec::new();
    for os in USER_AGENT_OS {
        for version in USER_AGENT_VERSIONS {
            agents.push(
                USER_AGENT_TEMPLATE
                    .replace("{os}", os)
                    .replace("{version}", version),
            );
        }
    }
    agents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_engine_proxy_does_not_contact_origin() {
        use axum::Router;
        use std::sync::Arc;

        let calls = Arc::new(AtomicUsize::new(0));
        let requests = Arc::clone(&calls);
        let app = Router::new().fallback(move || {
            requests.fetch_add(1, Ordering::Relaxed);
            async { "origin" }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut network = Network::from_settings(&Settings::defaults()).unwrap();
        network.client = reqwest::Client::builder().no_proxy().build().unwrap();

        let result = network
            .fetch(
                EngineRequest::get(url),
                Duration::from_secs(2),
                None,
                Some("http://[::1"),
            )
            .await;
        server.abort();
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert!(matches!(result, Err(NetworkError::Build(_))));
    }

    #[tokio::test]
    async fn engine_proxy_overrides_global_proxy() {
        use axum::Router;

        let mut proxies = Vec::new();
        let mut servers = Vec::new();
        for name in ["global", "engine"] {
            let app = Router::new().fallback(move || async move { name });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            proxies.push(format!("http://{}", listener.local_addr().unwrap()));
            servers.push(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap()
            }));
        }
        let mut settings = Settings::defaults();
        settings
            .outgoing
            .proxies
            .insert("http".into(), proxies[0].clone());
        let network = Network::from_settings(&settings).unwrap();
        let mut responses = Vec::new();
        for proxy in [None, Some(proxies[1].as_str()), Some(proxies[1].as_str())] {
            responses.push(
                network
                    .fetch(
                        EngineRequest::get("http://example.invalid/"),
                        Duration::from_secs(2),
                        None,
                        proxy,
                    )
                    .await,
            );
        }
        for server in servers {
            server.abort();
        }
        let bodies: Vec<_> = responses
            .into_iter()
            .map(|response| response.unwrap().text().to_string())
            .collect();
        assert_eq!(bodies, ["global", "engine", "engine"]);
    }

    #[tokio::test]
    async fn cached_responses_respect_request_cookies_headers_and_forms() {
        use axum::{Router, routing::post};
        use std::sync::Arc;

        let calls = Arc::new(AtomicUsize::new(0));
        let requests = Arc::clone(&calls);
        let app = Router::new().route(
            "/",
            post(move |headers: HeaderMap, body: String| {
                requests.fetch_add(1, Ordering::Relaxed);
                async move {
                    format!(
                        "{}|{}|{}",
                        headers[reqwest::header::COOKIE].to_str().unwrap(),
                        headers[reqwest::header::ACCEPT_LANGUAGE].to_str().unwrap(),
                        body
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let network = Network::from_settings(&Settings::defaults()).unwrap();
        let cache = Cache::default();

        for (safe, language, form, expected) in [
            ("off", "ja", vec![], "safesearch=off|ja|"),
            ("strict", "ja", vec![], "safesearch=strict|ja|"),
            ("strict", "en", vec![], "safesearch=strict|en|"),
            (
                "strict",
                "en",
                vec![("q", "cats&safe=0"), ("safe", "2")],
                "safesearch=strict|en|q=cats%26safe%3D0&safe=2",
            ),
            (
                "strict",
                "en",
                vec![("q", "cats"), ("safe", "0&safe=2")],
                "safesearch=strict|en|q=cats&safe=0%26safe%3D2",
            ),
        ] {
            let request = EngineRequest::post(&url)
                .cookie("safesearch", safe)
                .header("Accept-Language", language)
                .header("Accept", "text/plain")
                .form(
                    form.into_iter()
                        .map(|(key, value)| (key.into(), value.into()))
                        .collect(),
                );
            let response = network
                .fetch(request, Duration::from_secs(2), Some(&cache), None)
                .await
                .unwrap();
            assert_eq!(response.text(), expected);
        }
        let repeated = EngineRequest::post(&url)
            .header("Accept", "text/plain")
            .header("Accept-Language", "en")
            .cookie("safesearch", "strict");
        let response = network
            .fetch(repeated, Duration::from_secs(2), Some(&cache), None)
            .await
            .unwrap();
        assert_eq!(response.text(), "safesearch=strict|en|");
        assert_eq!(calls.load(Ordering::Relaxed), 5);
        server.abort();
    }

    #[test]
    fn invalid_ca_bundle_fails_instead_of_disabling_verification() {
        let mut outgoing = crate::settings::Outgoing::default();
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"not a certificate").unwrap();
        outgoing.ca_bundle = Some(file.path().to_string_lossy().into_owned());
        assert!(build_client(&outgoing, None).is_err());
        outgoing.ca_bundle = Some("/nonexistent/rustxng-ca.pem".into());
        assert!(build_client(&outgoing, None).is_err());
        assert!(outgoing.verify);
    }

    #[test]
    fn user_agents_are_diverse_and_plausible() {
        let network = Network {
            client: reqwest::Client::new(),
            media_client: reqwest::Client::new(),
            media_proxy_blocked: false,
            proxied: std::sync::Mutex::new(std::collections::HashMap::new()),
            outgoing: crate::settings::Outgoing::default(),
            user_agents: build_user_agents(),
            suffix: String::new(),
            retries: 0,
            counter: AtomicUsize::new(0),
            anubis_cookies: std::sync::Mutex::new(BTreeMap::new()),
            anubis_agents: std::sync::Mutex::new(BTreeMap::new()),
            bare: reqwest::Client::new(),
            solver: None,
        };
        let first = network.user_agent();
        let second = network.user_agent();
        assert!(first.starts_with("Mozilla/5.0"));
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn media_does_not_inherit_private_engine_endpoints() {
        use axum::{Router, routing::get};
        use std::sync::Arc;
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let app = Router::new().route(
            "/",
            get(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { "engine" }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut settings = Settings::defaults();
        let mut engine = crate::settings::EngineConfig::new("private", "mwmbl", "", &["general"]);
        engine.base_url = Some(url.clone());
        settings.engines = vec![engine];
        let network = Network::from_settings(&settings).unwrap();
        assert!(matches!(
            network
                .fetch_image(EngineRequest::get(&url), Duration::from_secs(2), 32)
                .await,
            Err(NetworkError::Build(_))
        ));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            network
                .fetch(EngineRequest::get(&url), Duration::from_secs(2), None, None)
                .await
                .unwrap()
                .text(),
            "engine"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn media_redirects_cannot_contact_private_targets() {
        use axum::{Router, routing::get};
        use std::sync::Arc;
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let target = Router::new().fallback(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { "private" }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = listener.local_addr().unwrap().port();
        let private_server =
            tokio::spawn(async move { axum::serve(listener, target).await.unwrap() });
        let app = Router::new()
            .route(
                "/literal",
                get(move || async move {
                    axum::response::Redirect::temporary(&format!("http://127.0.0.1:{target_port}/"))
                }),
            )
            .route(
                "/name",
                get(move || async move {
                    axum::response::Redirect::temporary(&format!("http://localhost:{target_port}/"))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut network = Network::from_settings(&Settings::defaults()).unwrap();
        network.media_client = crate::mcp::page::public_client(
            &crate::settings::Outgoing::default(),
            &[("example.com".into(), address)],
        )
        .unwrap()
        .0;
        network.retries = 0;
        for path in ["literal", "name"] {
            let url = format!("http://example.com:{}/{path}", address.port());
            assert!(
                network
                    .fetch_image(EngineRequest::get(url), Duration::from_secs(2), 32)
                    .await
                    .is_err()
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        server.abort();
        private_server.abort();
    }

    #[tokio::test]
    async fn media_size_limit_stops_before_end_of_stream() {
        use axum::{Router, body::Body, routing::get};
        use futures::StreamExt;
        let app = Router::new().route(
            "/",
            get(|| async {
                Body::from_stream(
                    futures::stream::once(async {
                        Ok::<_, std::convert::Infallible>(vec![b'x'; 33])
                    })
                    .chain(futures::stream::pending()),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut network = Network::from_settings(&Settings::defaults()).unwrap();
        network.media_client = crate::mcp::page::public_client(
            &crate::settings::Outgoing::default(),
            &[("example.com".into(), address)],
        )
        .unwrap()
        .0;
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            network.fetch_image(
                EngineRequest::get(format!("http://example.com:{}/", address.port())),
                Duration::from_secs(8),
                32,
            ),
        )
        .await
        .expect("must reject without waiting for EOF");
        assert!(matches!(result, Err(NetworkError::TooLarge(32))));
        server.abort();
    }

    #[tokio::test]
    async fn image_limits_apply_to_decoded_bytes_and_accept_the_exact_limit() {
        use axum::{Router, routing::get};
        use std::sync::Arc;

        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let app = Router::new()
            .route(
                "/exact",
                get(|| async {
                    (
                        [(reqwest::header::CONTENT_TYPE, "image/png")],
                        vec![b'x'; 32],
                    )
                }),
            )
            .route(
                "/compressed",
                get(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                    async {
                        // Compressed 33 bytes; decoded limit is 32.
                        (
                            [
                                (reqwest::header::CONTENT_TYPE, "image/png"),
                                (reqwest::header::CONTENT_ENCODING, "gzip"),
                            ],
                            vec![
                                31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 171, 168, 32, 0, 0, 172, 211, 191,
                                111, 33, 0, 0, 0,
                            ],
                        )
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut network = Network::from_settings(&Settings::defaults()).unwrap();
        network.media_client = crate::mcp::page::public_client(
            &crate::settings::Outgoing::default(),
            &[("example.com".into(), address)],
        )
        .unwrap()
        .0;
        network.retries = 2;
        let response = network
            .fetch_image(
                EngineRequest::get(format!("http://example.com:{}/exact", address.port())),
                Duration::from_secs(2),
                32,
            )
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.header("content-type"), Some("image/png"));
        assert_eq!(response.body, [b'x'; 32]);
        let error = network
            .fetch_image(
                EngineRequest::get(format!("http://example.com:{}/compressed", address.port())),
                Duration::from_secs(2),
                32,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, NetworkError::TooLarge(32)), "{error}");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[test]
    fn a_solver_error_that_reports_a_ban_is_hopeless_whatever_its_shape() {
        use crate::solver::SolverError;
        // FlareSolverr's JSON error shape.
        assert!(refuses(&SolverError::Failed(
            "Error: Error solving the challenge. Cloudflare has blocked this request.".to_string()
        )));
        // A non-200 reply to the API call, with the same sentence in its body.
        assert!(refuses(&SolverError::Transport(
            "500 Internal Server Error: {\"status\": \"error\", \"message\": \"Error: Error solving the challenge. Cloudflare has blocked this request. Probably your IP is banned for this site\"}"
                .to_string()
        )));
        // A page that merely failed to load says nothing about the address.
        assert!(!refuses(&SolverError::Transport(
            "request timed out".to_string()
        )));
        assert!(!refuses(&SolverError::Busy));
    }

    #[test]
    fn captcha_pages_are_recognised() {
        assert_eq!(
            classify_status(403, "please solve the captcha"),
            FailureKind::Captcha
        );
        assert_eq!(classify_status(403, "denied"), FailureKind::AccessDenied);
        assert_eq!(classify_status(429, ""), FailureKind::TooManyRequests);
        assert_eq!(classify_status(500, ""), FailureKind::Http);
    }
}
