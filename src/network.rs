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

const USER_AGENT_OS: &[&str] = &[
    "Windows NT 10.0; Win64; x64",
    "X11; Linux x86_64",
    "Macintosh; Intel Mac OS X 10.15",
    "X11; Ubuntu; Linux x86_64",
];
const USER_AGENT_VERSIONS: &[&str] = &["157.0", "156.0", "155.0", "154.0"];
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
    /// Fixed, honest User-Agent used instead of the rotating pool.
    identified: Option<String>,
    /// The honest User-Agent sent to hosts that publish an API policy, even
    /// when the instance as a whole does not identify itself.
    api_agent: String,
    /// Minimum pause between two requests to one host.
    host_spacing: Duration,
    /// Per-domain exceptions to `host_spacing` (domain, pause).
    host_spacing_overrides: Vec<(String, Duration)>,
    /// Earliest start of the next request per host.
    host_slots: std::sync::Mutex<std::collections::HashMap<String, tokio::time::Instant>>,
}

/// Build a client from the outgoing settings, optionally through another proxy.
fn build_client(
    outgoing: &crate::settings::Outgoing,
    extra_proxy: Option<reqwest::Proxy>,
    redirects: reqwest::redirect::Policy,
) -> Result<reqwest::Client, NetworkError> {
    let mut builder = reqwest::Client::builder()
        .redirect(redirects)
        .timeout(Duration::from_secs_f64(
            outgoing.max_request_timeout.max(1.0),
        ))
        .connect_timeout(Duration::from_secs_f64(
            (outgoing.request_timeout / 2.0).clamp(1.0, 5.0),
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

/// A normal (non-public-only) client for a trusted API endpoint.
/// It honours the outgoing proxy, TLS and CA settings, but never follows
/// redirects: the configured endpoint is fixed and a redirect must not make
/// the local process contact another destination.
pub(crate) fn api_client(
    outgoing: &crate::settings::Outgoing,
) -> Result<reqwest::Client, NetworkError> {
    build_client(outgoing, None, reqwest::redirect::Policy::none())
}

impl Network {
    #[cfg(test)]
    pub(crate) fn with_test_media_client(mut self, client: reqwest::Client) -> Self {
        self.media_client = client;
        self.media_proxy_blocked = false;
        self
    }

    pub fn from_settings(settings: &Settings) -> Result<Self, NetworkError> {
        let outgoing = &settings.outgoing;
        let client = build_client(
            outgoing,
            None,
            reqwest::redirect::Policy::limited(outgoing.max_redirects),
        )?;
        let (media_client, media_proxy_blocked) =
            crate::fetch::public_client(outgoing, &[]).map_err(NetworkError::Build)?;
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
            api_agent: identified_user_agent(&outgoing.contact),
            identified: outgoing
                .identified_ua
                .then(|| identified_user_agent(&outgoing.contact)),
            host_spacing: Duration::from_millis(outgoing.host_spacing_ms),
            host_spacing_overrides: outgoing
                .host_spacing_overrides
                .iter()
                .map(|(domain, millis)| {
                    (
                        domain.trim().trim_start_matches('.').to_lowercase(),
                        Duration::from_millis(*millis),
                    )
                })
                .collect(),
            host_slots: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// The pause between two requests to `host`: the most specific override
    /// (exact host or a parent domain), else the general spacing.
    fn spacing_for(&self, host: &str) -> Duration {
        self.host_spacing_overrides
            .iter()
            .filter(|(domain, _)| {
                host == domain
                    || host
                        .strip_suffix(domain.as_str())
                        .is_some_and(|rest| rest.ends_with('.'))
            })
            .max_by_key(|(domain, _)| domain.len())
            .map_or(self.host_spacing, |(_, spacing)| *spacing)
    }

    /// Reserve this host's next request slot and wait for it.
    ///
    /// Returns `Timeout` when the slot starts after the request's own budget,
    /// so a busy host is skipped instead of being asked early.
    async fn wait_for_host_slot(&self, url: &str, budget: Duration) -> Result<(), NetworkError> {
        if self.host_spacing.is_zero() && self.host_spacing_overrides.is_empty() {
            return Ok(());
        }
        let Some(host) = reqwest::Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_lowercase))
        else {
            return Ok(());
        };
        let spacing = self.spacing_for(&host);
        if spacing.is_zero() {
            return Ok(());
        }
        let now = tokio::time::Instant::now();
        let slot = {
            let Ok(mut slots) = self.host_slots.lock() else {
                return Ok(());
            };
            let next = slots.get(&host).copied().unwrap_or(now).max(now);
            if next.saturating_duration_since(now) >= budget {
                return Err(NetworkError::Timeout);
            }
            slots.insert(host, next + spacing);
            next
        };
        tokio::time::sleep_until(slot).await;
        Ok(())
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
        let client = build_client(
            &self.outgoing,
            Some(configured),
            reqwest::redirect::Policy::limited(self.outgoing.max_redirects),
        )?;
        if let Ok(mut cache) = self.proxied.lock() {
            cache.insert(proxy.to_string(), client.clone());
        }
        Ok(client)
    }

    /// A different user agent on every call, like upstream's rotation.
    pub fn user_agent(&self) -> String {
        if let Some(identified) = &self.identified {
            return identified.clone();
        }
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
            if !crate::fetch::is_fetchable(&url) {
                return Err(NetworkError::Build(
                    "only public http(s) media can be fetched".into(),
                ));
            }
            self.media_client.clone()
        } else {
            self.client_for(proxy)?
        };
        let cache_key = cache
            .map(|_| request_cache_key(&request, proxy))
            .transpose()?;
        // The fast path needs no request lock. On a miss, only one caller for
        // this cache and full request identity contacts the origin at a time.
        let _request_guard = if let (Some(cache), Some(key)) = (cache, cache_key.as_deref()) {
            if let Some(cached) = tokio::time::timeout_at(deadline, cache.get(key))
                .await
                .map_err(|_| NetworkError::Timeout)?
            {
                return Ok(cached_response(cached));
            }
            let guard = tokio::time::timeout_at(deadline, cache.request_lock(key).lock_owned())
                .await
                .map_err(|_| NetworkError::Timeout)?;
            // A previous caller may have populated the cache while we waited.
            if let Some(cached) = tokio::time::timeout_at(deadline, cache.get(key))
                .await
                .map_err(|_| NetworkError::Timeout)?
            {
                debug!(url = %request.url, "cache hit");
                return Ok(cached_response(cached));
            }
            Some(guard)
        } else {
            None
        };

        let mut attempt = 0;
        loop {
            attempt += 1;
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(NetworkError::Timeout);
            }
            let outcome = match tokio::time::timeout_at(
                deadline,
                self.send(client.clone(), &request, remaining, max_bytes),
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(_) => return Err(NetworkError::Timeout),
            };
            match outcome {
                Ok(response) => {
                    if let (Some(cache), Some(key)) = (cache, cache_key.as_deref()) {
                        if response.status == 200
                            && !response.body.is_empty()
                            && tokio::time::timeout_at(
                                deadline,
                                cache.set(
                                    key,
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
        self.wait_for_host_slot(&request.url, timeout).await?;
        let mut headers = self.default_headers();
        // Hosts that publish an API policy ask clients to name themselves, so
        // they never see the browser-like rotation.
        if is_documented_api_host(&request.url) {
            if let Ok(value) = HeaderValue::from_str(&self.api_agent) {
                headers.insert(reqwest::header::USER_AGENT, value);
            }
        }
        for (name, value) in request.headers.iter() {
            headers.insert(name.clone(), value.clone());
        }
        if !request.cookies.is_empty() {
            let cookie = request
                .cookies
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

fn cached_response(cached: CachedResponse) -> EngineResponse {
    EngineResponse {
        status: cached.status,
        final_url: cached.final_url,
        headers: cached.headers,
        body: cached.body.as_ref().clone(),
    }
}

fn request_cache_key(request: &EngineRequest, proxy: Option<&str>) -> Result<String, NetworkError> {
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
    use std::fmt::Write;
    let mut key = String::with_capacity(64);
    for byte in Sha256::digest(identity) {
        write!(&mut key, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(key)
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

/// Documented API hosts where RustXNG uses its identified User-Agent.
const DOCUMENTED_API_SUFFIXES: &[&str] = &[
    "wikipedia.org",
    "wikidata.org",
    "wikimedia.org",
    "wikibooks.org",
    "wikinews.org",
    "wikiquote.org",
    "wikisource.org",
    "wikiversity.org",
    "wikivoyage.org",
    "wiktionary.org",
    "api.github.com",
    "crates.io",
    "pypi.org",
    "registry.npmjs.org",
    "api.npms.io",
    "api.crossref.org",
    "api.openalex.org",
    "api.semanticscholar.org",
    "api.stackexchange.com",
    "export.arxiv.org",
    "api.openverse.org",
    "api.openverse.engineering",
    "nominatim.openstreetmap.org",
    "photon.komoot.io",
    "api.open-meteo.com",
    "geocoding-api.open-meteo.com",
    "api.mwmbl.org",
    "api2.marginalia-search.com",
    "qiita.com",
    "hn.algolia.com",
    "www.ebi.ac.uk",
];

/// Whether the URL points at a host with a published API policy.
pub fn is_documented_api_host(url: &str) -> bool {
    let Some(host) = reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_lowercase))
    else {
        return false;
    };
    DOCUMENTED_API_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// The User-Agent of an instance that identifies itself (`identified_ua`).
pub fn identified_user_agent(contact: &str) -> String {
    let base = format!(
        "rustxng/{} (+https://github.com/sahenjp/RustXNG",
        env!("CARGO_PKG_VERSION")
    );
    match contact.trim() {
        "" => format!("{base})"),
        contact => format!("{base}; {contact})"),
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
        assert!(build_client(&outgoing, None, reqwest::redirect::Policy::none()).is_err());
        outgoing.ca_bundle = Some("/nonexistent/rustxng-ca.pem".into());
        assert!(build_client(&outgoing, None, reqwest::redirect::Policy::none()).is_err());
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
            api_agent: identified_user_agent(""),
            identified: None,
            host_spacing: Duration::ZERO,
            host_spacing_overrides: Vec::new(),
            host_slots: std::sync::Mutex::new(std::collections::HashMap::new()),
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
        let client = crate::fetch::public_client(
            &crate::settings::Outgoing::default(),
            &[("example.com".into(), address)],
        )
        .unwrap()
        .0;
        let mut network = Network::from_settings(&Settings::defaults())
            .unwrap()
            .with_test_media_client(client);
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
        let client = crate::fetch::public_client(
            &crate::settings::Outgoing::default(),
            &[("example.com".into(), address)],
        )
        .unwrap()
        .0;
        let network = Network::from_settings(&Settings::defaults())
            .unwrap()
            .with_test_media_client(client);
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
        let client = crate::fetch::public_client(
            &crate::settings::Outgoing::default(),
            &[("example.com".into(), address)],
        )
        .unwrap()
        .0;
        let mut network = Network::from_settings(&Settings::defaults())
            .unwrap()
            .with_test_media_client(client);
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
    fn captcha_pages_are_recognised() {
        assert_eq!(
            classify_status(403, "please solve the captcha"),
            FailureKind::Captcha
        );
        assert_eq!(classify_status(403, "denied"), FailureKind::AccessDenied);
        assert_eq!(classify_status(429, ""), FailureKind::TooManyRequests);
        assert_eq!(classify_status(500, ""), FailureKind::Http);
    }

    fn network_with(configure: impl FnOnce(&mut crate::settings::Outgoing)) -> Network {
        let mut settings = Settings::default();
        configure(&mut settings.outgoing);
        Network::from_settings(&settings).expect("network")
    }

    #[test]
    fn documented_api_hosts_are_recognised_exactly() {
        assert!(is_documented_api_host(
            "https://en.wikipedia.org/w/api.php?x=1"
        ));
        assert!(is_documented_api_host("https://crates.io/api/v1/crates"));
        assert!(is_documented_api_host(
            "https://api.github.com/search/repositories"
        ));
        assert!(is_documented_api_host(
            "https://api2.marginalia-search.com/search"
        ));
        assert!(is_documented_api_host("https://qiita.com/api/v2/items"));
        assert!(!is_documented_api_host("https://github.com/search"));
        assert!(!is_documented_api_host("https://evilcrates.io/"));
        assert!(!is_documented_api_host("https://www.bing.com/search"));
    }

    #[test]
    fn an_identified_instance_always_sends_the_same_honest_user_agent() {
        let network = network_with(|outgoing| {
            outgoing.identified_ua = true;
            outgoing.contact = "ops@example.org".into();
        });
        let first = network.user_agent();
        assert_eq!(first, network.user_agent());
        assert!(first.starts_with("rustxng/"), "{first}");
        assert!(first.contains("ops@example.org"));
        assert!(!first.contains("Mozilla"));
        // The default stays the rotating pool.
        assert!(network_with(|_| {}).user_agent().starts_with("Mozilla/5.0"));
    }

    #[test]
    fn a_domain_can_have_its_own_spacing() {
        let network = network_with(|outgoing| {
            outgoing.host_spacing_ms = 1100;
            outgoing
                .host_spacing_overrides
                .insert("wikipedia.org".into(), 200);
            outgoing
                .host_spacing_overrides
                .insert("ja.wikipedia.org".into(), 0);
        });
        assert_eq!(
            network.spacing_for("en.wikipedia.org"),
            Duration::from_millis(200)
        );
        assert_eq!(network.spacing_for("ja.wikipedia.org"), Duration::ZERO);
        assert_eq!(
            network.spacing_for("crates.io"),
            Duration::from_millis(1100)
        );
        assert_eq!(
            network.spacing_for("notwikipedia.org"),
            Duration::from_millis(1100)
        );
    }

    #[tokio::test]
    async fn requests_to_one_host_are_spaced_and_a_busy_host_is_skipped() {
        let network = network_with(|outgoing| outgoing.host_spacing_ms = 200);
        let budget = Duration::from_secs(5);
        let started = std::time::Instant::now();
        network
            .wait_for_host_slot("https://api.example.org/a", budget)
            .await
            .unwrap();
        // A different host does not wait.
        network
            .wait_for_host_slot("https://other.example.org/a", budget)
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));
        network
            .wait_for_host_slot("https://api.example.org/b", budget)
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(190));
        // The next slot is 200 ms away: a 50 ms budget gives up instead.
        let result = network
            .wait_for_host_slot("https://api.example.org/c", Duration::from_millis(50))
            .await;
        assert!(matches!(result, Err(NetworkError::Timeout)));
    }
}
