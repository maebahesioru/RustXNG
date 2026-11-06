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
    /// Page fetching for MCP `read_page` and HTTP `include_raw_content`.
    #[serde(default)]
    pub fetch: FetchSettings,
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

/// Page fetching backend. `direct` keeps the built-in fetcher that validates
/// every destination locally. `deepseek` sends the URL to the DeepSeek Fetch
/// API, which returns Markdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FetchProvider {
    #[default]
    Direct,
    DeepSeek,
}

/// Page fetching configuration for MCP `read_page` and HTTP raw content.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FetchSettings {
    pub provider: FetchProvider,
    /// Request timeout for one page fetch, in seconds.
    pub timeout: f64,
    pub deepseek: DeepSeekFetchSettings,
}

impl Default for FetchSettings {
    fn default() -> Self {
        Self {
            provider: FetchProvider::Direct,
            timeout: 20.0,
            deepseek: DeepSeekFetchSettings::default(),
        }
    }
}

/// Credentials of the DeepSeek Fetch API.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct DeepSeekFetchSettings {
    pub base_url: String,
    pub api_key: String,
}

impl Default for DeepSeekFetchSettings {
    fn default() -> Self {
        Self {
            base_url: "https://api.deepseek.com".into(),
            api_key: String::new(),
        }
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
            secret_key: String::new(),
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
    /// EXPERIMENTAL: directory of an index built by `rustxng-wikidump` (the
    /// opening of every Wikipedia article). Empty keeps it off. It is matched
    /// loosely (one content word is enough, ranking decides) and merged like
    /// any other source; it follows `local_index_merge`.
    pub wikipedia_index_path: String,
    /// Add the official website (Wikidata P856) of the top Wikipedia articles
    /// a general search found. One request to the Wikidata Query Service.
    pub official_sites: bool,
    /// Weight of those official-site results in the merge.
    pub official_sites_weight: f64,
    /// How long the merge waits for that lookup once the engines are done,
    /// in milliseconds. The Query Service usually answers in about half a
    /// second but sometimes takes seconds; a later answer is dropped.
    pub official_sites_wait_ms: u64,
    /// When not empty, only engines named here (display name or engine id,
    /// case-insensitive) are enabled and every other entry is off. An engine
    /// listed here runs even if it ships disabled. This is how a reviewed,
    /// "clean" provider set is pinned (see `settings.clean.toml`).
    pub engine_allowlist: Vec<String>,
    /// Longest page text returned per result by include_raw_content.
    pub raw_content_chars: usize,
    /// Optional command for JavaScript page rendering.
    pub page_renderer_command: Vec<String>,
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
            wikipedia_index_path: String::new(),
            official_sites: true,
            official_sites_weight: 1.0,
            official_sites_wait_ms: 400,
            engine_allowlist: Vec::new(),
            raw_content_chars: 8_000,
            page_renderer_command: Vec::new(),
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
    /// Announce ourselves honestly: every upstream request carries the fixed
    /// `rustxng/<version> (+repo; contact)` User-Agent instead of a rotating
    /// browser look-alike. API providers (Wikimedia, crates.io, Crossref, ...)
    /// ask for an identifiable client with a contact.
    pub identified_ua: bool,
    /// Contact (mail address or URL) added to the identified User-Agent.
    pub contact: String,
    /// Minimum pause between two requests to the same host, in milliseconds.
    /// 0 disables it. API etiquette is often "at most one request a second".
    pub host_spacing_ms: u64,
    /// Per-host exceptions to `host_spacing_ms`, keyed by host or domain
    /// (`"wikipedia.org"` covers every edition). The most specific key wins.
    pub host_spacing_overrides: BTreeMap<String, u64>,
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
            identified_ua: false,
            contact: String::new(),
            host_spacing_ms: 0,
            host_spacing_overrides: BTreeMap::new(),
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
    /// Engine implementation id (`searx/engines/<engine>.py` upstream). May
    /// be left out of a user entry that overrides a default engine by name.
    #[serde(default)]
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
fn yaml_to_toml(value: &yaml_serde::Value) -> Option<toml::Value> {
    Some(match value {
        yaml_serde::Value::Null => return None,
        yaml_serde::Value::Bool(flag) => toml::Value::Boolean(*flag),
        yaml_serde::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                toml::Value::Integer(integer)
            } else {
                toml::Value::Float(number.as_f64()?)
            }
        }
        yaml_serde::Value::String(text) => toml::Value::String(text.clone()),
        yaml_serde::Value::Sequence(items) => {
            toml::Value::Array(items.iter().filter_map(yaml_to_toml).collect())
        }
        yaml_serde::Value::Mapping(mapping) => {
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
        yaml_serde::Value::Tagged(tagged) => return yaml_to_toml(&tagged.value),
    })
}

fn yaml_key(value: &yaml_serde::Value) -> Option<String> {
    match value {
        yaml_serde::Value::String(text) => Some(text.clone()),
        yaml_serde::Value::Bool(flag) => Some(flag.to_string()),
        yaml_serde::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

impl Settings {
    /// Initialise the proxy signing key once per Searcher. Never retain the
    /// historical sample key, including when loaded from an older config.
    pub(crate) fn ensure_secret_key(&mut self) -> Result<(), String> {
        if self.server.secret_key.trim().is_empty() || self.server.secret_key == "ultrasecretkey" {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes)
                .map_err(|error| format!("cannot generate a proxy signing key: {error}"))?;
            self.server.secret_key = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        }
        Ok(())
    }

    /// Parse a SearXNG `settings.yml` (YAML) document and merge it onto the
    /// built-in defaults. Upstream's configuration format is YAML, so a
    /// drop-in `settings.yml` is converted to TOML values first (the merge
    /// logic works on those). Unlike [`Settings::load`] this does not apply the
    /// `SEARXNG_*` overrides and does not validate.
    pub fn from_yaml_str(text: &str) -> Result<Settings, SettingsError> {
        Self::merge_values(Self::defaults_value(), Self::user_value(text, true)?)
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

    /// The built-in defaults as raw TOML values, the base of every merge.
    ///
    /// `search.suspended_times` ships only in `Search::default()`, so it is
    /// seeded here. Without the seed a user table would replace the shipped
    /// entries instead of extending them. A table in the defaults file wins
    /// over the Rust entries, so the seed merges instead of replacing.
    fn defaults_value() -> toml::Value {
        let mut value: toml::Value =
            toml::from_str(DEFAULT_SETTINGS).expect("built-in settings.toml must be valid");
        if let Some(toml::Value::Table(search)) = value.get_mut("search") {
            let mut suspended = toml::Value::try_from(Search::default().suspended_times)
                .expect("suspended_times is a table of integers");
            if let Some(shipped) = search.remove("suspended_times") {
                suspended = deep_merge(suspended, shipped);
            }
            search.insert("suspended_times".into(), suspended);
        }
        value
    }

    /// Parse a user document into raw TOML values. YAML is converted first so
    /// both formats share one merge path.
    fn user_value(text: &str, is_yaml: bool) -> Result<toml::Value, SettingsError> {
        if is_yaml {
            let yaml: yaml_serde::Value = yaml_serde::from_str(text)
                .map_err(|error| SettingsError::Validation(error.to_string()))?;
            yaml_to_toml(&yaml)
                .ok_or_else(|| SettingsError::Validation("empty YAML document".into()))
        } else {
            toml::from_str::<toml::Value>(text).map_err(SettingsError::Parse)
        }
    }

    /// Load settings from `path`, merged onto the built-in defaults, then apply
    /// the `SEARXNG_*` environment overrides.
    pub fn load(path: Option<&Path>) -> Result<Settings, SettingsError> {
        Self::load_profile(path, Profile::Standard)
    }

    /// Load settings for a profile: the built-in defaults, then the profile's
    /// overlay (built into the binary), then the file at `path`, then the
    /// `SEARXNG_*` environment overrides. A later layer wins.
    pub fn load_profile(path: Option<&Path>, profile: Profile) -> Result<Settings, SettingsError> {
        let overlay = profile
            .overlay()
            .map(|text| toml::from_str::<toml::Value>(text).map_err(SettingsError::Parse))
            .transpose()?;
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
                let user = Self::user_value(&raw, is_yaml)?;
                let user = match overlay {
                    Some(overlay) => layer(overlay, user),
                    None => user,
                };
                Self::merge_values(Self::defaults_value(), user).map_err(|error| {
                    // The merged deserialize works on values without spans, so a
                    // TOML type error loses its line and column. Re-parsing the
                    // user file alone recovers the spanned message. The two
                    // parsers disagree on TOML datetimes, so only swap in the
                    // re-parsed error when it reports the same problem.
                    if is_yaml {
                        return error;
                    }
                    if let SettingsError::Parse(original) = &error {
                        if let Err(spanned) = toml::from_str::<Settings>(&raw) {
                            if spanned.message() == original.message() {
                                return SettingsError::Parse(spanned);
                            }
                        }
                    }
                    error
                })?
            }
            None => match overlay {
                Some(overlay) => Self::merge_values(Self::defaults_value(), overlay)?,
                None => Settings::defaults(),
            },
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

    /// Merge a raw user document onto the raw built-in defaults, then
    /// deserialize once.
    ///
    /// Merging raw values is what keeps "not specified" apart from an explicit
    /// `false`, `0` or `[]`: deserializing the user file first fills every
    /// missing key with the Rust `Default` and loses that difference.
    ///
    /// The engine list is the one exception. It merges by engine name unless
    /// `use_default_settings` is `false`, so it is taken out of the raw
    /// document and merged on the typed form afterwards.
    fn merge_values(base: toml::Value, mut user: toml::Value) -> Result<Settings, SettingsError> {
        let user_engines = strip_engines(&mut user);
        let merged = deep_merge(base, user);
        let mut settings: Settings = merged.try_into().map_err(SettingsError::Parse)?;

        if settings.use_default_settings == Some(false) {
            settings.engines.clear();
        }

        if let Some(engines) = user_engines {
            let engines: Vec<EngineConfig> = engines.try_into().map_err(SettingsError::Parse)?;
            settings.engines = if settings.use_default_settings.unwrap_or(true) {
                merge_engines_by_name(settings.engines, engines)
            } else {
                engines
            };
        }
        Ok(settings)
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
        if let Some(engine) = self
            .engines
            .iter()
            .find(|engine| engine.engine.trim().is_empty())
        {
            return Err(SettingsError::Validation(format!(
                "engine {:?} needs `engine` (only entries that override a default engine by name may leave it out)",
                engine.name
            )));
        }
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
        if !self.fetch.timeout.is_finite()
            || self.fetch.timeout <= 0.0
            || std::time::Duration::try_from_secs_f64(self.fetch.timeout).is_err()
        {
            return Err(SettingsError::Validation(
                "fetch.timeout must be a finite positive duration".into(),
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
        let allowlist: Vec<String> = self
            .search
            .engine_allowlist
            .iter()
            .map(|name| name.trim().to_lowercase())
            .collect();
        self.engines.iter().filter(move |engine| {
            if engine.inactive {
                return false;
            }
            if allowlist.is_empty() {
                return engine.disabled != Some(true);
            }
            allowlist.contains(&engine.name.to_lowercase())
                || allowlist.contains(&engine.engine.to_lowercase())
        })
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

/// Take the `engines` array out of a raw document. Engine lists merge by name
/// on the typed form, never by value.
/// A set of built-in settings to start from (`--profile`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// `settings.toml`: keyless providers that do not refuse automated use.
    #[default]
    Standard,
    /// `settings.full.toml`: the standard set plus HTML search engines whose
    /// terms prohibit or do not permit automated access. Private use only.
    Full,
    /// `settings.clean.toml`: documented public APIs and our own crawl only.
    Clean,
}

impl Profile {
    pub const NAMES: &'static str = "standard, full, clean";

    /// Parse a profile name (`default` and `normal` mean `standard`).
    pub fn parse(name: &str) -> Result<Profile, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "standard" | "default" | "normal" => Ok(Profile::Standard),
            "full" => Ok(Profile::Full),
            "clean" => Ok(Profile::Clean),
            other => Err(format!(
                "unknown profile {other:?} (choose one of: {})",
                Profile::NAMES
            )),
        }
    }

    /// The profile named by `RUSTXNG_PROFILE`, if set.
    pub fn from_env() -> Result<Option<Profile>, String> {
        match std::env::var("RUSTXNG_PROFILE") {
            Ok(value) if !value.trim().is_empty() => Profile::parse(&value).map(Some),
            _ => Ok(None),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Profile::Standard => "standard",
            Profile::Full => "full",
            Profile::Clean => "clean",
        }
    }

    /// The settings this profile lays over the defaults.
    fn overlay(self) -> Option<&'static str> {
        match self {
            Profile::Standard => None,
            Profile::Full => Some(include_str!("../settings.full.toml")),
            Profile::Clean => Some(include_str!("../settings.clean.toml")),
        }
    }
}

/// Lay `user` over `profile`: tables merge key by key and the engine lists
/// are concatenated, so a user entry still overrides the profile's by name.
fn layer(mut profile: toml::Value, mut user: toml::Value) -> toml::Value {
    let engines = match (strip_engines(&mut profile), strip_engines(&mut user)) {
        (Some(toml::Value::Array(mut first)), Some(toml::Value::Array(second))) => {
            first.extend(second);
            Some(toml::Value::Array(first))
        }
        (first, second) => second.or(first),
    };
    let mut merged = deep_merge(profile, user);
    if let (Some(engines), toml::Value::Table(table)) = (engines, &mut merged) {
        table.insert("engines".into(), engines);
    }
    merged
}

fn strip_engines(value: &mut toml::Value) -> Option<toml::Value> {
    match value {
        toml::Value::Table(table) => table.remove("engines"),
        _ => None,
    }
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
    fn signing_keys_are_random_for_empty_or_legacy_defaults_and_custom_keys_survive() {
        let mut first = Settings::defaults();
        first.ensure_secret_key().unwrap();
        let mut second = Settings::defaults();
        second.server.secret_key = "ultrasecretkey".into();
        second.ensure_secret_key().unwrap();
        assert_eq!(first.server.secret_key.len(), 64);
        assert!(
            first
                .server
                .secret_key
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        );
        assert_eq!(second.server.secret_key.len(), 64);
        assert_ne!(first.server.secret_key, second.server.secret_key);
        let original = first.server.secret_key.clone();
        first.ensure_secret_key().unwrap();
        assert_eq!(first.server.secret_key, original);
        second.server.secret_key = "operator-managed-key".into();
        second.ensure_secret_key().unwrap();
        assert_eq!(second.server.secret_key, "operator-managed-key");
    }

    /// Merge `text` the way `Settings::load` does, but without the `SEARXNG_*`
    /// overrides, so an ambient environment variable cannot flip an assertion.
    fn merged(text: &str, is_yaml: bool) -> Settings {
        let user = Settings::user_value(text, is_yaml).expect("user settings parse");
        Settings::merge_values(Settings::defaults_value(), user).expect("settings merge")
    }

    fn merged_toml(text: &str) -> Settings {
        merged(text, false)
    }

    fn merged_yaml(text: &str) -> Settings {
        merged(text, true)
    }

    /// Load `text` through the real file path and `Settings::load`, the entry
    /// point the server uses.
    fn load_file(file_name: &str, text: &str) -> Settings {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(file_name);
        std::fs::write(&path, text).expect("write the settings file");
        Settings::load(Some(&path)).expect("settings load")
    }

    #[test]
    fn a_partial_file_loads_through_the_cli_entry_point() {
        let settings = load_file("settings.toml", "[server]\nport = 9999\n");

        assert_eq!(settings.cache.max_bytes, 33_554_432);
        assert_eq!(settings.ui.default_locale, "en");
        assert!(settings.engine("wikipedia").is_some());
    }

    #[test]
    fn a_yaml_file_loads_through_the_cli_entry_point() {
        let settings = load_file("settings.yml", "server:\n  port: 9999\n");

        assert_eq!(settings.cache.max_bytes, 33_554_432);
        assert_eq!(settings.ui.default_locale, "en");
    }

    #[test]
    fn a_partial_user_file_keeps_the_shipped_defaults() {
        let defaults = Settings::defaults();
        let settings = merged_toml("[server]\nport = 9999\n");

        assert_eq!(settings.server.port, 9999);
        assert_eq!(settings.cache.max_bytes, 33_554_432);
        assert_eq!(settings.search.default_lang, "auto");
        assert_eq!(settings.ui.default_locale, "en");
        assert_eq!(
            settings.general.instance_name,
            defaults.general.instance_name
        );
        assert_eq!(settings.search.languages, defaults.search.languages);
        assert_eq!(
            settings.outgoing.request_timeout,
            defaults.outgoing.request_timeout
        );
    }

    #[test]
    fn an_empty_user_file_keeps_every_default() {
        let defaults = Settings::defaults();
        let settings = merged_toml("");

        assert_eq!(settings.server.port, defaults.server.port);
        assert_eq!(settings.cache.max_bytes, defaults.cache.max_bytes);
        assert_eq!(settings.ui.default_locale, defaults.ui.default_locale);
        assert_eq!(settings.engines.len(), defaults.engines.len());
    }

    #[test]
    fn a_partial_yaml_file_keeps_the_shipped_defaults() {
        let settings = merged_yaml("server:\n  port: 9999\n");

        assert_eq!(settings.server.port, 9999);
        assert_eq!(settings.ui.default_locale, "en");
        assert_eq!(settings.search.default_lang, "auto");
    }

    #[test]
    fn a_partial_suspension_map_extends_the_shipped_entries() {
        let settings = merged_toml(
            r#"
[search.suspended_times]
SearxEngineTooManyRequests = 60
"#,
        );

        let suspended = &settings.search.suspended_times;
        assert_eq!(suspended.get("SearxEngineTooManyRequests"), Some(&60));
        assert_eq!(suspended.get("SearxEngineCaptcha"), Some(&86_400));
        assert_eq!(suspended.get("cf_SearxEngineCaptcha"), Some(&1_296_000));
    }

    #[test]
    fn a_nested_limiter_table_keeps_the_botdetection_defaults() {
        let settings = merged_toml(
            r#"
[server.limiter]
enabled = true

[server.limiter.botdetection.ip_limit]
filter_link_local = true
"#,
        );

        assert!(settings.server.limiter_enabled());
        let limiter = settings.server.limiter_config().expect("table form");
        assert_eq!(limiter.botdetection.ipv4_prefix, 32);
        assert_eq!(
            limiter.botdetection.trusted_proxies,
            vec!["127.0.0.0/8".to_string(), "::1".to_string()]
        );
        assert!(limiter.botdetection.ip_limit.filter_link_local);
        assert!(!limiter.botdetection.ip_limit.link_token);
        assert!(limiter.botdetection.ip_lists.pass_searxng_org);
    }

    #[test]
    fn engine_specific_options_survive_the_merge() {
        let settings = merged_toml(
            r#"
[[engines]]
name = "my xpath"
engine = "xpath"
shortcut = "mx"
categories = ["general"]
content_xpath = "//p"
"#,
        );

        // shipped engine options (the flattened `extra` table) stay
        let shipped = settings.engine("stack overflow").expect("stack overflow");
        assert_eq!(shipped.opt_str("api_site"), Some("stackoverflow"));
        // and a user engine keeps its own options
        let added = settings.engine("my xpath").expect("my xpath");
        assert_eq!(added.opt_str("content_xpath"), Some("//p"));
    }

    #[test]
    fn a_user_tab_list_is_kept() {
        let settings = merged_toml("categories_as_tabs = [\"general\"]\n");

        assert_eq!(settings.categories_as_tabs, vec!["general".to_string()]);
    }

    #[test]
    fn from_yaml_str_merges_onto_the_defaults() {
        let settings = Settings::from_yaml_str(
            r#"
server:
  port: 9999
engines:
  - name: "google"
    engine: google
    disabled: true
"#,
        )
        .expect("valid yaml");

        assert_eq!(settings.server.port, 9999);
        assert_eq!(settings.cache.max_bytes, 33_554_432);
        assert_eq!(settings.ui.default_locale, "en");
        assert_eq!(
            settings.engine("google").expect("google").disabled,
            Some(true)
        );
        assert!(settings.engine("wikipedia").is_some());
    }

    #[test]
    fn a_wrong_value_type_is_a_parse_error_that_names_the_line() {
        let user = Settings::user_value("[server]\nport = \"8888\"\n", false).expect("valid toml");
        let error = Settings::merge_values(Settings::defaults_value(), user)
            .expect_err("a string port is rejected");
        assert!(matches!(error, SettingsError::Parse(_)), "{error}");

        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, "[server]\nport = \"8888\"\n").expect("write the settings file");
        let error = Settings::load(Some(&path)).expect_err("a string port is rejected");
        assert!(error.to_string().contains("line 2"), "{error}");
    }

    #[test]
    fn an_empty_yaml_file_is_rejected_as_validation() {
        let error = Settings::user_value("", true).expect_err("an empty document");
        assert!(matches!(error, SettingsError::Validation(_)), "{error}");
    }

    #[test]
    fn an_explicit_empty_format_list_is_kept() {
        assert!(!Settings::defaults().search.formats.is_empty());

        let settings = merged_toml("[search]\nformats = []\n");
        assert!(
            settings.search.formats.is_empty(),
            "explicit [] disables the extra formats"
        );
        assert_eq!(settings.search.default_lang, "auto");
        assert_eq!(settings.ui.default_locale, "en");
    }

    #[test]
    fn an_explicit_false_is_kept() {
        assert!(Settings::defaults().general.enable_metrics);

        let settings = merged_toml("[general]\nenable_metrics = false\n");
        assert!(!settings.general.enable_metrics);
        assert_eq!(settings.ui.default_locale, "en");
    }

    #[test]
    fn an_explicit_zero_is_kept() {
        assert_eq!(Settings::defaults().outgoing.retries, 1);

        let settings = merged_toml("[outgoing]\nretries = 0\n");
        assert_eq!(settings.outgoing.retries, 0);
        assert_eq!(settings.cache.max_bytes, 33_554_432);
    }

    #[test]
    fn a_partial_file_still_merges_engines_by_name() {
        let settings = merged_toml(
            r#"
[server]
port = 9999

[[engines]]
name = "google"
engine = "google"
disabled = true
"#,
        );

        assert_eq!(
            settings.engine("google").expect("google").disabled,
            Some(true)
        );
        assert!(
            settings.engine("wikipedia").is_some(),
            "shipped engines stay"
        );
        assert_eq!(settings.ui.default_locale, "en");
    }

    #[test]
    fn use_default_settings_false_replaces_the_engine_list() {
        let settings = merged_toml(
            r#"
use_default_settings = false

[[engines]]
name = "only"
engine = "wikipedia"
shortcut = "only"
"#,
        );

        assert_eq!(settings.engines.len(), 1);
        assert_eq!(settings.engines[0].name, "only");
        assert_eq!(settings.server.port, 29483);
    }

    #[test]
    fn disabling_default_engines_never_restores_them_for_empty_lists() {
        for text in [
            "use_default_settings = false\nengines = []\n",
            "use_default_settings = false\n",
        ] {
            assert!(merged_toml(text).engines.is_empty());
        }
        for text in [
            "use_default_settings: false\nengines: []\n",
            "use_default_settings: false\n",
        ] {
            assert!(merged_yaml(text).engines.is_empty());
        }
        assert!(!merged_toml("engines = []\n").engines.is_empty());
    }

    #[test]
    fn restricted_html_providers_are_not_enabled_by_default() {
        let settings = Settings::defaults();
        for name in ["yahoo japan", "mojeek"] {
            assert_eq!(settings.engine(name).unwrap().disabled, Some(true));
            assert!(!settings.enabled_engines().any(|engine| engine.name == name));
        }
        let partial = merged_toml("[server]\nport = 9999\n");
        assert_eq!(partial.engine("yahoo japan").unwrap().disabled, Some(true));
        assert_eq!(partial.engine("mojeek").unwrap().disabled, Some(true));
    }

    #[test]
    fn an_allowlist_enables_exactly_the_named_engines() {
        let settings = merged_toml(
            r#"
[search]
engine_allowlist = ["Wikipedia", "crates", "mojeek"]
"#,
        );
        let names: Vec<&str> = settings
            .enabled_engines()
            .map(|engine| engine.name.as_str())
            .collect();
        assert!(names.contains(&"wikipedia"), "{names:?}");
        // Matched by engine id too, and an allowed engine runs even if it
        // ships disabled.
        assert!(names.contains(&"crates.io"), "{names:?}");
        assert!(names.contains(&"mojeek"), "{names:?}");
        assert!(!names.contains(&"duckduckgo"), "{names:?}");
        assert_eq!(names.len(), 3, "{names:?}");
        // No allowlist keeps the shipped behaviour.
        assert!(Settings::defaults().enabled_engines().count() > names.len());
    }

    #[test]
    fn a_user_engine_entry_can_set_its_own_proxy() {
        let settings = merged_toml(
            r#"
[[engines]]
name = "mwmbl"
engine = "mwmbl"
proxy = "http://127.0.0.1:18888"
"#,
        );
        let engine = settings.engine("mwmbl").expect("mwmbl kept");
        assert_eq!(engine.proxy.as_deref(), Some("http://127.0.0.1:18888"));
        // engines without the key stay direct
        let other = settings.engine("swisscows").expect("swisscows kept");
        assert!(other.proxy.is_none());
    }

    #[test]
    fn a_user_can_enable_an_engine_that_ships_disabled() {
        // exaapi ships disabled in the defaults
        let shipped = Settings::defaults();
        assert_eq!(
            shipped.engine("exaapi").expect("exaapi exists").disabled,
            Some(true)
        );

        let settings = merged_toml(
            r#"
[[engines]]
name = "exaapi"
engine = "exaapi"
shortcut = "exa"
categories = ["general", "web"]
disabled = false
api_key = "test-key"
"#,
        );
        let enabled = settings.engine("exaapi").expect("exaapi kept");
        assert_eq!(enabled.disabled, Some(false), "explicit false must win");
        assert_eq!(enabled.api_key.as_deref(), Some("test-key"));
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
        let merged = merged_toml(
            r#"
use_default_settings = true
[server]
port = 8888
"#,
        );
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
        let settings = merged_yaml(yaml);

        assert_eq!(settings.server.port, 8880);
        assert_eq!(settings.search.safe_search, 2);
        assert_eq!(settings.search.default_lang, "ja");
        assert!(settings.engine("wikipedia (ja)").is_some());
        assert_eq!(settings.ui.default_locale, "en");
    }

    #[test]
    fn yaml_settings_preserve_scalar_types_tags_and_aliases() {
        let settings = merged_yaml(
            r#"
general:
  instance_name: !label "RustXNG 日本語"
server:
  port: !!int "29484"
  secret_key: 000123
outgoing:
  request_timeout: !!float "1.25"
engines:
  - name: yaml fixture one
    engine: wikipedia
    base_url: &endpoint https://example.test/wiki
    categories: &categories [general, null, images]
    disabled: TRUE
    weight: &weight 0.75
    timeout: null
    negative_hex: -0x2a
    binary: 0b101
    description: |
      first line
      日本語の本文
  - name: yaml fixture two
    engine: wikipedia
    base_url: *endpoint
    categories: *categories
    weight: *weight
"#,
        );

        assert_eq!(settings.general.instance_name, "RustXNG 日本語");
        assert_eq!(settings.server.port, 29484);
        assert_eq!(settings.server.secret_key, "000123");
        assert_eq!(settings.outgoing.request_timeout, 1.25);
        let first = settings.engine("yaml fixture one").unwrap();
        let second = settings.engine("yaml fixture two").unwrap();
        assert_eq!(first.base_url.as_deref(), Some("https://example.test/wiki"));
        assert_eq!(second.base_url, first.base_url);
        assert_eq!(first.categories, ["general", "images"]);
        assert_eq!(second.categories, first.categories);
        assert_eq!(first.disabled, Some(true));
        assert_eq!(first.weight, Some(0.75));
        assert_eq!(second.weight, first.weight);
        assert_eq!(first.timeout, None);
        assert_eq!(first.opt_i64("negative_hex"), Some(-42));
        assert_eq!(first.opt_i64("binary"), Some(5));
        assert_eq!(
            first.opt_str("description"),
            Some("first line\n日本語の本文\n")
        );
    }

    #[test]
    fn yaml_mapping_keys_and_nulls_keep_their_conversion_rules() {
        let value = Settings::user_value(
            "options:\n  true: enabled\n  0x10: sixteen\n  1.5: decimal\n  omitted: null\n",
            true,
        )
        .unwrap();
        let options = value["options"].as_table().unwrap();
        assert_eq!(options["true"].as_str(), Some("enabled"));
        assert_eq!(options["16"].as_str(), Some("sixteen"));
        assert_eq!(options["1.5"].as_str(), Some("decimal"));
        assert!(!options.contains_key("omitted"));
    }

    #[test]
    fn malformed_yaml_and_unsafe_expansion_are_rejected() {
        for text in [
            "server: {port: 29483, port: 29484}",
            "server: {port: !!int invalid}",
            "server: *missing",
            "value: &recursive [*recursive]",
            "server: {port: 29483}\n---\nserver: {port: 29484}",
        ] {
            assert!(Settings::user_value(text, true).is_err(), "{text}");
        }
        let nested = format!("{}0{}", "[".repeat(129), "]".repeat(129));
        assert!(Settings::user_value(&nested, true).is_err());

        let mut aliases = String::from("a0: &a0 [payload]\n");
        for level in 1..8 {
            let repeated = vec![format!("*a{}", level - 1); 10].join(", ");
            aliases.push_str(&format!("a{level}: &a{level} [{repeated}]\n"));
        }
        assert!(Settings::user_value(&aliases, true).is_err());
    }

    #[test]
    fn user_engines_merge_by_name() {
        let settings = merged_toml(
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
        );
        let duckduckgo = settings.engine("duckduckgo").expect("duckduckgo kept");
        assert_eq!(duckduckgo.disabled, Some(true));
        assert!(settings.engine("my instance").is_some());
    }

    #[test]
    fn profiles_switch_the_provider_set() {
        let standard = Settings::load_profile(None, Profile::Standard).unwrap();
        let full = Settings::load_profile(None, Profile::Full).unwrap();
        let clean = Settings::load_profile(None, Profile::Clean).unwrap();

        let enabled = |settings: &Settings, name: &str| {
            settings.enabled_engines().any(|engine| engine.name == name)
        };
        assert!(!enabled(&standard, "exaapi"));
        assert!(!enabled(&standard, "yahoo japan"));
        assert!(enabled(&full, "yahoo japan"));
        assert!(enabled(&full, "exaapi"));
        assert!(enabled(&full, "wikipedia"));
        assert_eq!(clean.enabled_engines().count(), 0, "clean runs no engine");
        assert_eq!(clean.search.local_index_path, "index-docs");
        assert_eq!(clean.search.wikipedia_index_path, "index-wikipedia");
        assert!(!clean.search.official_sites);
        assert_eq!(Profile::parse("normal"), Ok(Profile::Standard));
        assert!(Profile::parse("everything").is_err());
    }

    #[test]
    fn a_settings_file_is_laid_over_the_profile() {
        let dir = std::env::temp_dir().join(format!("rustxng-profile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        std::fs::write(
            &path,
            "[server]\nport = 19999\n[[engines]]\nname = \"yahoo japan\"\ndisabled = true\n",
        )
        .unwrap();
        let settings = Settings::load_profile(Some(&path), Profile::Full).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(settings.server.port, 19999);
        assert_eq!(settings.general.instance_name, "RustXNG (full)");
        assert!(
            !settings
                .enabled_engines()
                .any(|engine| engine.name == "yahoo japan")
        );
        assert!(
            settings
                .enabled_engines()
                .any(|engine| engine.name == "yahoo")
        );
    }

    #[test]
    fn an_override_by_name_needs_no_engine_id() {
        let settings = merged_toml(
            r#"
            [[engines]]
            name = "yahoo"
            disabled = false
            "#,
        );
        let yahoo = settings.engine("yahoo").expect("yahoo kept");
        assert_eq!(yahoo.disabled, Some(false));
        assert_eq!(yahoo.engine, "yahoo");
        assert!(settings.validate().is_ok());

        let unknown = merged_toml(
            r#"
            [[engines]]
            name = "no such engine"
            "#,
        );
        assert!(unknown.validate().is_err());
    }

    #[test]
    fn invalid_format_is_rejected() {
        let mut settings = Settings::defaults();
        settings.search.formats.push("yaml".into());
        assert!(settings.validate().is_err());
    }

    #[test]
    fn deepseek_fetch_settings_are_parsed() {
        let settings = merged_toml(
            r#"
            [fetch]
            provider = "deepseek"
            timeout = 12.0
            [fetch.deepseek]
            base_url = "https://api.deepseek.com"
            api_key = "KEY"
            "#,
        );
        assert_eq!(settings.fetch.provider, FetchProvider::DeepSeek);
        assert_eq!(settings.fetch.timeout, 12.0);
        assert_eq!(settings.fetch.deepseek.api_key, "KEY");
    }

    #[test]
    fn fetch_timeout_must_be_finite_and_representable() {
        for (name, value) in [
            ("zero", "0.0"),
            ("negative", "-1.0"),
            ("infinity", "inf"),
            ("negative infinity", "-inf"),
            ("not a number", "nan"),
            ("out of range", "1e300"),
        ] {
            let settings = merged_toml(&format!("[fetch]\ntimeout = {value}\n"));
            assert!(
                settings.validate().is_err(),
                "fetch.timeout = {value} ({name}) must be rejected"
            );
        }
    }

    #[test]
    fn deepseek_engine_is_disabled_by_default() {
        let settings = Settings::defaults();
        let engine = settings.engine("deepseek").expect("deepseek entry");
        assert_eq!(engine.disabled, Some(true));
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
}
