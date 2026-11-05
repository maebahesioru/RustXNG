//! Search query parsing, ported from `searx/query.py`.
//!
//! Supported syntax (all of it is upstream behaviour):
//!
//! * `!engine` / `!shortcut` — search a single engine,
//! * `!category` — search every engine of a category,
//! * `!!` — "feeling lucky": redirect to the first result,
//! * `!!bang` — hand the query over to an external service,
//! * `:<lang>` — force a language (`:ja`, `:japanese`, `:japan`),
//! * `<3` — force a timeout of 3 seconds (`<850` = 850 ms),
//! * everything else is kept as search terms.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::categories;
use crate::engine::EngineRegistry;
use crate::external_bangs;
use crate::locales;

/// Time ranges understood by the `time_range` request parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeRange {
    Day,
    Week,
    Month,
    Year,
}

impl TimeRange {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "year" => Some(Self::Year),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    /// The window length in seconds (engines with epoch based filters).
    pub fn seconds(range: Option<TimeRange>) -> Option<i64> {
        range.map(|range| match range {
            Self::Day => 86_400,
            Self::Week => 604_800,
            Self::Month => 2_592_000,
            Self::Year => 31_536_000,
        })
    }

    /// The oldest instant a date based filter may return.
    pub fn since(range: Option<TimeRange>) -> Option<chrono::DateTime<chrono::Utc>> {
        Self::seconds(range).map(|seconds| chrono::Utc::now() - chrono::Duration::seconds(seconds))
    }

    /// The `relativedelta` keyword upstream uses for mojeek & friends.
    pub fn as_python_delta(self) -> (&'static str, i64) {
        match self {
            Self::Day => ("days", 1),
            Self::Week => ("weeks", 1),
            Self::Month => ("months", 1),
            Self::Year => ("years", 1),
        }
    }
}

/// A reference to an engine (optionally in the context of a category bang).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EngineRef {
    pub name: String,
    pub category: Option<String>,
}

/// Parsed raw text query.
#[derive(Debug, Clone)]
pub struct Query {
    /// The raw query as typed by the user.
    pub raw: String,
    /// Special parts (`!bang`, `:lang`, `<3`, `!!`).
    pub query_parts: Vec<String>,
    /// The search terms.
    pub user_query_parts: Vec<String>,
    /// Languages requested with `:lang` (normalised, e.g. `ja`, `en-US`).
    pub languages: Vec<String>,
    /// Timeout limit requested with `<n`, in seconds.
    pub timeout_limit: Option<f64>,
    /// External bang requested with `!!name`.
    pub external_bang: Option<String>,
    /// True when at least one `!engine` / `!category` bang was used.
    pub specific: bool,
    /// Engines selected with `!engine` bangs.
    pub engine_refs: Vec<EngineRef>,
    /// Autocompletion candidates collected while parsing.
    pub autocomplete_list: Vec<String>,
    /// `!!` was used: redirect to the first result.
    pub redirect_to_first_result: bool,
    /// Engines disabled in the user preferences.
    pub disabled_engines: Vec<String>,

    // ----- request parameters (filled by the web layer) -----
    /// UI locale, used for engine region/language selection.
    pub locale: String,
    /// 1-based page number.
    pub pageno: u32,
    /// Time range filter.
    pub time_range: Option<TimeRange>,
    /// Safe search level, 0-2.
    pub safesearch: u8,
    /// Categories selected in the UI.
    pub categories: Vec<String>,
    /// Engines selected in the UI.
    pub engines: Vec<String>,
    /// `search.max_page`.
    pub max_page: u32,
}

impl Query {
    /// Parse `raw` and return the query with default request parameters.
    pub fn parse(raw: &str, registry: &EngineRegistry, disabled_engines: &[String]) -> Query {
        let mut query = Query {
            raw: raw.to_string(),
            query_parts: Vec::new(),
            user_query_parts: Vec::new(),
            languages: Vec::new(),
            timeout_limit: None,
            external_bang: None,
            specific: false,
            engine_refs: Vec::new(),
            autocomplete_list: Vec::new(),
            redirect_to_first_result: false,
            disabled_engines: disabled_engines.to_vec(),
            locale: String::new(),
            pageno: 1,
            time_range: None,
            safesearch: 0,
            categories: Vec::new(),
            engines: Vec::new(),
            max_page: 0,
        };
        query.parse_query(raw, registry);
        query
    }

    fn parse_query(&mut self, raw: &str, registry: &EngineRegistry) {
        // Split on whitespace but keep empty parts out: upstream splits with a
        // capturing group to preserve the original spacing, which we only need
        // for `full_query`.
        let parts: Vec<&str> = raw.split_whitespace().collect();
        let last_index = parts.len().saturating_sub(1);

        for (index, part) in parts.iter().enumerate() {
            let enable_autocomplete = index == last_index;
            let special = self.parse_part(part, enable_autocomplete, registry);
            if special {
                self.query_parts.push((*part).to_string());
            } else {
                self.user_query_parts.push((*part).to_string());
            }
        }
    }

    fn parse_part(&mut self, part: &str, autocomplete: bool, registry: &EngineRegistry) -> bool {
        if part == "!!" {
            self.redirect_to_first_result = true;
            return true;
        }
        if let Some(rest) = part.strip_prefix("!!") {
            if !rest.is_empty() {
                let value = rest.to_lowercase();
                if external_bangs::lookup(&value).is_some() {
                    self.external_bang = Some(value);
                    return true;
                }
                if autocomplete {
                    for bang in external_bangs::autocomplete(&value) {
                        self.add_autocomplete(format!("!!{bang}"));
                    }
                }
            }
            return false;
        }
        if let Some(rest) = part.strip_prefix('<') {
            if self.parse_timeout(rest) {
                return true;
            }
            if autocomplete && rest.is_empty() {
                self.add_autocomplete("<3".to_string());
                self.add_autocomplete("<850".to_string());
            }
            return false;
        }
        if let Some(rest) = part.strip_prefix(':') {
            let value = rest.to_lowercase().replace('_', "-");
            if self.parse_language(&value) {
                return true;
            }
            if autocomplete {
                self.autocomplete_languages(&value);
            }
            return false;
        }
        if let Some(rest) = part.strip_prefix('!') {
            let value = rest.replace(['-', '_'], " ").to_lowercase();
            if self.parse_bang(&value, registry) {
                self.specific = true;
                return true;
            }
            if autocomplete {
                self.autocomplete_bangs(&value, registry);
            }
        }
        false
    }

    fn parse_timeout(&mut self, value: &str) -> bool {
        if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        let raw: u64 = match value.parse() {
            Ok(raw) => raw,
            Err(_) => return false,
        };
        // below 100 the unit is seconds, 100 and above it is milliseconds
        self.timeout_limit = Some(if raw < 100 {
            raw as f64
        } else {
            raw as f64 / 1000.0
        });
        true
    }

    fn parse_bang(&mut self, value: &str, registry: &EngineRegistry) -> bool {
        if value.is_empty() {
            return false;
        }
        // !engine (name or shortcut)
        if let Some(engine) = registry.get(value).or_else(|| registry.by_shortcut(value)) {
            self.engine_refs.push(EngineRef {
                name: engine.name().to_string(),
                category: None,
            });
            return true;
        }
        if matches!(value, "local index" | "wikipedia index") {
            self.engine_refs.push(EngineRef {
                name: value.replace(' ', "_"),
                category: None,
            });
            return true;
        }
        // !category
        let category = categories::normalize(value);
        if registry.has_category(&category) {
            for name in registry.engines_in_category(&category) {
                if !self.disabled_engines.contains(&name) {
                    self.engine_refs.push(EngineRef {
                        name,
                        category: Some(category.clone()),
                    });
                }
            }
            return true;
        }
        false
    }

    fn autocomplete_bangs(&mut self, value: &str, registry: &EngineRegistry) {
        if value.is_empty() {
            for suggestion in ["images", "wikipedia", "osm"] {
                if !self.disabled_engines.iter().any(|d| d == suggestion) {
                    self.add_autocomplete(format!("!{suggestion}"));
                }
            }
            return;
        }
        for category in registry.categories() {
            if category.starts_with(value) {
                self.add_autocomplete(format!("!{}", category.replace(' ', "_")));
            }
        }
        for engine in registry.names() {
            if engine.to_lowercase().starts_with(value) {
                self.add_autocomplete(format!("!{}", engine.replace(' ', "_")));
            }
        }
        for shortcut in registry.shortcuts() {
            if shortcut.starts_with(value) {
                self.add_autocomplete(format!("!{shortcut}"));
            }
        }
    }

    fn parse_language(&mut self, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        let mut found = false;
        for locale in locales::LOCALES {
            let id = locale.id.to_lowercase();
            let english = locale.english.to_lowercase();
            let native = locale.native.to_lowercase();
            let country = locale.country.to_lowercase();
            let matches = value == id
                || value == english
                || value == native
                || (!country.is_empty() && value.replace('-', " ") == country);
            if matches && !self.languages.contains(&normalise_locale(&id)) {
                self.languages.push(normalise_locale(&id));
                found = true;
                if value == id {
                    break;
                }
            }
        }
        // A valid, yet not selectable language code (upstream allows it too).
        if is_language_code(value) || value == "auto" {
            let normalised = if value == "auto" {
                "auto".to_string()
            } else {
                normalise_locale(value)
            };
            if !self.languages.contains(&normalised) {
                self.languages.push(normalised);
                found = true;
            }
        }
        found
    }

    fn autocomplete_languages(&mut self, value: &str) {
        if value.is_empty() {
            for tag in [":en", ":en_us", ":english", ":united_kingdom"] {
                self.add_autocomplete(tag.to_string());
            }
            return;
        }
        for locale in locales::LOCALES.iter().take(2000) {
            let id = locale.id.to_lowercase();
            let english = locale.english.to_lowercase();
            if id.starts_with(value) {
                let suggestion = if value.len() <= 2 {
                    id.split('-').next().unwrap_or(&id).to_string()
                } else {
                    id.clone()
                };
                self.add_autocomplete(format!(":{suggestion}"));
            }
            if english.starts_with(value) {
                self.add_autocomplete(format!(":{}", english.replace('-', "_")));
            }
            let country = locale.country.to_lowercase();
            if !country.is_empty() && country.starts_with(&value.replace('-', " ")) {
                self.add_autocomplete(format!(":{}", country.replace(' ', "_")));
            }
        }
    }

    fn add_autocomplete(&mut self, value: String) {
        if !self.autocomplete_list.contains(&value) {
            self.autocomplete_list.push(value);
        }
    }

    /// The search terms, without the special parts.
    pub fn query_text(&self) -> String {
        self.user_query_parts.join(" ")
    }

    /// The full query, special parts first (upstream `getFullQuery`).
    pub fn full_query(&self) -> String {
        format!("{} {}", self.query_parts.join(" "), self.query_text())
            .trim()
            .to_string()
    }

    /// A copy of this query with different search terms, keeping the bangs.
    pub fn with_query(&self, text: &str) -> Query {
        let mut clone = self.clone();
        clone.user_query_parts = text.split_whitespace().map(str::to_string).collect();
        clone.raw = clone.full_query();
        clone.autocomplete_list.clear();
        clone
    }

    /// Engines referenced by `!engine` bangs.
    pub fn engine_ref_names(&self) -> Vec<String> {
        self.engine_refs
            .iter()
            .map(|reference| reference.name.clone())
            .collect()
    }

    /// The language sent to the engines: the first requested language, or the
    /// UI locale when nothing was requested.
    pub fn engine_language(&self) -> String {
        // "all" asks for no language filter; where one language is needed
        // (which Wikipedia to ask, a translation target) it behaves like "auto".
        let automatic = self
            .languages
            .first()
            .is_some_and(|language| language == "auto" || language == "all");
        if let Some(language) = self
            .languages
            .first()
            .filter(|language| language.as_str() != "auto" && language.as_str() != "all")
        {
            return language.clone();
        }
        if !automatic && !self.locale.is_empty() {
            return self.locale.clone();
        }
        // Neither the UI nor the request chose a language: follow the query.
        // Japanese engines answer Japanese queries far better than "all" does.
        if self
            .query_text()
            .chars()
            .any(|c| matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff))
        {
            "ja".to_string()
        } else {
            "en".to_string()
        }
    }

    /// With no language chosen ("auto"), a Japanese query searches in
    /// Japanese even when the locale only came from the UI language: the UI
    /// locale says how to show the page, not what the query is written in.
    /// `explicit_locale` is true when the request itself named a locale.
    pub fn follow_query_script(&mut self, explicit_locale: bool) {
        if explicit_locale || !self.languages.iter().all(|language| language == "auto") {
            return;
        }
        let ui_locale = std::mem::take(&mut self.locale);
        let detected = self.engine_language();
        self.locale = if detected == "ja" {
            detected
        } else {
            ui_locale
        };
    }

    /// Apply the `language` request parameter, following upstream's
    /// `parse_lang` (`searx/webadapter.py`): a `:lang` bang in the query
    /// wins over the parameter, and a value that is neither a language
    /// code nor `auto` is an error (upstream answers 400).
    pub fn apply_language_param(&mut self, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() || !self.languages.is_empty() {
            return Ok(());
        }
        match language_code_value(value) {
            Some(language) => {
                self.languages.push(language);
                Ok(())
            }
            None => Err(format!("invalid language code: {value}")),
        }
    }

    /// Fill the language from the preferences when neither a bang nor the
    /// `language` parameter chose one (upstream's last resort in
    /// `parse_lang`). Invalid values and `auto` are ignored: `auto` is
    /// the seeded default, and honouring it here would suppress the
    /// locale / query-language fallback in [`Query::engine_language`].
    pub fn apply_language_preference(&mut self, value: &str) {
        if !self.languages.is_empty() {
            return;
        }
        if let Some(language) = language_code_value(value) {
            if language != "auto" && language != "all" {
                self.languages.push(language);
            }
        }
    }

    /// The categories used for this search (UI selection or bang categories).
    pub fn selected_categories(&self) -> Vec<String> {
        if !self.categories.is_empty() {
            return self.categories.clone();
        }
        let bang_categories: BTreeSet<String> = self
            .engine_refs
            .iter()
            .filter_map(|reference| reference.category.clone())
            .collect();
        if !bang_categories.is_empty() {
            return bang_categories.into_iter().collect();
        }
        vec![categories::GENERAL.to_string()]
    }
}

/// A request language value normalised like a `:lang` code, or `None`
/// when the value is not a language code (and not `auto` or `all`; upstream
/// accepts `all` as "every language").
fn language_code_value(value: &str) -> Option<String> {
    let lowered = value.trim().to_lowercase().replace('_', "-");
    if lowered == "auto" || lowered == "all" {
        return Some(lowered);
    }
    let normalized = normalise_locale(&lowered);
    if locales::LOCALES
        .iter()
        .any(|locale| locale.id.eq_ignore_ascii_case(&normalized))
    {
        return Some(normalized);
    }
    None
}

fn normalise_locale(value: &str) -> String {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() == 2 {
        format!("{}-{}", parts[0].to_lowercase(), parts[1].to_uppercase())
    } else {
        value.to_lowercase()
    }
}

fn is_language_code(value: &str) -> bool {
    let mut parts = value.split('-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(language), None, None) => {
            (2..=3).contains(&language.len()) && language.chars().all(|c| c.is_ascii_alphabetic())
        }
        (Some(language), Some(region), None) => {
            (2..=3).contains(&language.len())
                && language.chars().all(|c| c.is_ascii_alphabetic())
                && (2..=4).contains(&region.len())
                && region.chars().all(|c| c.is_ascii_alphabetic())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn local_index_bangs_are_explicit_sources_without_remote_engines() {
        for source in ["local_index", "wikipedia_index"] {
            let query = super::Query::parse(
                &format!("!{source} rust ownership"),
                &crate::engine::EngineRegistry::default(),
                &[],
            );
            assert!(query.specific);
            assert_eq!(query.engine_ref_names(), [source]);
            assert_eq!(query.query_text(), "rust ownership");
        }
    }

    #[test]
    fn a_japanese_query_searches_in_japanese_unless_a_locale_was_asked_for() {
        let mut query = Query::parse("気象庁 天気予報", &registry(), &[]);
        query.locale = "en".into();
        query.follow_query_script(false);
        assert_eq!(query.locale, "ja");

        let mut query = Query::parse("weather forecast", &registry(), &[]);
        query.locale = "en".into();
        query.follow_query_script(false);
        assert_eq!(query.locale, "en");

        let mut query = Query::parse("気象庁", &registry(), &[]);
        query.locale = "en-US".into();
        query.follow_query_script(true);
        assert_eq!(query.locale, "en-US");
    }

    use super::*;
    use crate::engine::EngineRegistry;
    use crate::settings::Settings;

    fn registry() -> EngineRegistry {
        EngineRegistry::from_settings(&Settings::defaults()).expect("default registry")
    }

    #[test]
    fn plain_query_has_no_special_parts() {
        let query = Query::parse("rust async runtime", &registry(), &[]);
        assert_eq!(query.query_text(), "rust async runtime");
        assert!(query.query_parts.is_empty());
        assert!(!query.specific);
        assert_eq!(query.full_query(), "rust async runtime");
    }

    #[test]
    fn engine_bang_selects_engine() {
        let query = Query::parse("!wikipedia rust", &registry(), &[]);
        assert_eq!(query.query_text(), "rust");
        assert_eq!(query.engine_ref_names(), vec!["wikipedia".to_string()]);
        assert!(query.specific);
    }

    #[test]
    fn shortcut_bang_selects_engine() {
        let query = Query::parse("!wp async rust", &registry(), &[]);
        assert_eq!(query.engine_ref_names(), vec!["wikipedia".to_string()]);
    }

    #[test]
    fn category_bang_expands_to_engines() {
        let query = Query::parse("!images cats", &registry(), &[]);
        assert!(query.engine_refs.len() > 1);
        assert!(
            query
                .engine_refs
                .iter()
                .all(|reference| reference.category.as_deref() == Some("images"))
        );
        assert_eq!(query.selected_categories(), vec!["images".to_string()]);
    }

    #[test]
    fn disabled_engines_are_skipped_in_category_bangs() {
        let disabled = vec!["bing images".to_string()];
        let query = Query::parse("!images cats", &registry(), &disabled);
        assert!(
            !query
                .engine_ref_names()
                .contains(&"bing images".to_string())
        );
    }

    #[test]
    fn language_bang_accepts_code_and_name() {
        let query = Query::parse(":ja 猫", &registry(), &[]);
        assert_eq!(query.languages, vec!["ja".to_string()]);
        // A language name matches the language and its region variant, exactly
        // like upstream's LanguageParser.
        let query = Query::parse(":japanese 猫", &registry(), &[]);
        assert_eq!(query.languages[0], "ja");
        assert!(query.languages.contains(&"ja-JP".to_string()));
        let query = Query::parse(":en-us test", &registry(), &[]);
        assert_eq!(query.languages, vec!["en-US".to_string()]);
    }

    #[test]
    fn language_parameter_sets_the_engine_language() {
        let mut query = Query::parse("rust async", &registry(), &[]);
        query.apply_language_param("fr").expect("valid code");
        assert_eq!(query.languages, vec!["fr".to_string()]);
        assert_eq!(query.engine_language(), "fr");
    }

    #[test]
    fn language_all_means_every_language() {
        let mut query = Query::parse("rust", &EngineRegistry::default(), &[]);
        assert!(query.apply_language_param("all").is_ok());
        assert_eq!(query.languages, vec!["all".to_string()]);
        // Where one language is needed, it follows the query like "auto".
        assert_eq!(query.engine_language(), "en");
        let mut japanese = Query::parse("富士山", &EngineRegistry::default(), &[]);
        japanese.apply_language_param("ALL").unwrap();
        assert_eq!(japanese.engine_language(), "ja");
        assert!(query.apply_language_param("nonsense").is_ok()); // already chosen
        let mut fresh = Query::parse("rust", &EngineRegistry::default(), &[]);
        assert!(fresh.apply_language_param("nonsense").is_err());
    }

    #[test]
    fn language_parameter_is_normalised() {
        let mut query = Query::parse("rust", &registry(), &[]);
        query.apply_language_param("en-us").expect("valid code");
        assert_eq!(query.languages, vec!["en-US".to_string()]);
        let mut query = Query::parse("rust", &registry(), &[]);
        query.apply_language_param(" auto ").expect("auto is valid");
        assert_eq!(query.languages, vec!["auto".to_string()]);
    }

    #[test]
    fn auto_language_keeps_query_detection_and_overrides_preferences() {
        let mut query = Query::parse("日本語のエラー", &registry(), &[]);
        query.locale = "en".into();
        query.apply_language_param("auto").unwrap();
        query.apply_language_preference("fr");
        assert_eq!(query.engine_language(), "ja");
        let mut query = Query::parse(":auto 日本語", &registry(), &[]);
        query.locale = "en".into();
        assert_eq!(query.engine_language(), "ja");
    }

    #[test]
    fn language_bang_wins_over_the_parameter() {
        let mut query = Query::parse(":ja 猫", &registry(), &[]);
        query
            .apply_language_param("fr")
            .expect("bang wins, no error");
        assert_eq!(query.languages, vec!["ja".to_string()]);
    }

    #[test]
    fn invalid_language_parameter_is_rejected() {
        let mut query = Query::parse("rust", &registry(), &[]);
        assert!(query.apply_language_param("not-a-language").is_err());
        assert!(query.apply_language_param("xyz").is_err());
        assert!(query.apply_language_param("zz-ZZ").is_err());
        assert!(query.languages.is_empty());
        // An empty parameter counts as absent.
        query.apply_language_param("").expect("empty is ignored");
        assert!(query.languages.is_empty());
    }

    #[test]
    fn language_preference_is_the_last_resort() {
        let mut query = Query::parse("rust", &registry(), &[]);
        query.apply_language_preference("ja");
        assert_eq!(query.languages, vec!["ja".to_string()]);
        // A bang already chose: the preference changes nothing.
        let mut query = Query::parse(":fr chat", &registry(), &[]);
        query.apply_language_preference("ja");
        assert_eq!(query.languages, vec!["fr".to_string()]);
        // `auto` and junk never enter the language list.
        let mut query = Query::parse("rust", &registry(), &[]);
        query.apply_language_preference("auto");
        query.apply_language_preference("not-a-language");
        assert!(query.languages.is_empty());
    }

    #[test]
    fn time_ranges_are_convertible() {
        assert_eq!(TimeRange::seconds(Some(TimeRange::Day)), Some(86_400));
        assert_eq!(TimeRange::seconds(None), None);
        let since = TimeRange::since(Some(TimeRange::Week)).expect("a date");
        assert!(since < chrono::Utc::now());
        assert!((chrono::Utc::now() - since).num_days() >= 6);
    }

    #[test]
    fn timeout_bang_uses_seconds_below_100() {
        let query = Query::parse("rust <3", &registry(), &[]);
        assert_eq!(query.timeout_limit, Some(3.0));
        let query = Query::parse("rust <850", &registry(), &[]);
        assert_eq!(query.timeout_limit, Some(0.85));
    }

    #[test]
    fn double_bang_redirects_to_first_result() {
        let query = Query::parse("!! rust", &registry(), &[]);
        assert!(query.redirect_to_first_result);
    }

    #[test]
    fn external_bang_is_recognised() {
        let query = Query::parse("!!w cats", &registry(), &[]);
        assert_eq!(query.external_bang.as_deref(), Some("w"));
    }

    #[test]
    fn autocomplete_lists_bangs_and_languages() {
        let query = Query::parse("!wikip", &registry(), &[]);
        assert!(query.autocomplete_list.contains(&"!wikipedia".to_string()));
        // ":jap" is a valid language code, so it is parsed instead of being
        // autocompleted; ":engl" is not.
        let query = Query::parse(":engl", &registry(), &[]);
        assert!(query.autocomplete_list.contains(&":english".to_string()));
    }

    #[test]
    fn unknown_bang_stays_a_search_term() {
        let query = Query::parse("!nosuchengine rust", &registry(), &[]);
        assert!(query.engine_refs.is_empty());
        assert_eq!(query.query_text(), "!nosuchengine rust");
    }
}
