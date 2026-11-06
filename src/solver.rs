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

use std::time::Duration;

use serde::Deserialize;
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
}

/// Body markers that mean "this is a challenge, not the page".
///
/// A 200 only counts for markers that never appear on a real result page; WAF
/// statuses may also be a plain CAPTCHA interstitial.
const STRONG_MARKERS: &[&str] = &[
    "just a moment",
    "cf-chl",
    "cf_chl_opt",
    "challenge-platform",
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
        })
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
                    let result = self.solve_in(slot, url).await;
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

    async fn solve_in(&self, slot: &Slot, url: &str) -> Result<Solved, SolverError> {
        let mut fresh_profile_used = false;
        let mut dwelt = false;
        loop {
            let session = self.session(slot).await?;
            let dwell = dwelt.then_some(SLOW_CHALLENGE_SECONDS);
            match self.get(&session, url, dwell).await {
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
        Ok(Solved {
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
        })
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
            body: "<html><head><meta name=\"color-scheme\" content=\"light dark\"></head><body><pre style=\"word-wrap: break-word; white-space: pre-wrap;\">Forbidden</pre></body></html>".into(),
        };
        assert!(stale_block_page(&blocked));
    }

    #[test]
    fn real_data_is_never_mistaken_for_a_bare_refusal() {
        let json = Solved {
            status: 200,
            url: "https://example.test/api".into(),
            body: r#"<pre>{"pagination":{"total":81325}}</pre>"#.into(),
        };
        assert!(!stale_block_page(&json));
        let long = Solved {
            status: 200,
            url: "https://example.test/".into(),
            body: format!("<html><body>>Forbidden<{}</body></html>", "x".repeat(600)),
        };
        assert!(!stale_block_page(&long));
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
            .solve_within("http://example.invalid/", Duration::from_millis(120), false)
            .await;
        assert!(matches!(result, Err(SolverError::Busy)));
        assert!(started.elapsed() >= Duration::from_millis(100));
    }
}
