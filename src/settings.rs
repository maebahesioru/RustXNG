//! Settings handling, ported from `searx/settings_defaults.py`,
//! `searx/settings_loader.py` and `searx/settings.yml`.
//!
//! Differences to upstream are deliberate and documented:
//!
//! * the built-in defaults ship as a TOML file (`settings.toml`); YAML engines
//!   such as the generic `xpath` engine are not ported,
//! * `use_default_settings` can be `true` (merge user engines into the
//!   defaults, the upstream default) or `false` (replace the engine list),
//! * unknown engine specific keys are kept in `EngineConfig::extra` so that a
//!   config written for SearXNG keeps loading.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::locales;

/// The built-in defaults, embedded at compile time.
pub const DEFAULT_SETTINGS: &str = include_str!("../settings.toml");

/// Formats the `/search` endpoint can render (upstream `OUTPUT_FORMATS`).
pub static OUTPUT_FORMATS: &[&str] = &["html", "csv", "json", "rss"];

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("could not read settings file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid settings file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid settings: {0}")]
    Validation(String),
}

/// Top level settings object.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// When true the engines of the defaults and of the user file are merged.
    pub use_default_settings: Option<bool>,
    pub general: General,
    pub server: Server,
    pub search: Search,
    pub ui: Ui,
    pub outgoing: Outgoing,
    pub engines: Vec<EngineConfig>,
    pub plugins: Plugins,
    /// Response cache backend (upstream `cache` and `valkey`).
    #[serde(default)]
    pub cache: CacheSettings,
    /// Deep research: plan queries, search, fetch, synthesise a cited report.
    #[serde(default)]
    pub research: ResearchSettings,
    /// Optional external browser solver for WAF / CAPTCHA guarded engines.
    #[serde(default)]
    pub solver: SolverSettings,
    /// Categories shown as tabs (upstream `categories_as_tabs`).
    pub categories_as_tabs: Vec<String>,
    /// Accepted for compatibility with SearXNG configs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi_resolvers: Option<toml::Table>,
}

/// Deep research configuration. `enabled` requires an OpenAI-compatible
/// `base_url` and `api_key`; without them the /research endpoint still returns
/// a thorough evidence bundle (no synthesised report).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ResearchSettings {
    pub enabled: bool,
    /// OpenAI-compatible base URL, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Sub-queries generated per question.
    pub max_queries: usize,
    /// Sources kept after deduplication.
    pub max_sources: usize,
    /// LLM request timeout in seconds.
    pub timeout: u64,
}

impl Default for ResearchSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            max_queries: 5,
            max_sources: 10,
            timeout: 60,
        }
    }
}

/// External challenge solver. Disabled by default: the engines are expected to
/// work with plain HTTP, and driving a browser is slow compared to them.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SolverSettings {
    pub enabled: bool,
    /// FlareSolverr-compatible JSON endpoint.
    pub endpoint: String,
    /// Browser session name, reused across searches.
    pub session: String,
    /// How many browser tabs may be driven at once.
    ///
    /// A search that hits three blocked engines at the same time would
    /// otherwise serialise behind one page load and lose the engines to the
    /// "busy" shortcut.
    pub pool: usize,
    /// How long one search may wait for the browser before falling back.
    pub budget_ms: u64,
    /// Ceiling for a single page load inside the browser.
    pub max_timeout_ms: u64,
    /// Ceiling for an engine suspension once the solver is enabled.
    ///
    /// A 24-hour ban is the right answer when plain HTTP hit a wall, because
    /// nothing changes on the next search. With a browser in the pool it is
    /// the wrong answer: the challenge the engine met is exactly the one the
    /// solver clears, so an hour-long bench keeps a working engine out of
    /// every answer. Zero keeps the upstream timings untouched.
    pub suspension_cap_secs: u64,
}

impl Default for SolverSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://127.0.0.1:8191/v1".into(),
            session: "rustxng".into(),
            pool: 2,
            budget_ms: 2_500,
            max_timeout_ms: 45_000,
            suspension_cap_secs: 300,
        }
    }
}

impl SolverSettings {
    /// The cap that applies to engine suspensions, if any.
    ///
    /// A switched-off solver changes nothing about how hard the engines are
    /// hit, so the upstream suspension times stay in charge.
    pub fn suspension_cap(&self) -> Option<u64> {
        if !self.enabled || self.suspension_cap_secs == 0 {
            return None;
        }
        Some(self.suspension_cap_secs)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct General {
    pub debug: bool,
    pub instance_name: String,
    pub contact_url: String,
    pub donation_url: String,
    pub privacypolicy_url: String,
    pub enable_metrics: bool,
    pub public_instance: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            debug: false,
            instance_name: "RustXNG".into(),
            contact_url: String::new(),
            donation_url: String::new(),
            privacypolicy_url: String::new(),
            enable_metrics: true,
            public_instance: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Server {
    pub bind_address: String,
    pub port: u16,
    /// Exit after this many seconds without a request (0 keeps running).
    /// Handy for sidecars that a launcher forgets to stop.
    #[serde(default)]
    pub idle_timeout: u64,
    pub base_url: Option<String>,
    pub secret_key: String,
    /// `server.limiter`: the legacy boolean or a `[server.limiter]` table.
    pub limiter: LimiterSettings,
    /// Length of the legacy limiter window in seconds.
    ///
    /// Only used when `server.limiter = true` (the flat form); the table form
    /// uses the upstream `ip_limit` windows.
    pub limiter_window: u64,
    /// Requests allowed per client and window in the legacy flat form.
    pub limiter_max: u32,
    /// Upstream's `server.public_instance`: enables the stricter limiter.
    pub public_instance: bool,
    pub image_proxy: bool,
    /// HTTP method of the search form: `GET` or `POST`.
    pub method: String,
    pub http_protocol_version: Option<String>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1".into(),
            port: 29483,
            idle_timeout: 0,
            base_url: None,
            secret_key: "ultrasecretkey".into(),
            limiter: LimiterSettings::default(),
            limiter_window: 60,
            limiter_max: 30,
            public_instance: false,
            image_proxy: false,
            method: "POST".into(),
            http_protocol_version: None,
        }
    }
}

impl Server {
    /// Whether the limiter is switched on, in either the flat or table form.
    pub fn limiter_enabled(&self) -> bool {
        self.limiter.is_enabled()
    }

    /// The `[server.limiter]` table, if the table form is used.
    pub fn limiter_config(&self) -> Option<&LimiterConfig> {
        self.limiter.config()
    }
}

/// `server.limiter`: a boolean (upstream's `server.limiter`) or a table:
///
/// ```text
/// [server.limiter]
/// enabled = true
/// trusted_proxies = ["127.0.0.0/8"]
///
/// [server.limiter.botdetection]
/// ipv4_prefix = 32
/// ipv6_prefix = 48
///
/// [server.limiter.botdetection.ip_limit]
/// filter_link_local = false
/// link_token = true
///
/// [server.limiter.botdetection.ip_lists]
/// pass_ip = ["192.168.0.0/16"]
/// block_ip = []
/// ```
///
/// The `[botdetection]` keys mirror `limiter.toml` upstream.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum LimiterSettings {
    /// `server.limiter = true`: enabled with the legacy flat window/limit.
    Enabled(bool),
    /// `[server.limiter]` with the botdetection configuration.
    Config(Box<LimiterConfig>),
}

impl Default for LimiterSettings {
    fn default() -> Self {
        Self::Enabled(false)
    }
}

impl LimiterSettings {
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Enabled(enabled) => *enabled,
            Self::Config(config) => config.enabled,
        }
    }

    pub fn config(&self) -> Option<&LimiterConfig> {
        match self {
            Self::Enabled(_) => None,
            Self::Config(config) => Some(config),
        }
    }
}

/// The `[server.limiter]` table.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LimiterConfig {
    /// Enabling the table form enables the limiter unless set to `false`.
    pub enabled: bool,
    /// Trusted proxies; merged with `botdetection.trusted_proxies`.
    pub trusted_proxies: Vec<String>,
    pub botdetection: BotDetection,
}

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            trusted_proxies: Vec::new(),
            botdetection: BotDetection::default(),
        }
    }
}

/// `[server.limiter.botdetection]`, mirroring upstream's `limiter.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct BotDetection {
    /// Number of leading bits that define an IPv4 client network.
    pub ipv4_prefix: u8,
    /// Number of leading bits that define an IPv6 client network.
    pub ipv6_prefix: u8,
    /// Addresses of reverse proxies whose forwarding headers are trusted.
    pub trusted_proxies: Vec<String>,
    pub ip_limit: IpLimitConfig,
    pub ip_lists: IpListsConfig,
}

impl Default for BotDetection {
    fn default() -> Self {
        Self {
            ipv4_prefix: 32,
            ipv6_prefix: 48,
            trusted_proxies: vec!["127.0.0.0/8".into(), "::1".into()],
            ip_limit: IpLimitConfig::default(),
            ip_lists: IpListsConfig::default(),
        }
    }
}

/// `[server.limiter.botdetection.ip_limit]`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct IpLimitConfig {
    /// Also rate limit link-local client networks.
    pub filter_link_local: bool,
    /// Challenge clients without a valid limiter token earlier.
    pub link_token: bool,
}

/// `[server.limiter.botdetection.ip_lists]`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct IpListsConfig {
    pub block_ip: Vec<String>,
    pub pass_ip: Vec<String>,
    /// Pass the hardcoded check.searx.space addresses.
    pub pass_searxng_org: bool,
}

impl Default for IpListsConfig {
    fn default() -> Self {
        Self {
            block_ip: Vec::new(),
            pass_ip: Vec::new(),
            pass_searxng_org: true,
        }
    }
}

fn all_locale_tags() -> Vec<String> {
    locales::LOCALE_TAGS
        .iter()
        .map(|tag| (*tag).to_string())
        .collect()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Search {
    pub safe_search: u8,
    /// Autocomplete backend chain, `` or `none` disables autocompletion.
    pub autocomplete: String,
    pub autocomplete_min: usize,
    pub default_lang: String,
    pub languages: Vec<String>,
    pub ban_time_on_fail: u64,
    pub max_ban_time_on_fail: u64,
    /// Consecutive empty answers before an engine is rested.
    pub empty_result_limit: u64,
    /// How long a rested engine stays out of the rotation.
    pub empty_result_cooldown: u64,
    pub suspended_times: BTreeMap<String, u64>,
    pub formats: Vec<String>,
    pub max_page: u32,
    pub timeout_limit: f64,
    /// How the engines of one search are started: all at once or tier by tier.
    pub strategy: SearchStrategy,
    /// Engines started per tier in the staged strategy.
    pub staged_tier: usize,
    /// The staged strategy stops starting new tiers at this many results.
    pub staged_min_results: usize,
    /// How many of the top results get their page text when the request asks
    /// for raw content. Larger means richer answers, slower fetches.
    pub raw_content_limit: usize,
    /// Strength of the URL/title query-term signal blended into the ranking.
    /// 0 keeps pure upstream position fusion; higher values lift the page that
    /// actually matches the query the way a neural ranker does.
    pub relevance_weight: f64,
    /// OpenAI-compatible embeddings endpoint for semantic re-ranking
    /// (e.g. the bundled tools/embed_server.py, or any cloud embeddings API).
    /// Empty disables semantic re-ranking.
    pub embedding_url: String,
    pub embedding_model: String,
    /// EXPERIMENTAL: directory of the local BM25 index. Empty (the default)
    /// keeps the feature off. When a directory is set its hits are merged
    /// into the upstream results.
    pub local_index_path: String,
    /// Set false to keep the index on disk but out of the ranking.
    pub local_index_merge: bool,
    /// Longest page text returned per result by include_raw_content.
    pub raw_content_chars: usize,
    /// How many extra result pages of the paging engines to merge into the
    /// answer. 0 (the default) only uses page 1; 1 adds page 2, and so on.
    pub deep_pages: u32,
    /// Run a site-scoped second pass when the named site is missing from the
    /// first pass (finds the reference page that a tutorial page outranks).
    pub site_scoped_retry: bool,
    /// How strongly canonical-site authority is blended into the ranking.
    /// Naming a site (arxiv, mdn, python, ...) lifts its canonical host and
    /// pushes its mirror copies down, which keywords and embeddings cannot see.
    pub authority_weight: f64,
    /// How strongly the cosine similarity is blended into the ranking.
    pub embedding_weight: f64,
    /// How many of the top results are re-ranked by embedding similarity.
    pub embedding_topk: usize,
}

/// Search execution strategy, upstream's default is "parallel".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchStrategy {
    /// Start every selected engine at once (SearXNG compatible).
    #[default]
    Parallel,
    /// Start engines tier by tier and stop once enough results were collected.
    Staged,
}

impl Default for Search {
    fn default() -> Self {
        let mut suspended_times = BTreeMap::new();
        suspended_times.insert("SearxEngineAccessDenied".into(), 86_400);
        suspended_times.insert("SearxEngineCaptcha".into(), 86_400);
        // A 429 usually clears in minutes; an hour is far too punishing for
        // a good engine such as Yahoo! JAPAN (upstream defaults to 3600).
        suspended_times.insert("SearxEngineTooManyRequests".into(), 300);
        suspended_times.insert("cf_SearxEngineCaptcha".into(), 1_296_000);
        suspended_times.insert("cf_SearxEngineAccessDenied".into(), 86_400);
        suspended_times.insert("recaptcha_SearxEngineCaptcha".into(), 604_800);
        Self {
            safe_search: 0,
            autocomplete: "duckduckgo".into(),
            autocomplete_min: 4,
            default_lang: String::new(),
            languages: all_locale_tags(),
            ban_time_on_fail: 5,
            max_ban_time_on_fail: 120,
            empty_result_limit: 8,
            empty_result_cooldown: 300,
            suspended_times,
            formats: OUTPUT_FORMATS.iter().map(|f| (*f).to_string()).collect(),
            max_page: 0,
            timeout_limit: 3.0,
            strategy: SearchStrategy::Parallel,
            staged_tier: 8,
            staged_min_results: 10,
            raw_content_limit: 5,
            relevance_weight: 4.0,
            embedding_url: String::new(),
            embedding_model: "BAAI/bge-small-en-v1.5".into(),
            local_index_path: String::new(),
            local_index_merge: true,
            raw_content_chars: 8_000,
            deep_pages: 0,
            site_scoped_retry: false,
            authority_weight: 8.0,
            embedding_weight: 2.5,
            embedding_topk: 25,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Ui {
    pub default_theme: String,
    pub default_locale: String,
    pub center_alignment: bool,
    pub results_on_new_tab: bool,
    pub infinite_scroll: bool,
    pub search_on_category_select: bool,
    pub hotkeys: String,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            default_theme: "simple".into(),
            default_locale: String::new(),
            center_alignment: false,
            results_on_new_tab: false,
            infinite_scroll: false,
            search_on_category_select: true,
            hotkeys: "default".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Outgoing {
    pub request_timeout: f64,
    pub max_request_timeout: f64,
    pub useragent_suffix: String,
    pub pool_connections: usize,
    pub pool_maxsize: usize,
    pub enable_http2: bool,
    pub verify: bool,
    /// Additional trusted CA certificates in a PEM bundle.
    pub ca_bundle: Option<String>,
    pub retries: u32,
    pub max_redirects: usize,
    pub keepalive_expiry: f64,
    pub proxies: BTreeMap<String, String>,
    pub using_tor_proxy: bool,
}

impl Default for Outgoing {
    fn default() -> Self {
        Self {
            request_timeout: 4.0,
            max_request_timeout: 20.0,
            useragent_suffix: String::new(),
            pool_connections: 100,
            pool_maxsize: 20,
            enable_http2: true,
            verify: true,
            ca_bundle: None,
            retries: 1,
            max_redirects: 20,
            keepalive_expiry: 5.0,
            proxies: BTreeMap::new(),
            using_tor_proxy: false,
        }
    }
}

/// Response cache configuration (upstream `cache` and `valkey` sections).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CacheSettings {
    /// Backend: "memory" (default) or "valkey".
    pub backend: String,
    /// Connection URL of the valkey backend, e.g. redis://127.0.0.1:6379/0.
    pub url: String,
    /// Entry lifetime in seconds (upstream cache.ttl).
    pub ttl: u64,
    /// Upper bound of the in-memory cache in bytes (upstream cache.max_bytes).
    pub max_bytes: usize,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            backend: "memory".to_string(),
            url: "redis://127.0.0.1:6379/0".to_string(),
            ttl: 300,
            max_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Plugins {
    /// Plugins enabled for every request (upstream `enabled_plugins`).
    pub enabled: Vec<String>,
    /// Plugins disabled for every request (upstream `disabled_plugins`).
    pub disabled: Vec<String>,
}

/// One entry of the `engines` list.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EngineConfig {
    /// Display name, used by bangs and preferences.
    pub name: String,
    /// Engine implementation id (`searx/engines/<engine>.py` upstream).
    pub engine: String,
    #[serde(default)]
    pub shortcut: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    /// `None` means "not specified"; an explicit `false` re-enables an engine
    /// that ships disabled in the defaults.
    #[serde(default)]
    pub disabled: Option<bool>,
    #[serde(default)]
    pub inactive: bool,
    #[serde(default)]
    pub weight: Option<f64>,
    #[serde(default)]
    pub timeout: Option<f64>,
    #[serde(default)]
    pub paging: Option<bool>,
    #[serde(default)]
    pub language_support: Option<bool>,
    #[serde(default)]
    pub time_range_support: Option<bool>,
    #[serde(default)]
    pub safesearch: Option<bool>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub search_url: Option<String>,
    /// Route this engine through its own proxy (upstream per-engine proxies).
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    /// Free-form, engine specific options (kept for SearXNG compatibility).
    #[serde(flatten, default)]
    pub extra: toml::Table,
}

impl EngineConfig {
    /// Build an engine config programmatically (used by the built-in defaults
    /// and by tests).
    pub fn new(name: &str, engine: &str, shortcut: &str, categories: &[&str]) -> Self {
        Self {
            name: name.into(),
            engine: engine.into(),
            shortcut: Some(shortcut.into()),
            categories: categories.iter().map(|c| (*c).to_string()).collect(),
            proxy: None,
            disabled: None,
            inactive: false,
            weight: None,
            timeout: None,
            paging: None,
            language_support: None,
            time_range_support: None,
            safesearch: None,
            base_url: None,
            search_url: None,
            api_key: None,
            extra: toml::Table::new(),
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
                    .map(|v| v as f64)
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

    /// Shortcut falling back to the engine id, like upstream.
    pub fn shortcut_or_name(&self) -> String {
        self.shortcut.clone().unwrap_or_else(|| self.engine.clone())
    }
}

/// Convert a YAML value into a TOML value (mappings become tables).
fn yaml_to_toml(value: &serde_yaml::Value) -> Option<toml::Value> {
    Some(match value {
        serde_yaml::Value::Null => return None,
        serde_yaml::Value::Bool(flag) => toml::Value::Boolean(*flag),
        serde_yaml::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                toml::Value::Integer(integer)
            } else {
                toml::Value::Float(number.as_f64()?)
            }
        }
        serde_yaml::Value::String(text) => toml::Value::String(text.clone()),
        serde_yaml::Value::Sequence(items) => {
            toml::Value::Array(items.iter().filter_map(yaml_to_toml).collect())
        }
        serde_yaml::Value::Mapping(mapping) => {
            let mut table = toml::Table::new();
            for (key, value) in mapping {
                if let Some(key) = yaml_key(key) {
                    if let Some(value) = yaml_to_toml(value) {
                        table.insert(key, value);
                    }
                }
            }
            toml::Value::Table(table)
        }
        serde_yaml::Value::Tagged(tagged) => return yaml_to_toml(&tagged.value),
    })
}

fn yaml_key(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(text) => Some(text.clone()),
        serde_yaml::Value::Bool(flag) => Some(flag.to_string()),
        serde_yaml::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

impl Settings {
    /// Parse a SearXNG `settings.yml` (YAML) document into settings.
    ///
    /// Upstream's configuration format is YAML, so a drop-in `settings.yml` is
    /// converted to the internal representation: the document is turned into TOML
    /// values first (the internal merge logic works on those).
    pub fn from_yaml_str(text: &str) -> Result<Settings, SettingsError> {
        let yaml: serde_yaml::Value = serde_yaml::from_str(text)
            .map_err(|error| SettingsError::Validation(error.to_string()))?;
        let Some(value) = yaml_to_toml(&yaml) else {
            return Err(SettingsError::Validation("empty YAML document".into()));
        };
        value.try_into().map_err(SettingsError::Parse)
    }

    /// Upstream's `server.public_instance`, also accepted under `[general]`
    /// for configurations written for older versions of this port.
    pub fn is_public_instance(&self) -> bool {
        self.general.public_instance || self.server.public_instance
    }

    /// The built-in defaults (no user file involved).
    pub fn defaults() -> Settings {
        toml::from_str(DEFAULT_SETTINGS).expect("built-in settings.toml must be valid")
    }

    /// Load settings from `path`, merged onto the built-in defaults, then apply
    /// the `SEARXNG_*` environment overrides.
    pub fn load(path: Option<&Path>) -> Result<Settings, SettingsError> {
        let mut settings = match path {
            Some(path) => {
                let raw = std::fs::read_to_string(path).map_err(|source| SettingsError::Read {
                    path: path.to_path_buf(),
                    source,
                })?;
                // Upstream configurations are YAML; TOML is also accepted.
                let is_yaml = matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("yml" | "yaml")
                );
                let user = if is_yaml {
                    Settings::from_yaml_str(&raw)?
                } else {
                    toml::from_str::<Settings>(&raw)?
                };
                Self::merge(Settings::defaults(), user)?
            }
            None => Settings::defaults(),
        };
        settings.apply_env();
        settings.validate()?;
        Ok(settings)
    }

    /// Resolve the settings path from `SEARXNG_SETTINGS_PATH` (upstream
    /// behaviour) or `--config`.
    pub fn path_from_env() -> Option<PathBuf> {
        std::env::var("SEARXNG_SETTINGS_PATH")
            .ok()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    /// Merge a user settings object onto `base`.
    ///
    /// Like upstream, scalars and sections are overridden by the user, and the
    /// engine list is merged by engine name unless `use_default_settings` is
    /// `false`.
    fn merge(mut base: Settings, user: Settings) -> Result<Settings, SettingsError> {
        let merge_engines = user.use_default_settings.unwrap_or(true);
        base.use_default_settings = user.use_default_settings;

        // Sections: only overwrite fields the user actually set. TOML gives us
        // concrete values for every field, so we merge on the serialized form.
        base.general = merge_table(&base.general, &user.general)?;
        base.server = merge_table(&base.server, &user.server)?;
        base.search = merge_table(&base.search, &user.search)?;
        base.ui = merge_table(&base.ui, &user.ui)?;
        base.outgoing = merge_table(&base.outgoing, &user.outgoing)?;
        base.plugins = merge_table(&base.plugins, &user.plugins)?;
        base.cache = merge_table(&base.cache, &user.cache)?;
        base.research = merge_table(&base.research, &user.research)?;
        base.solver = merge_table(&base.solver, &user.solver)?;
        if !user.categories_as_tabs.is_empty() {
            base.categories_as_tabs = user.categories_as_tabs;
        }
        if user.doi_resolvers.is_some() {
            base.doi_resolvers = user.doi_resolvers;
        }

        if !user.engines.is_empty() {
            if merge_engines {
                base.engines = merge_engines_by_name(base.engines, user.engines);
            } else {
                base.engines = user.engines;
            }
        }
        Ok(base)
    }

    fn apply_env(&mut self) {
        if let Ok(value) = std::env::var("SEARXNG_BIND_ADDRESS") {
            if !value.is_empty() {
                self.server.bind_address = value;
            }
        }
        if let Ok(value) = std::env::var("SEARXNG_PORT") {
            if let Ok(port) = value.parse::<u16>() {
                self.server.port = port;
            }
        }
        if let Ok(value) = std::env::var("SEARXNG_SECRET") {
            if !value.is_empty() {
                self.server.secret_key = value;
            }
        }
        if let Ok(value) = std::env::var("SEARXNG_BASE_URL") {
            if !value.is_empty() {
                self.server.base_url = Some(value);
            }
        }
        if let Ok(value) = std::env::var("SEARXNG_DEBUG") {
            self.general.debug = matches!(value.as_str(), "1" | "true" | "on");
        }
        if let Ok(value) = std::env::var("SEARXNG_ENABLE_METRICS") {
            self.general.enable_metrics = !matches!(value.as_str(), "0" | "false" | "off");
        }
        if let Ok(value) = std::env::var("SEARXNG_SEARCH_DEFAULT_LANG") {
            if !value.is_empty() {
                self.search.default_lang = value;
            }
        }
    }

    /// Validate the settings the way upstream does before starting.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.server.port == 0 {
            return Err(SettingsError::Validation(
                "server.port must not be 0".into(),
            ));
        }
        if self.server.method != "GET" && self.server.method != "POST" {
            return Err(SettingsError::Validation(format!(
                "server.method must be GET or POST, got {:?}",
                self.server.method
            )));
        }
        if let Some(config) = self.server.limiter_config() {
            if config.botdetection.ipv4_prefix > 32 {
                return Err(SettingsError::Validation(format!(
                    "server.limiter.botdetection.ipv4_prefix must be 0-32, got {}",
                    config.botdetection.ipv4_prefix
                )));
            }
            if config.botdetection.ipv6_prefix > 128 {
                return Err(SettingsError::Validation(format!(
                    "server.limiter.botdetection.ipv6_prefix must be 0-128, got {}",
                    config.botdetection.ipv6_prefix
                )));
            }
        }
        if self.search.safe_search > 2 {
            return Err(SettingsError::Validation(
                "search.safe_search must be 0, 1 or 2".into(),
            ));
        }
        if self.outgoing.request_timeout <= 0.0 || self.outgoing.max_request_timeout <= 0.0 {
            return Err(SettingsError::Validation(
                "outgoing timeouts must be positive".into(),
            ));
        }
        if self.outgoing.max_request_timeout < self.outgoing.request_timeout {
            return Err(SettingsError::Validation(
                "outgoing.max_request_timeout must be >= outgoing.request_timeout".into(),
            ));
        }
        for format in &self.search.formats {
            if !OUTPUT_FORMATS.contains(&format.as_str()) {
                return Err(SettingsError::Validation(format!(
                    "unknown search format {format:?} (allowed: {})",
                    OUTPUT_FORMATS.join(", ")
                )));
            }
        }
        let mut names: BTreeMap<&str, &str> = BTreeMap::new();
        let mut shortcuts: BTreeMap<&str, &str> = BTreeMap::new();
        for engine in &self.engines {
            if engine.name.trim().is_empty() {
                return Err(SettingsError::Validation(
                    "engine entries need a name".into(),
                ));
            }
            if engine.engine.trim().is_empty() {
                return Err(SettingsError::Validation(format!(
                    "engine {:?} needs an engine type",
                    engine.name
                )));
            }
            if let Some(previous) = names.insert(engine.name.as_str(), engine.engine.as_str()) {
                return Err(SettingsError::Validation(format!(
                    "duplicate engine name {:?} ({previous} and {})",
                    engine.name, engine.engine
                )));
            }
            if let Some(shortcut) = engine.shortcut.as_deref() {
                if let Some(previous) = shortcuts.insert(shortcut, engine.name.as_str()) {
                    return Err(SettingsError::Validation(format!(
                        "duplicate engine shortcut {shortcut:?} ({previous} and {})",
                        engine.name
                    )));
                }
            }
        }
        Ok(())
    }

    /// Engines that are enabled by default, in config order.
    pub fn enabled_engines(&self) -> impl Iterator<Item = &EngineConfig> {
        self.engines
            .iter()
            .filter(|engine| engine.disabled != Some(true) && !engine.inactive)
    }

    pub fn engine(&self, name: &str) -> Option<&EngineConfig> {
        self.engines.iter().find(|engine| engine.name == name)
    }

    /// Default language list limited to what the UI offers.
    pub fn languages(&self) -> Vec<String> {
        self.search.languages.clone()
    }

    /// The base URL used for absolute links in feeds/opensearch.
    pub fn base_url(&self) -> String {
        self.server.base_url.clone().unwrap_or_else(|| {
            let host = if self.server.bind_address == "0.0.0.0" {
                "localhost"
            } else {
                self.server.bind_address.as_str()
            };
            format!("http://{host}:{}", self.server.port)
        })
    }
}

fn merge_table<T>(base: &T, user: &T) -> Result<T, SettingsError>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    let base_value = toml::Value::try_from(base)
        .map_err(|error| SettingsError::Validation(error.to_string()))?;
    let user_value = toml::Value::try_from(user)
        .map_err(|error| SettingsError::Validation(error.to_string()))?;
    let merged = deep_merge(base_value, user_value);
    merged
        .try_into()
        .map_err(|error: toml::de::Error| SettingsError::Parse(error))
}

fn deep_merge(base: toml::Value, user: toml::Value) -> toml::Value {
    match (base, user) {
        (toml::Value::Table(mut base), toml::Value::Table(user)) => {
            for (key, value) in user {
                let merged = match base.remove(&key) {
                    Some(existing) => deep_merge(existing, value),
                    None => value,
                };
                base.insert(key, merged);
            }
            toml::Value::Table(base)
        }
        (_, user) => user,
    }
}

/// Merge user engine entries into the default list by engine name.
fn merge_engines_by_name(
    defaults: Vec<EngineConfig>,
    user: Vec<EngineConfig>,
) -> Vec<EngineConfig> {
    let mut merged = defaults;
    for engine in user {
        match merged.iter_mut().find(|entry| entry.name == engine.name) {
            Some(existing) => {
                // User entries win field by field; unset fields keep the default.
                // The implementation id is a plain string, so an explicit
                // "engine = ..." in the user file must replace the default one:
                // otherwise a user cannot swap an engine to another
                // implementation (for example anaconda's HTML XPath version to
                // its JSON API) and the override silently does nothing.
                if !engine.engine.is_empty() {
                    existing.engine = engine.engine.clone();
                }
                if engine.shortcut.is_some() {
                    existing.shortcut = engine.shortcut.clone();
                }
                if !engine.categories.is_empty() {
                    existing.categories = engine.categories.clone();
                }
                if let Some(disabled) = engine.disabled {
                    existing.disabled = Some(disabled);
                }
                if engine.inactive {
                    existing.inactive = true;
                }
                if engine.weight.is_some() {
                    existing.weight = engine.weight;
                }
                if engine.timeout.is_some() {
                    existing.timeout = engine.timeout;
                }
                if engine.paging.is_some() {
                    existing.paging = engine.paging;
                }
                if engine.language_support.is_some() {
                    existing.language_support = engine.language_support;
                }
                if engine.time_range_support.is_some() {
                    existing.time_range_support = engine.time_range_support;
                }
                if engine.safesearch.is_some() {
                    existing.safesearch = engine.safesearch;
                }
                if engine.base_url.is_some() {
                    existing.base_url = engine.base_url.clone();
                }
                if engine.search_url.is_some() {
                    existing.search_url = engine.search_url.clone();
                }
                if engine.proxy.is_some() {
                    existing.proxy = engine.proxy.clone();
                }
                if engine.api_key.is_some() {
                    existing.api_key = engine.api_key.clone();
                }
                for (key, value) in engine.extra {
                    existing.extra.insert(key, value);
                }
            }
            None => merged.push(engine),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_engine_entry_can_set_its_own_proxy() {
        let defaults = Settings::defaults();
        let user: Settings = toml::from_str(
            r#"
[[engines]]
name = "mwmbl"
engine = "mwmbl"
proxy = "http://127.0.0.1:18888"
"#,
        )
        .expect("user settings parse");
        let merged = Settings::merge(defaults, user).expect("merge");
        let engine = merged.engine("mwmbl").expect("mwmbl kept");
        assert_eq!(engine.proxy.as_deref(), Some("http://127.0.0.1:18888"));
        // engines without the key stay direct
        let other = merged.engine("swisscows").expect("swisscows kept");
        assert!(other.proxy.is_none());
    }

    #[test]
    fn a_user_solver_section_reaches_the_merged_settings() {
        let defaults = Settings::defaults();
        assert!(!defaults.solver.enabled, "the solver ships switched off");
        let user: Settings = toml::from_str(
            r#"
[solver]
enabled = true
endpoint = "http://127.0.0.1:8191/v1"
budget_ms = 4000
"#,
        )
        .expect("user settings parse");
        let merged = Settings::merge(defaults, user).expect("merge");
        assert!(merged.solver.enabled, "the user section must be merged");
        assert_eq!(merged.solver.budget_ms, 4000);
        // untouched keys keep their defaults
        assert_eq!(merged.solver.max_timeout_ms, 45_000);
    }

    #[test]
    fn a_user_can_enable_an_engine_that_ships_disabled() {
        let defaults = Settings::defaults();
        // exaapi ships disabled in the defaults
        let shipped = defaults.engine("exaapi").expect("exaapi exists");
        assert_eq!(shipped.disabled, Some(true));

        let user: Settings = toml::from_str(
            r#"
[[engines]]
name = "exaapi"
engine = "exaapi"
shortcut = "exa"
categories = ["general", "web"]
disabled = false
api_key = "test-key"
"#,
        )
        .expect("user settings parse");
        let merged = Settings::merge(defaults, user).expect("merge");
        let enabled = merged.engine("exaapi").expect("exaapi kept");
        assert_eq!(enabled.disabled, Some(false), "explicit false must win");
        assert_eq!(enabled.api_key.as_deref(), Some("test-key"));
    }

    #[test]
    fn a_user_engine_entry_can_switch_the_implementation() {
        let defaults = Settings::defaults();
        // anaconda ships as an HTML XPath engine in the defaults
        let shipped = defaults.engine("anaconda").expect("anaconda exists");
        assert_eq!(shipped.engine, "xpath");

        let user: Settings = toml::from_str(
            r#"
[[engines]]
name = "anaconda"
engine = "json_engine"
search_url = "https://api.anaconda.org/search?name={query}"
title_query = "full_name"
disabled = false
"#,
        )
        .expect("user settings parse");
        let merged = Settings::merge(defaults, user).expect("merge");
        let engine = merged.engine("anaconda").expect("anaconda kept");
        assert_eq!(engine.engine, "json_engine", "implementation override wins");
        assert_eq!(
            engine.search_url.as_deref(),
            Some("https://api.anaconda.org/search?name={query}")
        );
    }

    #[test]
    fn builtin_defaults_are_valid() {
        let settings = Settings::defaults();
        settings.validate().expect("defaults must validate");
        assert!(!settings.engines.is_empty());
        assert_eq!(settings.server.port, 29483);
    }

    #[test]
    fn builtin_default_port_is_29483() {
        assert_eq!(Server::default().port, 29483);
    }

    #[test]
    fn an_explicit_legacy_port_8888_in_user_config_still_wins() {
        let user: Settings = toml::from_str(
            r#"
use_default_settings = true
[server]
port = 8888
"#,
        )
        .expect("user settings parse");
        let merged = Settings::merge(Settings::defaults(), user).expect("merge");
        assert_eq!(merged.server.port, 8888);
        merged.validate().expect("legacy port must validate");
    }

    #[test]
    fn upstream_yaml_settings_are_accepted() {
        let yaml = r#"
use_default_settings: true
search:
  safe_search: 2
  default_lang: "ja"
server:
  port: 8880
engines:
  - name: "wikipedia (ja)"
    engine: wikipedia
    shortcut: wja
    categories: ["general"]
    base_url: "https://ja.wikipedia.org"
"#;
        let user = Settings::from_yaml_str(yaml).expect("valid yaml");
        let merged = Settings::merge(Settings::defaults(), user).expect("merge");
        assert_eq!(merged.server.port, 8880);
        assert_eq!(merged.search.safe_search, 2);
        assert_eq!(merged.search.default_lang, "ja");
        assert!(merged.engine("wikipedia (ja)").is_some());
    }

    #[test]
    fn user_engines_merge_by_name() {
        let defaults = Settings::defaults();
        let user: Settings = toml::from_str(
            r#"
            use_default_settings = true
            [[engines]]
            name = "duckduckgo"
            engine = "duckduckgo"
            disabled = true
            [[engines]]
            name = "my instance"
            engine = "wikipedia"
            shortcut = "mywiki"
            categories = ["general"]
            "#,
        )
        .unwrap();
        let merged = Settings::merge(defaults, user).unwrap();
        let duckduckgo = merged.engine("duckduckgo").expect("duckduckgo kept");
        assert_eq!(duckduckgo.disabled, Some(true));
        assert!(merged.engine("my instance").is_some());
    }

    #[test]
    fn invalid_format_is_rejected() {
        let mut settings = Settings::defaults();
        settings.search.formats.push("yaml".into());
        assert!(settings.validate().is_err());
    }

    #[test]
    fn duplicate_shortcuts_are_rejected() {
        let mut settings = Settings::defaults();
        let mut clone = settings.engines[0].clone();
        clone.name = "clone of the first engine".into();
        clone.shortcut = settings.engines[0].shortcut.clone();
        settings.engines.push(clone);
        assert!(settings.validate().is_err());
    }

    #[test]
    fn a_shipped_engine_never_lists_single_letter_categories() {
        // The shipped settings once spelled `["g", "e", "n", "e", "r", "a", "l"]`
        // instead of `["general"]`, which silently dropped those engines from
        // every category search. A category name is always longer than one
        // character, so a lone letter is always that typo.
        let settings = Settings::defaults();
        let mut broken = Vec::new();
        for engine in &settings.engines {
            if engine
                .categories
                .iter()
                .any(|category| category.chars().count() == 1)
            {
                broken.push(format!("{}: {:?}", engine.name, engine.categories));
            }
        }
        assert!(broken.is_empty(), "single letter categories: {broken:?}");
    }
}
