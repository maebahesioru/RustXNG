//! Optional external challenge solver.
//!
//! Engines behind a WAF or an interactive CAPTCHA cannot be reached with a
//! plain HTTP client: the page is a verification wall, not results. A solver
//! drives a real browser, clears the wall once and hands back the rendered
//! HTML. The engine budget still decides how long a search may wait; a solve
//! that outlives it is cancelled and its tab released, because a tab held by an
//! abandoned page load is worse than a cold one.
//!
//! The wire protocol is the one FlareSolverr exposes on `POST /` (JSON with
//! `cmd`, `url`, `session` and `maxTimeout`), because that is the widest
//! supported OSS implementation. The endpoint is configurable so a drop-in
//! replacement works unchanged.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::settings::SolverSettings;

#[derive(Debug, thiserror::Error)]
pub enum SolverError {
    #[error("solver request failed: {0}")]
    Transport(String),
    #[error("solver reported an error: {0}")]
    Failed(String),
    #[error("solver returned an empty page")]
    Empty,
    #[error("solver is busy")]
    Busy,
}

/// One page as the browser saw it after the challenge was cleared.
#[derive(Debug, Clone)]
pub struct Solved {
    pub status: u16,
    pub body: String,
    pub url: String,
    /// Cookies the browser is holding after the page load.
    pub cookies: Vec<Cookie>,
}

/// One cookie the browser won while clearing a wall.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub path: String,
    /// Seconds since the epoch; zero or negative for a session cookie.
    #[serde(default)]
    pub expires: f64,
}

impl Cookie {
    fn expired(&self, now: f64) -> bool {
        self.expires > 0.0 && self.expires < now
    }
}

/// Body markers that mean "this is a challenge, not the page".
///
/// A 200 only counts for markers that never appear on a real result page; WAF
/// statuses may also be a plain CAPTCHA interstitial.
const STRONG_MARKERS: &[&str] = &[
    "just a moment",
    "cf_chl_opt",
    "checking your browser",
    "verify you are human",
    "are you a robot",
    "ddos protection by",
    "attention required! | cloudflare",
    // Fastly's bot wall (Bandcamp). The asset path is unique to the interstitial,
    // so a real result page carrying the words "client challenge" will not match.
    "_fs-ch-",
    "security filter flagged",
    // Azure's front door (Front Door / Application Gateway WAF) serves a
    // challenge page that keeps the answer until its script has run for a
    // few seconds. The marker is unique to the interstitial.
    "azure waf js challenge",
];

/// Markers a real page can carry as well.
///
/// Cloudflare's interstitial and its normal bot-management script share the
/// `/cdn-cgi/challenge-platform/` path: Vimeo's own result page loads
/// `.../challenge-platform/scripts/precursor/main.js`, so the marker alone
/// says nothing. The interstitial is always a small page, so these only count
/// while the body is short.
const SCRIPT_MARKERS: &[&str] = &["cf-chl", "challenge-platform"];

/// Above this size a page is a document, not a challenge interstitial.
const INTERSTITIAL_MAX_BYTES: usize = 64 * 1024;

const SOFT_MARKERS: &[&str] = &[
    "captcha",
    "unusual traffic",
    "enable javascript and cookies to continue",
    "please complete the security check",
];

/// The page each tab loads at startup.
///
/// Launching the browser costs several seconds, which is more than a search is
/// willing to wait for a single engine. Navigate once at boot so the first
/// blocked search finds a running browser instead of paying that cost.
const WARM_URL: &str = "https://example.com/";

/// How long a page is left to settle when the wall needs its script to run.
///
/// Azure Front Door answers its interstitial for the first seconds even to a
/// real browser and only then loads the page that was asked for.
const SLOW_CHALLENGE_SECONDS: u64 = 8;

/// True when a response is a verification wall rather than a page.
pub fn looks_like_challenge(status: u16, body: &[u8]) -> bool {
    if !matches!(status, 200 | 403 | 429 | 503) {
        return false;
    }
    let window = &body[..body.len().min(32_768)];
    let text = String::from_utf8_lossy(window).to_lowercase();
    let strong = STRONG_MARKERS.iter().any(|marker| text.contains(marker));
    if strong {
        return true;
    }
    if body.len() <= INTERSTITIAL_MAX_BYTES
        && SCRIPT_MARKERS.iter().any(|marker| text.contains(marker))
    {
        return true;
    }
    status != 200 && SOFT_MARKERS.iter().any(|marker| text.contains(marker))
}

/// A browser renders a JSON or plain-text document inside a pre block, so an
/// API request that only a browser can clear comes back as HTML around the
/// payload. Undo that wrapper when the payload really is JSON; a real page that
/// happens to hold a code block keeps its markup.
fn unwrap_json_document(body: String) -> String {
    let head = body.trim_start();
    if !(head.starts_with("<html") || head.starts_with("<!DOCTYPE")) {
        return body;
    }
    let Some(open) = body.find("<pre") else {
        return body;
    };
    let Some(close) = body.rfind("</pre>") else {
        return body;
    };
    let Some(offset) = body[open..close].find('>') else {
        return body;
    };
    let start = open + offset + 1;
    if start >= close {
        return body;
    }
    let decoded = decode_entities(&body[start..close]);
    match decoded.trim_start().as_bytes().first() {
        Some(b'{') | Some(b'[') => decoded,
        _ => body,
    }
}

fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
}

/// One browser tab: a session on the solver side plus the exclusive right to
/// drive it.
struct Slot {
    name: String,
    /// Session id created on the solver side.
    session: Mutex<Option<String>>,
    /// One browser session cannot serve two pages at once.
    gate: Mutex<()>,
}

impl Slot {
    fn new(name: String) -> Self {
        Self {
            name,
            session: Mutex::new(None),
            gate: Mutex::new(()),
        }
    }
}

/// A FlareSolverr-compatible browser session, reused across requests.
///
/// Several tabs are kept so a search that needs three blocked engines at once
/// does not serialise behind one page load and lose every engine to the
/// "busy" shortcut.
pub struct Solver {
    client: reqwest::Client,
    endpoint: String,
    max_timeout_ms: u64,
    budget: Duration,
    slots: Vec<Slot>,
    /// Cookies the browser has won, per host, mirrored to disk.
    jar: StdMutex<Jar>,
    /// One warm-up runs at a time: warming is housekeeping, and a burst of
    /// blocked engines must not turn into a race for the tabs a search needs.
    warm_gate: Mutex<()>,
    /// Whether a search that ran out of time may hand the wall to the
    /// background instead of losing the engine for good.
    warm: bool,
}

#[derive(Deserialize)]
struct Envelope {
    status: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    solution: Option<Solution>,
}

#[derive(Deserialize)]
struct Solution {
    #[serde(default)]
    url: String,
    #[serde(default)]
    status: u16,
    #[serde(default)]
    response: String,
    /// FlareSolverr hands back every cookie the browser holds for the page; the
    /// clearance cookie is what makes the next request skip the wall entirely.
    #[serde(default)]
    cookies: Vec<Cookie>,
}

impl Solver {
    pub fn new(settings: &SolverSettings) -> Result<Self, SolverError> {
        let budget = Duration::from_millis(settings.budget_ms.max(200));
        let max_timeout_ms = settings.max_timeout_ms.max(1_000);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(max_timeout_ms + 15_000))
            .build()
            .map_err(|error| SolverError::Transport(error.to_string()))?;
        let prefix = if settings.session.trim().is_empty() {
            "rustxng".to_string()
        } else {
            settings.session.clone()
        };
        let pool = settings.pool.clamp(1, 8);
        let slots = (0..pool)
            .map(|index| {
                let name = if index == 0 {
                    prefix.clone()
                } else {
                    format!("{prefix}-{}", index + 1)
                };
                Slot::new(name)
            })
            .collect();
        Ok(Self {
            client,
            endpoint: settings.endpoint.clone(),
            max_timeout_ms,
            budget,
            slots,
            jar: StdMutex::new(Jar::load(settings.cookie_file.as_deref())),
            warm_gate: Mutex::new(()),
            warm: settings.warm,
        })
    }

    /// Cookies the browser won for this url's host, ready to replay.
    pub fn jar_cookies(&self, url: &str) -> Vec<(String, String)> {
        let host = url_host(url);
        match self.jar.lock() {
            Ok(jar) => jar.cookies_for(&host),
            Err(_) => Vec::new(),
        }
    }

    /// Drop the cookies remembered for this url's host.
    ///
    /// A clearance cookie expires silently: the site starts answering with the
    /// challenge again while the jar still claims the wall is cleared. Replaying
    /// a dead cookie fails exactly like sending none, so forget it as soon as a
    /// plain request meets a challenge and let the browser win a fresh one.
    pub fn forget_cookies(&self, url: &str) {
        let host = url_host(url);
        if let Ok(mut jar) = self.jar.lock() {
            jar.forget(&host);
        }
    }

    /// Clear a wall in the background so later searches never queue for a tab.
    ///
    /// A search budgets three seconds for engines and a browser page load needs
    /// more, so a challenge-guarded engine loses inside a general search no
    /// matter how well the solver works. Clearing it once, off the request path,
    /// leaves the cookie in the jar: the next search fetches the engine with a
    /// plain HTTP request and answers in milliseconds.
    pub fn warm_in_background(self: &std::sync::Arc<Self>, url: &str, dwell: Option<u64>) {
        if !self.warm || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let host = url_host(url);
        {
            let Ok(mut jar) = self.jar.lock() else {
                return;
            };
            if !jar.should_warm(&host) {
                return;
            }
        }
        let solver = self.clone();
        let target = url.to_string();
        tokio::spawn(async move {
            let _serial = solver.warm_gate.lock().await;
            let wait = Duration::from_millis(solver.max_timeout_ms.saturating_mul(2).max(30_000));
            let started = Instant::now();
            let outcome = solver.solve_within(&target, wait, false, dwell).await;
            match outcome {
                Ok(solved) => debug!(
                    url = %target,
                    ms = started.elapsed().as_millis(),
                    len = solved.body.len(),
                    cookies = solved.cookies.len(),
                    "solver: warmed a wall in the background"
                ),
                Err(error) => debug!(
                    url = %target,
                    ms = started.elapsed().as_millis(),
                    %error,
                    "solver: background warm-up failed"
                ),
            }
            if let Ok(mut jar) = solver.jar.lock() {
                jar.warmed(&host);
            }
        });
    }

    /// Keep the cookies a solved page handed back.
    fn remember(&self, url: &str, cookies: &[Cookie]) {
        if cookies.is_empty() {
            return;
        }
        let host = url_host(url);
        if let Ok(mut jar) = self.jar.lock() {
            jar.remember(&host, cookies);
            debug!(host = %host, cookies = cookies.len(), "solver: keeping the cookies it won");
        }
    }

    /// How long a single search is willing to wait for the browser.
    pub fn budget(&self) -> Duration {
        self.budget
    }

    /// Start every browser tab now instead of on the first blocked search.
    ///
    /// A cold tab needs five to seven seconds to launch, which no engine budget
    /// survives; a warm one answers in about a second. The work happens in the
    /// background so startup does not wait for it.
    pub fn prewarm(self: &std::sync::Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        for index in 0..self.slots.len() {
            let solver = self.clone();
            tokio::spawn(async move {
                let slot = &solver.slots[index];
                let Ok(_guard) = slot.gate.try_lock() else {
                    return;
                };
                let started = std::time::Instant::now();
                let session = match solver.session(slot).await {
                    Ok(session) => session,
                    Err(error) => {
                        debug!(slot = %slot.name, %error, "solver could not open a tab");
                        return;
                    }
                };
                match solver.get(&session, WARM_URL, None).await {
                    Ok(_) => debug!(
                        slot = %slot.name,
                        ms = started.elapsed().as_millis(),
                        "solver tab warmed up"
                    ),
                    Err(error) => {
                        debug!(slot = %slot.name, %error, "solver tab warm-up failed");
                    }
                }
            });
        }
    }

    /// Whether a tab is idle this instant.
    ///
    /// Engines the solver has never helped only take a tab that is free right
    /// now. Letting every blocked engine queue turned a dozen hopeless
    /// challenges into one race for three tabs, and the engines the browser
    /// can actually serve lost that race on every search.
    pub fn slot_free(&self) -> bool {
        self.slots.iter().any(|slot| slot.gate.try_lock().is_ok())
    }

    /// Fetch `url` through a free browser tab, clearing any challenge.
    ///
    /// A search only budgets a few seconds for the browser, so the wait for a
    /// free tab is bounded by that budget: queueing longer would fail the
    /// engine as well.
    ///
    /// An engine the browser has never served is confined to one tab, the
    /// last one. A dozen engines whose walls the browser cannot pass would
    /// otherwise hold every tab for the whole budget on every search, and the
    /// few engines it can serve would never get one to prove themselves with.
    /// Partitioned this way, trying a new engine costs the working ones
    /// nothing but a wait.
    pub async fn solve_within(
        &self,
        url: &str,
        wait: Duration,
        proven: bool,
        dwell: Option<u64>,
    ) -> Result<Solved, SolverError> {
        let deadline = tokio::time::Instant::now() + wait;
        let last = self.slots.len() - 1;
        loop {
            let candidates: &[Slot] = if proven {
                &self.slots
            } else {
                &self.slots[last..]
            };
            for slot in candidates {
                if let Ok(guard) = slot.gate.try_lock() {
                    let result = self.solve_in(slot, url, dwell).await;
                    drop(guard);
                    return result;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(SolverError::Busy);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn solve_in(
        &self,
        slot: &Slot,
        url: &str,
        dwell: Option<u64>,
    ) -> Result<Solved, SolverError> {
        let mut fresh_profile_used = false;
        let mut dwelt = false;
        loop {
            let session = self.session(slot).await?;
            // An engine that names its own settle time has a page whose results
            // are drawn by script: there is no challenge to recognise, so the
            // wait has to happen on the first read or the shell is all we get.
            let settle = dwell.or(dwelt.then_some(SLOW_CHALLENGE_SECONDS));
            match self.get(&session, url, settle).await {
                Ok(solved) => {
                    // A CloudFront-style refusal has no challenge to clear: the
                    // page says the browser is not welcome, so another tab cannot
                    // help. Report the block instead of handing the engine a body
                    // that can only parse into zero results.
                    if refused_page(&solved) {
                        return Err(SolverError::Failed(format!(
                            "the site blocked the browser outright ({} bytes, status {})",
                            solved.body.len(),
                            solved.status
                        )));
                    }
                    // Some walls only answer once their challenge script has
                    // had the time to run: Azure Front Door serves the
                    // interstitial for the first seconds even to a real
                    // browser. Keep the tab and ask the same URL again, this
                    // time letting the page settle before it is read.
                    if !dwelt && looks_like_challenge(solved.status, solved.body.as_bytes()) {
                        dwelt = true;
                        debug!(
                            session = %slot.name,
                            len = solved.body.len(),
                            "solver came back with a challenge; letting the page settle"
                        );
                        continue;
                    }
                    // Other walls let a brand new browser profile through and
                    // refuse the same one afterwards (magnific answers
                    // "Forbidden" to every request after the first). A used
                    // profile is worthless there, so trade it for a fresh one
                    // once before giving up.
                    if !fresh_profile_used && stale_block_page(&solved) {
                        fresh_profile_used = true;
                        warn!(
                            session = %slot.name,
                            len = solved.body.len(),
                            "solver got a bare block page; starting a fresh browser profile"
                        );
                        self.discard(slot).await;
                        continue;
                    }
                    // Both remedies are spent. A wall that is still standing
                    // leaves the engine nothing to parse, so say so instead of
                    // reporting an empty result set (`silent empty`).
                    if looks_like_challenge(solved.status, solved.body.as_bytes())
                        || stale_block_page(&solved)
                    {
                        return Err(SolverError::Failed(format!(
                            "the wall survived the solver ({} bytes, status {}); the site blocked it",
                            solved.body.len(),
                            solved.status
                        )));
                    }
                    return Ok(solved);
                }
                // A browser tab can die between searches; recreate the session
                // once instead of failing the engine for the rest of the run.
                Err(SolverError::Failed(message))
                    if !fresh_profile_used && mentions_session(&message) =>
                {
                    fresh_profile_used = true;
                    warn!(session = %slot.name, %message, "solver session lost; recreating it");
                    self.discard(slot).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Throw the browser profile away so the next attempt starts clean.
    async fn discard(&self, slot: &Slot) {
        let mut guard = slot.session.lock().await;
        let Some(name) = guard.take() else {
            return;
        };
        drop(guard);
        let reply = self
            .post(serde_json::json!({
                "cmd": "sessions.destroy",
                "session": name,
            }))
            .await;
        if let Err(error) = reply {
            debug!(%error, session = %name, "solver could not drop the browser profile");
        }
    }

    async fn session(&self, slot: &Slot) -> Result<String, SolverError> {
        let mut guard = slot.session.lock().await;
        if let Some(id) = guard.as_ref() {
            return Ok(id.clone());
        }
        let envelope = self
            .post(serde_json::json!({
                "cmd": "sessions.create",
                "session": slot.name,
            }))
            .await?;
        let lowered = envelope.message.to_lowercase();
        if envelope.status != "ok" && !lowered.contains("already exists") {
            return Err(SolverError::Failed(format!(
                "{}: {}",
                envelope.status, envelope.message
            )));
        }
        debug!(session = %slot.name, "created a solver browser session");
        *guard = Some(slot.name.clone());
        Ok(slot.name.clone())
    }

    async fn get(
        &self,
        session: &str,
        url: &str,
        dwell: Option<u64>,
    ) -> Result<Solved, SolverError> {
        let mut payload = serde_json::json!({
            "cmd": "request.get",
            "url": url,
            "session": session,
            "maxTimeout": self.max_timeout_ms,
        });
        if let Some(seconds) = dwell {
            payload["waitInSeconds"] = serde_json::json!(seconds);
        }
        let envelope = self.post(payload).await?;
        if envelope.status != "ok" {
            return Err(SolverError::Failed(format!(
                "{}: {}",
                envelope.status, envelope.message
            )));
        }
        let solution = envelope.solution.ok_or(SolverError::Empty)?;
        if solution.response.trim().is_empty() {
            return Err(SolverError::Empty);
        }
        let solved = Solved {
            status: if solution.status == 0 {
                200
            } else {
                solution.status
            },
            body: unwrap_json_document(solution.response),
            url: if solution.url.trim().is_empty() {
                url.to_string()
            } else {
                solution.url
            },
            cookies: solution.cookies,
        };
        // The clearance cookie is the whole point of paying for a browser: keep
        // it before the page is handed back, so a solved wall stays solved even
        // if the caller drops the response.
        self.remember(url, &solved.cookies);
        Ok(solved)
    }

    async fn post(&self, payload: serde_json::Value) -> Result<Envelope, SolverError> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await
            .map_err(|error| SolverError::Transport(error.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| SolverError::Transport(error.to_string()))?;
        if !status.is_success() {
            return Err(SolverError::Transport(format!(
                "{status}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        serde_json::from_str(&text).map_err(|error| {
            SolverError::Transport(format!(
                "unexpected solver reply ({error}): {}",
                text.chars().take(200).collect::<String>()
            ))
        })
    }
}

/// Cookies won per host, mirrored to disk so a restart does not pay again.
///
/// Every wall the browser clears leaves a clearance cookie the site honours for
/// hours. Keeping those cookies turns the browser into a one-off cost per host:
/// later searches fetch the engine with a plain HTTP client and answer inside
/// the engine budget, which a browser round trip never can.
struct Jar {
    /// Host to the cookies the browser won there.
    hosts: BTreeMap<String, Vec<Cookie>>,
    /// File the jar is mirrored to; without one it lives in memory only.
    path: Option<PathBuf>,
    /// Hosts with a warm-up running right now, so a burst warms each once.
    warming: BTreeSet<String>,
    /// When each host was last warmed, so a wall that stays shut is not retried
    /// on every single search.
    warmed_at: BTreeMap<String, Instant>,
    /// Set once after a write fails, to warn a single time.
    warned: bool,
}

/// How long a host is left alone after a warm-up, successful or not.
const WARM_BACKOFF: Duration = Duration::from_secs(30 * 60);

/// One cookie as it is written to the jar file.
#[derive(Serialize, Deserialize)]
struct StoredCookie {
    host: String,
    name: String,
    value: String,
    #[serde(default)]
    expires: f64,
}

impl Jar {
    fn load(path: Option<&str>) -> Self {
        let mut jar = Self {
            hosts: BTreeMap::new(),
            path: path.map(PathBuf::from),
            warming: BTreeSet::new(),
            warmed_at: BTreeMap::new(),
            warned: false,
        };
        let Some(file) = jar.path.clone() else {
            return jar;
        };
        let Ok(raw) = std::fs::read_to_string(&file) else {
            return jar;
        };
        match serde_json::from_str::<Vec<StoredCookie>>(&raw) {
            Ok(stored) => {
                for entry in stored {
                    jar.hosts.entry(entry.host).or_default().push(Cookie {
                        name: entry.name,
                        value: entry.value,
                        domain: String::new(),
                        path: String::new(),
                        expires: entry.expires,
                    });
                }
                debug!(file = %file.display(), hosts = jar.hosts.len(), "solver: loaded the cookie jar");
            }
            Err(error) => {
                warn!(file = %file.display(), %error, "solver: ignoring an unreadable cookie jar");
            }
        }
        jar
    }

    fn cookies_for(&self, host: &str) -> Vec<(String, String)> {
        let now = unix_seconds();
        let mut out: Vec<(String, String)> = Vec::new();
        for (known, cookies) in &self.hosts {
            if !same_site(host, known) {
                continue;
            }
            for cookie in cookies.iter().filter(|cookie| !cookie.expired(now)) {
                if out.iter().any(|(name, _)| name == &cookie.name) {
                    continue;
                }
                out.push((cookie.name.clone(), cookie.value.clone()));
            }
        }
        out
    }

    fn remember(&mut self, host: &str, cookies: &[Cookie]) {
        if cookies.is_empty() || host.is_empty() {
            return;
        }
        let entry = self.hosts.entry(host.to_string()).or_default();
        for cookie in cookies.iter().filter(|cookie| !cookie.name.is_empty()) {
            match entry.iter_mut().find(|known| known.name == cookie.name) {
                Some(known) => known.value = cookie.value.clone(),
                None => entry.push(cookie.clone()),
            }
        }
        self.persist();
    }

    fn forget(&mut self, host: &str) {
        let before = self.hosts.len();
        self.hosts.retain(|known, _| !same_site(host, known));
        // A cookie the site just refused must not silence the background warm
        // for half an hour: dropping the backoff is what lets the next search
        // win a fresh one. The in-flight marker stays, so a page load that is
        // already running is still not duplicated.
        self.warmed_at.retain(|known, _| !same_site(host, known));
        if self.hosts.len() != before {
            debug!(host = %host, "solver: dropped stale cookies");
            self.persist();
        }
    }

    fn should_warm(&mut self, host: &str) -> bool {
        if host.is_empty() || self.warming.contains(host) {
            return false;
        }
        if let Some(last) = self.warmed_at.get(host) {
            if last.elapsed() < WARM_BACKOFF {
                return false;
            }
        }
        // Cookies already in hand mean the wall stands open: nothing to warm.
        if !self.cookies_for(host).is_empty() {
            return false;
        }
        self.warming.insert(host.to_string());
        true
    }

    fn warmed(&mut self, host: &str) {
        self.warming.remove(host);
        self.warmed_at.insert(host.to_string(), Instant::now());
    }

    /// Mirror the jar to disk, atomically, ignoring failures after one warning.
    fn persist(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let now = unix_seconds();
        let stored: Vec<StoredCookie> = self
            .hosts
            .iter()
            .flat_map(|(host, cookies)| {
                let host = host.clone();
                cookies
                    .iter()
                    .filter(move |cookie| !cookie.expired(now))
                    .map(move |cookie| StoredCookie {
                        host: host.clone(),
                        name: cookie.name.clone(),
                        value: cookie.value.clone(),
                        expires: cookie.expires,
                    })
            })
            .collect();
        let Ok(body) = serde_json::to_vec(&stored) else {
            return;
        };
        let temp = path.with_extension("tmp");
        let write = std::fs::write(&temp, &body).and_then(|()| std::fs::rename(&temp, &path));
        if let Err(error) = write {
            if !self.warned {
                self.warned = true;
                warn!(file = %path.display(), %error, "solver: cannot write the cookie jar, keeping it in memory");
            }
        }
    }
}

/// Whether cookies won for one host may be replayed to another.
///
/// The solver records the host it fetched rather than the cookie's own domain,
/// so a parent and a child name the same site and both are valid carriers.
fn same_site(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

/// The host of an absolute URL, without scheme, port, user or path.
fn url_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, rest)| rest);
    host.split(':').next().unwrap_or("").to_string()
}

/// Seconds since the epoch, the unit cookies use for their expiry.
fn unix_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// A page whose whole body is one word of refusal.
///
/// Sites guarded per browser profile answer a used profile with a bare
/// "Forbidden" instead of a challenge, which no amount of waiting clears.
fn stale_block_page(solved: &Solved) -> bool {
    if solved.body.len() > 512 {
        return false;
    }
    [
        "<h1>Forbidden",
        ">Forbidden<",
        ">Access Denied<",
        ">Too Many Requests<",
    ]
    .iter()
    .any(|marker| solved.body.contains(marker))
}

/// A page that refuses the browser itself rather than asking it to solve
/// something. CloudFront answers like this when the address is blocked, so a
/// fresh profile or a longer dwell cannot change the outcome.
fn refused_page(solved: &Solved) -> bool {
    if solved.body.len() > 4096 {
        return false;
    }
    [
        "ERROR: The request could not be satisfied",
        "Request blocked. We can't connect",
    ]
    .iter()
    .any(|marker| solved.body.contains(marker))
}

fn mentions_session(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("session") || lowered.contains("tab crashed") || lowered.contains("invalid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cloudfront_refusal_is_reported_as_a_block() {
        let page = Solved {
            status: 200,
            url: "https://www.deviantart.com/search".into(),
            cookies: Vec::new(),
            body: "<html><head><title>ERROR: The request could not be satisfied</title></head><body><h1>403 ERROR</h1> Request blocked. We can't connect to the server for this app or website at this time.</body></html>".into(),
        };
        assert!(refused_page(&page));
        let short = Solved {
            body: "Request blocked. We can't connect to the server".into(),
            ..page.clone()
        };
        assert!(refused_page(&short));
        let real = Solved {
            status: 200,
            url: "https://example.org/".into(),
            cookies: Vec::new(),
            body: "<html><body><h1>Search results</h1><p>blocked by the firewall is a song</p></body></html>".into(),
        };
        assert!(!refused_page(&real));
    }

    #[test]
    fn cloudflare_interstitial_is_a_challenge() {
        let body = br#"<title>Just a moment...</title><div id="cf-chl-widget"></div>"#;
        assert!(looks_like_challenge(403, body));
        assert!(looks_like_challenge(200, body));
    }

    #[test]
    fn a_real_page_that_loads_the_bot_management_script_is_not_a_challenge() {
        // Vimeo's result page loads
        // /cdn-cgi/challenge-platform/scripts/precursor/main.js as a normal
        // asset, so the path alone cannot mean "interstitial".
        let mut body = String::from("<html><body><script src=\"/cdn-cgi/challenge-platform/scripts/precursor/main.js\"></script>");
        body.push_str(&"<a href=\"/x\">result</a>".repeat(4_000));
        body.push_str("</body></html>");
        assert!(body.len() > INTERSTITIAL_MAX_BYTES);
        assert!(!looks_like_challenge(200, body.as_bytes()));
        // The interstitial itself is small and still counts.
        let wall = b"<html><head><title>Just a moment...</title></head><body><script src=\"/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1\"></script></body></html>";
        assert!(looks_like_challenge(403, wall));
    }

    #[test]
    fn a_captcha_on_an_error_status_is_a_challenge() {
        let body = b"<p>Please solve the captcha to continue</p>";
        assert!(looks_like_challenge(403, body));
        assert!(looks_like_challenge(429, body));
    }

    #[test]
    fn real_results_are_not_a_challenge() {
        let body = b"<html><body><h3>captcha solver library</h3><a href=\"/x\">x</a></body></html>";
        assert!(!looks_like_challenge(200, body));
        assert!(!looks_like_challenge(404, b"captcha"));
    }

    #[test]
    fn azure_front_door_interstitial_is_a_challenge() {
        // privacywall.org sits behind Azure Front Door. Its interstitial is
        // served with 200, so the status alone never gives it away, and the
        // answer only arrives once the challenge script has run.
        let body = b"<!DOCTYPE html><html><head><title>Azure WAF</title></head>\
                     <body><p>Azure WAF JS Challenge</p></body></html>";
        assert!(looks_like_challenge(200, body));
        assert!(looks_like_challenge(403, body));
    }

    #[test]
    fn a_bare_forbidden_page_asks_for_a_fresh_profile() {
        let blocked = Solved {
            status: 200,
            url: "https://example.test/api".into(),
            cookies: Vec::new(),
            body: "<html><head><meta name=\"color-scheme\" content=\"light dark\"></head><body><pre style=\"word-wrap: break-word; white-space: pre-wrap;\">Forbidden</pre></body></html>".into(),
        };
        assert!(stale_block_page(&blocked));
    }

    #[test]
    fn real_data_is_never_mistaken_for_a_bare_refusal() {
        let json = Solved {
            status: 200,
            url: "https://example.test/api".into(),
            cookies: Vec::new(),
            body: r#"<pre>{"pagination":{"total":81325}}</pre>"#.into(),
        };
        assert!(!stale_block_page(&json));
        let long = Solved {
            status: 200,
            url: "https://example.test/".into(),
            cookies: Vec::new(),
            body: format!("<html><body>>Forbidden<{}</body></html>", "x".repeat(600)),
        };
        assert!(!stale_block_page(&long));
    }

    #[test]
    fn the_jar_keeps_a_won_cookie_per_host_and_drops_it_when_asked() {
        let mut jar = Jar {
            hosts: BTreeMap::new(),
            path: None,
            warming: BTreeSet::new(),
            warmed_at: BTreeMap::new(),
            warned: false,
        };
        jar.remember(
            "example.test",
            &[Cookie {
                name: "cf_clearance".into(),
                value: "abc".into(),
                domain: String::new(),
                path: String::new(),
                expires: 0.0,
            }],
        );
        assert_eq!(
            jar.cookies_for("example.test"),
            vec![("cf_clearance".to_string(), "abc".to_string())]
        );
        // A fresh host is worth one background browser, and only one: the
        // second search while the tab is loading must not queue another.
        assert!(jar.should_warm("other.test"));
        assert!(!jar.should_warm("other.test"));
        jar.warmed("other.test");
        jar.warmed("example.test");
        assert!(
            !jar.should_warm("example.test"),
            "a host warmed a moment ago is left alone"
        );
        // The wall came back, so the cookie is worthless: forgetting it must
        // also drop the backoff, otherwise a dead cookie keeps the site broken
        // for half an hour.
        jar.forget("example.test");
        assert!(jar.cookies_for("example.test").is_empty());
        assert!(
            jar.should_warm("example.test"),
            "the backoff dies with the cookie"
        );
    }

    #[test]
    fn an_expired_cookie_never_leaves_the_jar() {
        let mut jar = Jar {
            hosts: BTreeMap::new(),
            path: None,
            warming: BTreeSet::new(),
            warmed_at: BTreeMap::new(),
            warned: false,
        };
        jar.remember(
            "example.test",
            &[
                Cookie {
                    name: "fresh".into(),
                    value: "1".into(),
                    domain: String::new(),
                    path: String::new(),
                    expires: 0.0,
                },
                Cookie {
                    name: "dead".into(),
                    value: "2".into(),
                    domain: String::new(),
                    path: String::new(),
                    expires: 1.0,
                },
            ],
        );
        assert_eq!(
            jar.cookies_for("example.test"),
            vec![("fresh".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn a_solved_api_response_is_unwrapped_from_the_browser_page() {
        let page = String::from(
            "<html><head><meta charset=\"utf-8\"></head><body><pre>{\"query\":{\"search\":[]}}\n</pre></body></html>",
        );
        assert_eq!(unwrap_json_document(page), "{\"query\":{\"search\":[]}}\n");
    }

    #[test]
    fn a_page_with_a_code_block_keeps_its_markup() {
        let page =
            String::from("<html><body><pre>fn main() {}</pre><a href=\"/x\">x</a></body></html>");
        assert_eq!(unwrap_json_document(page.clone()), page);
    }

    #[test]
    fn the_pool_gets_one_name_per_tab() {
        let settings = SolverSettings {
            enabled: true,
            session: "rustxng".into(),
            pool: 3,
            ..SolverSettings::default()
        };
        let solver = Solver::new(&settings).expect("solver");
        let names: Vec<&str> = solver.slots.iter().map(|slot| slot.name.as_str()).collect();
        assert_eq!(names, vec!["rustxng", "rustxng-2", "rustxng-3"]);
    }

    #[tokio::test]
    async fn a_busy_pool_waits_but_gives_up_within_the_budget() {
        let settings = SolverSettings {
            enabled: true,
            pool: 1,
            ..SolverSettings::default()
        };
        let solver = Solver::new(&settings).expect("solver");
        let _busy = solver.slots[0].gate.try_lock().expect("free gate");
        let started = std::time::Instant::now();
        let result = solver
            .solve_within("http://example.invalid/", Duration::from_millis(120), false, None)
            .await;
        assert!(matches!(result, Err(SolverError::Busy)));
        assert!(started.elapsed() >= Duration::from_millis(100));
    }
}
