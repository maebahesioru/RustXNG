//! Engine framework, ported from `searx/engines/__init__.py`.
//!
//! An engine turns a parsed Query into an HTTP EngineRequest, and turns the
//! EngineResponse back into a list of EngineItems. Like upstream, both
//! functions are synchronous; the network layer is asynchronous.

pub mod state;
pub mod util;
pub mod xml;
pub mod xpath;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use scraper::Html;
use serde::{Deserialize, Serialize};

use crate::query::Query;
use crate::results::{Infobox, SearchResult};
use crate::settings::{EngineConfig, Settings};

/// Engine trait object as stored by the registry.
pub type EngineHandle = Arc<dyn Engine>;

/// HTTP methods used by engines (upstream also allows PUT/DELETE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    #[default]
    Get,
    Post,
}

/// A request built by an engine.
#[derive(Debug, Clone)]
pub struct EngineRequest {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    /// `application/x-www-form-urlencoded` body.
    pub form: Vec<(String, String)>,
    /// `application/json` body.
    pub json: Option<serde_json::Value>,
    pub cookies: Vec<(String, String)>,
}

impl EngineRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            headers: HeaderMap::new(),
            form: Vec::new(),
            json: None,
            cookies: Vec::new(),
        }
    }

    pub fn post(url: impl Into<String>) -> Self {
        let mut request = Self::get(url);
        request.method = Method::Post;
        request
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.set_header(name, value);
        self
    }

    pub fn set_header(&mut self, name: &str, value: &str) {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            self.headers.insert(name, value);
        }
    }

    pub fn form(mut self, values: Vec<(String, String)>) -> Self {
        self.form = values;
        self
    }

    pub fn json_body(mut self, value: serde_json::Value) -> Self {
        self.json = Some(value);
        self
    }

    pub fn cookie(mut self, name: &str, value: &str) -> Self {
        self.cookies.push((name.to_string(), value.to_string()));
        self
    }
}

/// A response as far as engines are concerned.
#[derive(Debug, Clone)]
pub struct EngineResponse {
    pub status: u16,
    pub final_url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl EngineResponse {
    pub fn new(status: u16, final_url: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            status,
            final_url: final_url.into(),
            headers: BTreeMap::new(),
            body,
        }
    }

    /// Whether this is a page a browser can render rather than a machine
    /// payload. The browser solver only helps with the former: handing it an
    /// API endpoint that answered JSON just burns a tab.
    pub fn serves_html(&self) -> bool {
        if let Some(value) = self
            .headers
            .get("content-type")
            .or_else(|| self.headers.get("Content-Type"))
        {
            let lowered = value.to_ascii_lowercase();
            if lowered.contains("html") {
                return true;
            }
            if !lowered.is_empty() {
                return false;
            }
        }
        let head = self.body.iter().take(64).copied().collect::<Vec<u8>>();
        let head = String::from_utf8_lossy(&head);
        let head = head.trim_start();
        head.starts_with('<') && !head.starts_with("<?xml")
    }

    /// The body as UTF-8 (lossy, like upstream's response text).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    pub fn json(&self) -> Result<serde_json::Value, EngineError> {
        serde_json::from_slice(&self.body)
            .map_err(|error| EngineError::Parse(format!("invalid JSON response: {error}")))
    }

    pub fn html(&self) -> Result<Html, EngineError> {
        Ok(Html::parse_document(&self.text()))
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_lowercase()).map(String::as_str)
    }
}

/// What an engine can return (upstream engine results).
#[derive(Debug, Clone)]
pub enum EngineItem {
    Result(SearchResult),
    Suggestion(String),
    Answer(String),
    Correction(String),
    Infobox(Infobox),
    EngineData { key: String, value: String },
}

impl EngineItem {
    pub fn result(result: SearchResult) -> Self {
        Self::Result(result)
    }
}

/// Errors an engine can raise, mirroring upstream's exception hierarchy.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EngineError {
    /// Upstream access denied exception.
    #[error("access denied")]
    AccessDenied,
    /// Upstream captcha exception.
    #[error("captcha required")]
    Captcha,
    /// Upstream too many requests exception.
    #[error("too many requests")]
    TooManyRequests,
    #[error("engine request failed: {0}")]
    Message(String),
    #[error("engine response could not be parsed: {0}")]
    Parse(String),
}

/// The failure categories the framework tracks per engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Timeout,
    AccessDenied,
    Captcha,
    TooManyRequests,
    /// The browser solver had no free tab for this engine. That is a capacity
    /// limit, not a wall: the engine must not be suspended for it, or the next
    /// searches lose it for nothing.
    SolverBusy,
    /// The search ran out of time before a tab was ever taken. This is the
    /// normal outcome of a whole search with a three second deadline, where a
    /// browser round trip cannot fit: the engine is fine, only unaffordable.
    SolverSkipped,
    Http,
    Parse,
    /// The response parsed, but looks fabricated or unrelated (see \`quality\`).
    Suspicious,
    Other,
}

impl FailureKind {
    /// Upstream error type strings, they show up in the UI.
    pub fn error_type(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::AccessDenied => "access denied",
            Self::Captcha => "captcha",
            Self::TooManyRequests => "too many requests",
            Self::SolverBusy => "solver busy",
            Self::SolverSkipped => "solver skipped",
            Self::Http => "http error",
            Self::Parse => "parsing error",
            Self::Suspicious => "suspicious response",
            Self::Other => "unexpected error",
        }
    }

    /// Suspension key used in the settings.
    pub fn suspension_key(self) -> Option<&'static str> {
        match self {
            Self::AccessDenied => Some("SearxEngineAccessDenied"),
            Self::Captcha => Some("SearxEngineCaptcha"),
            Self::TooManyRequests => Some("SearxEngineTooManyRequests"),
            _ => None,
        }
    }

    /// Classify a status code the way upstream does.
    pub fn from_status(status: u16) -> Option<Self> {
        match status {
            401..=403 => Some(Self::AccessDenied),
            429 => Some(Self::TooManyRequests),
            500..=599 => Some(Self::Http),
            _ => None,
        }
    }
}

/// The label the UI and the JSON API show for an upstream error type string.
///
/// One table for every surface: the JSON feeds used to keep a second copy that
/// quietly fell through to "Unexpected error" for anything new, which made a
/// capacity problem look like a broken engine.
pub fn error_type_label(error_type: &str) -> &'static str {
    match error_type {
        "timeout" => "Timeout",
        "access denied" => "Access denied",
        "captcha" => "CAPTCHA",
        "too many requests" => "Too many requests",
        "solver busy" => "Solver busy",
        "solver skipped" => "Solver skipped",
        "http error" => "HTTP error",
        "parsing error" => "Parsing error",
        "suspended" => "Suspended",
        "suspicious response" => "Suspicious response",
        _ => "Unexpected error",
    }
}

impl From<&EngineError> for FailureKind {
    fn from(error: &EngineError) -> Self {
        match error {
            EngineError::AccessDenied => Self::AccessDenied,
            EngineError::Captcha => Self::Captcha,
            EngineError::TooManyRequests => Self::TooManyRequests,
            EngineError::Parse(_) => Self::Parse,
            EngineError::Message(_) => Self::Other,
        }
    }
}

/// Defaults an engine declares for its configuration.
#[derive(Debug, Clone)]
pub struct EngineDefaults {
    pub categories: &'static [&'static str],
    pub shortcut: &'static str,
    pub paging: bool,
    pub time_range_support: bool,
    pub safesearch: bool,
    pub language_support: bool,
    pub weight: f64,
    pub timeout: f64,
}

impl Default for EngineDefaults {
    fn default() -> Self {
        Self {
            categories: &["general"],
            shortcut: "",
            paging: false,
            time_range_support: false,
            safesearch: false,
            language_support: false,
            weight: 1.0,
            timeout: 4.0,
        }
    }
}

/// Common engine metadata, resolved from the settings entry.
#[derive(Debug, Clone)]
pub struct EngineMeta {
    pub name: String,
    pub engine_id: String,
    pub shortcut: String,
    pub categories: Vec<String>,
    pub paging: bool,
    pub time_range_support: bool,
    pub safesearch: bool,
    pub language_support: bool,
    pub weight: f64,
    pub timeout: Duration,
    pub base_url: Option<String>,
    /// Per-engine proxy URL, when the settings ask for one.
    pub proxy: Option<String>,
    pub search_url: Option<String>,
    pub api_key: Option<String>,
    pub extra: toml::Table,
}

impl EngineMeta {
    pub fn from_config(config: &EngineConfig, defaults: EngineDefaults) -> Self {
        let categories = if config.categories.is_empty() {
            defaults
                .categories
                .iter()
                .map(|category| (*category).to_string())
                .collect()
        } else {
            config.categories.clone()
        };
        let shortcut = config
            .shortcut
            .clone()
            .unwrap_or_else(|| defaults.shortcut.to_string());
        Self {
            name: config.name.clone(),
            engine_id: config.engine.clone(),
            shortcut,
            categories,
            paging: config.paging.unwrap_or(defaults.paging),
            time_range_support: config
                .time_range_support
                .unwrap_or(defaults.time_range_support),
            safesearch: config.safesearch.unwrap_or(defaults.safesearch),
            language_support: config.language_support.unwrap_or(defaults.language_support),
            weight: config.weight.unwrap_or(defaults.weight),
            timeout: Duration::from_secs_f64(config.timeout.unwrap_or(defaults.timeout).max(0.1)),
            base_url: config
                .base_url
                .clone()
                .or_else(|| config.opt_str("base_url").map(str::to_string)),
            proxy: config.proxy.clone(),
            search_url: config.search_url.clone(),
            api_key: config.api_key.clone(),
            extra: config.extra.clone(),
        }
    }

    pub fn opt_str(&self, key: &str) -> Option<&str> {
        self.extra.get(key).and_then(|value| value.as_str())
    }

    pub fn opt_bool(&self, key: &str) -> Option<bool> {
        self.extra.get(key).and_then(|value| value.as_bool())
    }

    pub fn opt_i64(&self, key: &str) -> Option<i64> {
        self.extra.get(key).and_then(|value| value.as_integer())
    }

    pub fn opt_f64(&self, key: &str) -> Option<f64> {
        self.extra
            .get(key)
            .and_then(|value| value.as_float())
            .or_else(|| {
                self.extra
                    .get(key)
                    .and_then(|value| value.as_integer())
                    .map(|value| value as f64)
            })
    }

    pub fn opt_str_list(&self, key: &str) -> Vec<String> {
        self.extra
            .get(key)
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The engine interface, upstream's request() / response() pair.
///
/// Engines only have to provide their metadata and the two halves of the
/// upstream `request()` / `response()` pair; every other accessor delegates to
/// the metadata.
pub trait Engine: Send + Sync {
    /// Configuration resolved for this engine.
    fn meta(&self) -> &EngineMeta;

    /// Display name (settings.engines[].name).
    fn name(&self) -> &str {
        &self.meta().name
    }
    /// Implementation id (settings.engines[].engine).
    fn engine_id(&self) -> &str {
        &self.meta().engine_id
    }
    /// Bang shortcut.
    fn shortcut(&self) -> &str {
        &self.meta().shortcut
    }
    /// Categories this engine answers for.
    fn categories(&self) -> &[String] {
        &self.meta().categories
    }
    /// Whether the engine supports pageno.
    fn paging(&self) -> bool {
        self.meta().paging
    }
    /// Whether the engine understands time_range.
    fn time_range_support(&self) -> bool {
        self.meta().time_range_support
    }
    /// Whether the engine understands safesearch.
    fn safesearch_support(&self) -> bool {
        self.meta().safesearch
    }
    /// Whether the engine accepts a language.
    fn language_support(&self) -> bool {
        self.meta().language_support
    }
    /// Ranking weight (upstream default 1.0).
    fn weight(&self) -> f64 {
        self.meta().weight
    }
    /// Per engine timeout.
    fn timeout(&self) -> Duration {
        self.meta().timeout
    }
    /// Per engine proxy URL, when the settings give this engine one.
    fn proxy(&self) -> Option<&str> {
        self.meta().proxy.as_deref()
    }
    /// Build the HTTP request for the query.
    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError>;
    /// Parse the response into results.
    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError>;

    /// A second request that has to be made before parsing.
    ///
    /// Some sites answer the search page with a shell that loads the actual
    /// list from a signed fragment URL, so the first response holds no
    /// results. Such an engine returns that URL here; the aggregator fetches it
    /// with whatever is left of the engine timeout and parses *that* response.
    /// The default performs a single request, like every ordinary engine.
    fn followup_request(
        &self,
        _response: &EngineResponse,
        _query: &Query,
    ) -> Result<Option<EngineRequest>, EngineError> {
        Ok(None)
    }

    /// Whether this engine can answer the query at all.
    ///
    /// The default keeps ordinary engines unchanged. Syntax-gated engines can
    /// override this so unrelated general searches skip them before request
    /// construction and failure accounting.
    fn is_applicable(&self, _query: &Query) -> bool {
        true
    }

    /// Whether this engine queries a local database instead of HTTP
    /// (upstream `engine_type = "offline"`). Offline engines run
    /// [`Engine::search_offline`] on a blocking thread.
    fn is_offline(&self) -> bool {
        false
    }

    /// Run the database query. Only called for engines whose
    /// [`Engine::is_offline`] returns true.
    fn search_offline(&self, _query: &Query, _pageno: u32) -> Result<Vec<EngineItem>, EngineError> {
        Err(EngineError::Message(
            "offline engine without a query implementation".into(),
        ))
    }
}

impl fmt::Debug for dyn Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Engine")
            .field("name", &self.name())
            .field("engine_id", &self.engine_id())
            .finish()
    }
}

/// All engines of an instance, indexed like upstream's engine registry.
#[derive(Default)]
pub struct EngineRegistry {
    handles: Vec<EngineHandle>,
    by_name: HashMap<String, usize>,
    by_shortcut: HashMap<String, usize>,
    /// Engines that could not be initialised (missing api key, unreachable
    /// instance, ...). They are skipped, like upstream does, and reported by
    /// the stats endpoints.
    failed: Vec<(String, String)>,
}

impl EngineRegistry {
    /// Build the registry from the settings, calling the engine factory.
    pub fn from_settings(settings: &Settings) -> Result<Self, String> {
        let mut registry = Self::default();
        for config in settings.enabled_engines() {
            let handle = match crate::engines::build(config) {
                Ok(handle) => handle,
                Err(error) => {
                    tracing::warn!(engine = %config.name, %error, "engine cannot be initialised, skipped");
                    registry.failed.push((config.name.clone(), error));
                    continue;
                }
            };
            let index = registry.handles.len();
            registry.by_name.insert(handle.name().to_lowercase(), index);
            if !handle.engine_id().is_empty() {
                registry
                    .by_name
                    .entry(handle.engine_id().to_lowercase())
                    .or_insert(index);
            }
            if !handle.shortcut().is_empty() {
                registry
                    .by_shortcut
                    .insert(handle.shortcut().to_lowercase(), index);
            }
            registry.handles.push(handle);
        }
        Ok(registry)
    }

    /// Register an already built engine (used by tests and by embedders).
    pub fn push(&mut self, handle: EngineHandle) {
        let index = self.handles.len();
        self.by_name.insert(handle.name().to_lowercase(), index);
        if !handle.engine_id().is_empty() {
            self.by_name
                .entry(handle.engine_id().to_lowercase())
                .or_insert(index);
        }
        if !handle.shortcut().is_empty() {
            self.by_shortcut
                .insert(handle.shortcut().to_lowercase(), index);
        }
        self.handles.push(handle);
    }

    /// Look an engine up by name (case insensitive) or engine id.
    pub fn get(&self, name: &str) -> Option<&EngineHandle> {
        self.by_name
            .get(&name.trim().to_lowercase())
            .map(|index| &self.handles[*index])
    }

    /// Look an engine up by shortcut.
    pub fn by_shortcut(&self, shortcut: &str) -> Option<&EngineHandle> {
        self.by_shortcut
            .get(&shortcut.trim().to_lowercase())
            .map(|index| &self.handles[*index])
    }

    pub fn iter(&self) -> impl Iterator<Item = &EngineHandle> {
        self.handles.iter()
    }

    /// Engines that could not be initialised, as (name, reason) pairs.
    pub fn failed(&self) -> &[(String, String)] {
        &self.failed
    }

    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// All category names that have at least one engine.
    pub fn categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = self
            .handles
            .iter()
            .flat_map(|engine| engine.categories().iter().cloned())
            .collect();
        categories.sort();
        categories.dedup();
        categories
    }

    pub fn has_category(&self, category: &str) -> bool {
        let category = crate::categories::normalize(category);
        self.handles.iter().any(|engine| {
            engine
                .categories()
                .iter()
                .any(|candidate| crate::categories::normalize(candidate) == category)
        })
    }

    /// Names of the engines answering for a category.
    pub fn engines_in_category(&self, category: &str) -> Vec<String> {
        let category = crate::categories::normalize(category);
        self.handles
            .iter()
            .filter(|engine| {
                engine
                    .categories()
                    .iter()
                    .any(|candidate| crate::categories::normalize(candidate) == category)
            })
            .map(|engine| engine.name().to_string())
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.handles
            .iter()
            .map(|engine| engine.name().to_string())
            .collect()
    }

    pub fn shortcuts(&self) -> Vec<String> {
        self.handles
            .iter()
            .map(|engine| engine.shortcut().to_string())
            .filter(|shortcut| !shortcut.is_empty())
            .collect()
    }

    /// Ranking weight of an engine, 1.0 when unknown.
    pub fn weight_of(&self, name: &str) -> f64 {
        self.get(name).map(|engine| engine.weight()).unwrap_or(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn every_failure_kind_has_a_label_of_its_own() {
        // A label that falls back to the generic text hides what happened: a
        // tab shortage read as a broken engine for days.
        for kind in [
            FailureKind::Timeout,
            FailureKind::AccessDenied,
            FailureKind::Captcha,
            FailureKind::TooManyRequests,
            FailureKind::SolverBusy,
            FailureKind::SolverSkipped,
            FailureKind::Http,
            FailureKind::Parse,
            FailureKind::Suspicious,
        ] {
            let label = error_type_label(kind.error_type());
            assert_ne!(label, "Unexpected error", "{kind:?} has no label");
        }
        assert_eq!(error_type_label("solver busy"), "Solver busy");
        assert_eq!(error_type_label("solver skipped"), "Solver skipped");
        assert_eq!(error_type_label("unexpected error"), "Unexpected error");
    }

    #[test]
    fn registry_contains_the_default_engines() {
        let registry = EngineRegistry::from_settings(&Settings::defaults()).unwrap();
        assert!(registry.get("wikipedia").is_some());
        assert!(registry.get("Wikipedia").is_some());
        assert!(registry.by_shortcut("wp").is_some());
        assert!(registry.has_category("images"));
        assert!(!registry.engines_in_category("images").is_empty());
    }

    #[test]
    fn unknown_engine_config_is_an_error() {
        let mut settings = Settings::defaults();
        settings.engines.push(EngineConfig::new(
            "broken",
            "does_not_exist",
            "brk",
            &["general"],
        ));
        // A broken engine is skipped (like upstream) and reported, the rest
        // of the registry still builds.
        let registry = EngineRegistry::from_settings(&settings).expect("registry");
        assert!(registry.get("broken").is_none());
        assert!(registry.failed().iter().any(|(name, _)| name == "broken"));
    }
}
