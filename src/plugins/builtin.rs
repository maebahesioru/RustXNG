//! Bundled plugins. Each one is a small, self contained port of the upstream
//! plugin with the same id.

use chrono::{FixedOffset, Utc};

use super::{Plugin, PluginContext};
use crate::aggregator::SearchResponse;
use crate::query::Query;
use crate::results::SearchResult;

/// Every bundled plugin, in the order they run.
pub fn all() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(Hostnames),
        Box::new(TrackerUrlRemover),
        Box::new(SelfInfo),
        Box::new(TimeZone),
        Box::new(Calculator),
        Box::new(HashPlugin),
        Box::new(OaDoiRewrite),
        Box::new(super::ahmia_filter::AhmiaFilter::new()),
        Box::new(super::infinite_scroll::InfiniteScroll),
        Box::new(super::tor_check::TorCheck),
        Box::new(super::unit_converter::UnitConverter),
    ]
}

/// Adds the hostname of every result (upstream `hostnames`).
struct Hostnames;

impl Plugin for Hostnames {
    fn id(&self) -> &'static str {
        "hostnames"
    }
    fn name(&self) -> &'static str {
        "Hostnames"
    }
    fn description(&self) -> &'static str {
        "Show the hostname and merge results of the same website."
    }
    fn on_result(&self, result: &mut SearchResult, _context: &PluginContext) {
        result.pretty_url = Some(crate::results::pretty_url(&result.url));
        let hostname = url::Url::parse(&result.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default();
        if !hostname.is_empty() {
            result
                .metadata
                .entry("hostname".to_string())
                .or_insert(hostname);
        }
    }
}

/// Removes tracking parameters from result URLs (upstream `tracker_url_remover`).
struct TrackerUrlRemover;

impl Plugin for TrackerUrlRemover {
    fn id(&self) -> &'static str {
        "tracker_url_remover"
    }
    fn name(&self) -> &'static str {
        "Tracker URL remover"
    }
    fn description(&self) -> &'static str {
        "Remove trackers from result URLs."
    }
    fn on_result(&self, result: &mut SearchResult, _context: &PluginContext) {
        let Ok(mut parsed) = url::Url::parse(&result.url) else {
            return;
        };
        let kept: Vec<(String, String)> = parsed
            .query_pairs()
            .filter(|(key, _)| !crate::results::is_tracking_param(key))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        if kept.len() == parsed.query_pairs().count() {
            return;
        }
        {
            let mut pairs = parsed.query_pairs_mut();
            pairs.clear();
            for (key, value) in &kept {
                pairs.append_pair(key, value);
            }
        }
        if kept.is_empty() {
            parsed.set_query(None);
        }
        result.url = parsed.to_string();
    }
}

/// Reports the IP address and user agent (upstream `self_info`).
struct SelfInfo;

impl Plugin for SelfInfo {
    fn id(&self) -> &'static str {
        "self_info"
    }
    fn name(&self) -> &'static str {
        "Self Information"
    }
    fn description(&self) -> &'static str {
        "Display your IP address and user agent."
    }
    fn default_enabled(&self) -> bool {
        false
    }
    fn post_search(&self, response: &mut SearchResponse, context: &PluginContext) {
        if context.user.ip.is_empty() {
            return;
        }
        response
            .answers
            .push(format!("Your IP address is {}", context.user.ip));
        if !context.user.user_agent.is_empty() {
            response
                .answers
                .push(format!("Your user agent is {}", context.user.user_agent));
        }
    }
}

/// Shows the current time (upstream `time_zone`, simplified: fixed offsets
/// instead of the upstream geo location service).
struct TimeZone;

/// Query keywords that trigger the plugin.
const TIME_KEYWORDS: &[&str] = &["time", "timezone", "timezones", "now", "clock"];

/// City or timezone name -> UTC offset in minutes.
const TIME_ZONES: &[(&str, i32)] = &[
    ("utc", 0),
    ("gmt", 0),
    ("london", 0),
    ("lisbon", 0),
    ("berlin", 60),
    ("paris", 60),
    ("madrid", 60),
    ("rome", 60),
    ("amsterdam", 60),
    ("stockholm", 60),
    ("warsaw", 60),
    ("cairo", 120),
    ("athens", 120),
    ("helsinki", 120),
    ("moscow", 180),
    ("dubai", 240),
    ("karachi", 300),
    ("delhi", 330),
    ("mumbai", 330),
    ("kolkata", 330),
    ("dhaka", 360),
    ("bangkok", 420),
    ("jakarta", 420),
    ("hanoi", 420),
    ("singapore", 480),
    ("beijing", 480),
    ("shanghai", 480),
    ("hongkong", 480),
    ("perth", 480),
    ("tokyo", 540),
    ("seoul", 540),
    ("osaka", 540),
    ("jst", 540),
    ("kst", 540),
    ("sydney", 600),
    ("melbourne", 600),
    ("brisbane", 600),
    ("auckland", 720),
    ("honolulu", -600),
    ("anchorage", -540),
    ("losangeles", -480),
    ("pst", -480),
    ("vancouver", -480),
    ("denver", -420),
    ("mst", -420),
    ("chicago", -360),
    ("cst", -360),
    ("newyork", -300),
    ("est", -300),
    ("toronto", -300),
    ("saopaulo", -180),
    ("buenosaires", -180),
    ("reykjavik", 0),
];

impl Plugin for TimeZone {
    fn id(&self) -> &'static str {
        "time_zone"
    }
    fn name(&self) -> &'static str {
        "Timezones plugin"
    }
    fn description(&self) -> &'static str {
        "Display the current time on different time zones."
    }
    fn pre_search(&self, _query: &mut Query, _context: &PluginContext) {}
    fn post_search(&self, response: &mut SearchResponse, context: &PluginContext) {
        if response.results.is_empty() && context.search_query.is_empty() {
            return;
        }
        let lowered = context.search_query.to_lowercase();
        let words: Vec<&str> = lowered.split_whitespace().collect();
        if !words.iter().any(|word| TIME_KEYWORDS.contains(word)) {
            return;
        }
        let place: String = words
            .iter()
            .filter(|word| !TIME_KEYWORDS.contains(*word) && **word != "in" && **word != "at")
            .copied()
            .collect::<Vec<_>>()
            .join("");
        let offset_minutes = TIME_ZONES
            .iter()
            .find(|(name, _)| *name == place)
            .map(|(_, offset)| *offset)
            .unwrap_or(0);
        let Some(offset) = FixedOffset::east_opt(offset_minutes * 60) else {
            return;
        };
        let now = Utc::now().with_timezone(&offset);
        let label = if place.is_empty() {
            "UTC".to_string()
        } else {
            place
        };
        response.answers.push(format!(
            "It is {} (UTC{:+03}:{:02}) in {}",
            now.format("%Y-%m-%d %H:%M:%S"),
            offset_minutes / 60,
            (offset_minutes % 60).abs(),
            label
        ));
    }
}

/// Solves simple mathematical expressions (upstream `calculator`).
struct Calculator;

impl Plugin for Calculator {
    fn id(&self) -> &'static str {
        "calculator"
    }
    fn name(&self) -> &'static str {
        "Calculator"
    }
    fn description(&self) -> &'static str {
        "Parse and solve mathematical expressions."
    }
    fn post_search(&self, response: &mut SearchResponse, context: &PluginContext) {
        let query = context.search_query.trim().trim_end_matches('=').trim();
        if query.len() < 2 || !query.chars().any(|c| c.is_ascii_digit()) {
            return;
        }
        if !query
            .chars()
            .all(|c| c.is_ascii_digit() || "+-*/^%()., sqrtabcosinlg ".contains(c))
        {
            return;
        }
        let Some(value) = evaluate(query) else {
            return;
        };
        let rendered = if value.fract() == 0.0 && value.abs() < 1e15 {
            format!("{}", value as i64)
        } else {
            format!("{value}")
        };
        response.answers.push(format!("{query} = {rendered}"));
    }
}

/// Maximum characters accepted in a calculator expression.
const MAX_EXPRESSION_CHARS: usize = 4_096;

/// Maximum lexical tokens accepted in one calculator expression.
const MAX_TOKENS: usize = 512;

/// Maximum recursive parser entries. One parenthesis nesting costs three
/// entries (expression, power, unary).
const MAX_PARSE_DEPTH: usize = 128;

/// Counts the lexical tokens of an expression without whitespace. The lexer
/// must match the one in `Parser::atom`.
fn count_tokens(input: &[char]) -> usize {
    let mut tokens = 0;
    let mut index = 0;
    while index < input.len() {
        let character = input[index];
        index += 1;
        if character.is_ascii_digit() || character == '.' {
            while index < input.len() && (input[index].is_ascii_digit() || input[index] == '.') {
                index += 1;
            }
        } else if character.is_ascii_alphabetic() {
            while index < input.len() && input[index].is_ascii_alphabetic() {
                index += 1;
            }
        }
        tokens += 1;
    }
    tokens
}

/// Evaluate a mathematical expression (\`+ - * / % ^\` and parentheses).
pub fn evaluate(expression: &str) -> Option<f64> {
    let mut input = Vec::with_capacity(expression.len().min(MAX_EXPRESSION_CHARS));
    for character in expression.chars().filter(|c| !c.is_whitespace()) {
        if input.len() == MAX_EXPRESSION_CHARS {
            return None;
        }
        input.push(character);
    }
    if count_tokens(&input) > MAX_TOKENS {
        return None;
    }
    let mut parser = Parser {
        input,
        position: 0,
        depth: 0,
    };
    let value = parser.expression()?;
    if parser.position != parser.input.len() || !value.is_finite() {
        return None;
    }
    Some(value)
}

struct Parser {
    input: Vec<char>,
    position: usize,
    /// Current recursion depth. A failed parse abandons the parser, so it is only unwound on success.
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.input.get(self.position).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek();
        if character.is_some() {
            self.position += 1;
        }
        character
    }

    /// Enters one recursion level, or fails once the depth budget is spent.
    fn enter(&mut self) -> Option<()> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            return None;
        }
        Some(())
    }

    fn expression(&mut self) -> Option<f64> {
        self.enter()?;
        let mut value = self.term()?;
        while let Some(operator) = self.peek() {
            match operator {
                '+' => {
                    self.bump();
                    value += self.term()?;
                }
                '-' => {
                    self.bump();
                    value -= self.term()?;
                }
                _ => break,
            }
        }
        self.depth -= 1;
        Some(value)
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.power()?;
        while let Some(operator) = self.peek() {
            match operator {
                '*' => {
                    self.bump();
                    value *= self.power()?;
                }
                '/' => {
                    self.bump();
                    value /= self.power()?;
                }
                '%' => {
                    self.bump();
                    value %= self.power()?;
                }
                _ => break,
            }
        }
        Some(value)
    }

    fn power(&mut self) -> Option<f64> {
        self.enter()?;
        let base = self.unary()?;
        let value = if self.peek() == Some('^') {
            self.bump();
            base.powf(self.power()?)
        } else {
            base
        };
        self.depth -= 1;
        Some(value)
    }

    fn unary(&mut self) -> Option<f64> {
        self.enter()?;
        let value = match self.peek() {
            Some('-') => {
                self.bump();
                -self.unary()?
            }
            Some('+') => {
                self.bump();
                self.unary()?
            }
            _ => self.atom()?,
        };
        self.depth -= 1;
        Some(value)
    }

    fn atom(&mut self) -> Option<f64> {
        match self.bump()? {
            '(' => {
                let value = self.expression()?;
                if self.bump() != Some(')') {
                    return None;
                }
                Some(value)
            }
            character if character.is_ascii_digit() || character == '.' => {
                let mut number = String::from(character);
                while let Some(next) = self.peek() {
                    if next.is_ascii_digit() || next == '.' {
                        number.push(next);
                        self.bump();
                    } else {
                        break;
                    }
                }
                number.parse().ok()
            }
            character if character.is_ascii_alphabetic() => {
                let mut name = String::from(character);
                while let Some(next) = self.peek() {
                    if next.is_ascii_alphabetic() {
                        name.push(next);
                        self.bump();
                    } else {
                        break;
                    }
                }
                let argument = self.unary()?;
                match name.as_str() {
                    "sqrt" => Some(argument.sqrt()),
                    "abs" => Some(argument.abs()),
                    "sin" => Some(argument.sin()),
                    "cos" => Some(argument.cos()),
                    "tan" => Some(argument.tan()),
                    "log" => Some(argument.log10()),
                    "ln" => Some(argument.ln()),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// Identifies common hashes (upstream `hash_plugin`).
struct HashPlugin;

impl Plugin for HashPlugin {
    fn id(&self) -> &'static str {
        "hash_plugin"
    }
    fn name(&self) -> &'static str {
        "Hash plugin"
    }
    fn description(&self) -> &'static str {
        "Identify hash types."
    }
    fn post_search(&self, response: &mut SearchResponse, context: &PluginContext) {
        let query = context.search_query.trim();
        if !query.chars().all(|c| c.is_ascii_hexdigit()) {
            return;
        }
        let kind = match query.len() {
            8 => "CRC-32",
            32 => "MD5",
            40 => "SHA-1",
            64 => "SHA-256",
            96 => "SHA-384",
            128 => "SHA-512",
            _ => return,
        };
        response.answers.push(format!("Hash type: {kind}"));
    }
}

/// Rewrites DOI links to their open access variant (upstream `oa_doi_rewrite`).
struct OaDoiRewrite;

impl Plugin for OaDoiRewrite {
    fn id(&self) -> &'static str {
        "oa_doi_rewrite"
    }
    fn name(&self) -> &'static str {
        "Open Access DOI rewrite"
    }
    fn description(&self) -> &'static str {
        "Try to find an open access version of a DOI link."
    }
    fn default_enabled(&self) -> bool {
        false
    }
    fn on_result(&self, result: &mut SearchResult, _context: &PluginContext) {
        if let Some(rest) = result
            .url
            .strip_prefix("https://doi.org/")
            .or_else(|| result.url.strip_prefix("http://doi.org/"))
            .or_else(|| result.url.strip_prefix("https://dx.doi.org/"))
            .or_else(|| result.url.strip_prefix("http://dx.doi.org/"))
        {
            result.url = format!("https://oadoi.org/{rest}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::SearchResponse;
    use crate::engine::EngineRegistry;
    use crate::query::Query;
    use crate::settings::Settings;

    fn context(query: &str) -> PluginContext {
        PluginContext {
            search_query: query.to_string(),
            base_url: "http://localhost:8888".into(),
            ..Default::default()
        }
    }

    fn empty_response(query: &str) -> SearchResponse {
        let registry = EngineRegistry::from_settings(&Settings::defaults()).unwrap();
        SearchResponse::new(&Query::parse(query, &registry, &[]))
    }

    #[test]
    fn calculator_solves_expressions() {
        assert_eq!(evaluate("1 + 2 * 3"), Some(7.0));
        assert_eq!(evaluate("(1 + 2) * 3"), Some(9.0));
        assert_eq!(evaluate("2 ^ 10"), Some(1024.0));
        assert_eq!(evaluate("sqrt(16) + 1"), Some(5.0));
        assert!(evaluate("1 + ").is_none());
        assert!(evaluate("hello").is_none());
    }

    #[test]
    fn calculator_rejects_deeply_nested_expressions() {
        let expression = format!("{}1{}", "(".repeat(1000), ")".repeat(1000));
        assert!(evaluate(&expression).is_none());
    }

    #[test]
    fn calculator_accepts_nesting_within_the_depth_budget() {
        let expression = format!("{}1{}", "(".repeat(41), ")".repeat(41));
        assert_eq!(evaluate(&expression), Some(1.0));
    }

    #[test]
    fn calculator_rejects_nesting_over_the_depth_budget() {
        let expression = format!("{}1{}", "(".repeat(42), ")".repeat(42));
        assert!(evaluate(&expression).is_none());
    }

    #[test]
    fn calculator_rejects_overlong_expressions() {
        let expression = format!("{}1", "0".repeat(4_096));
        assert!(evaluate(&expression).is_none());
    }

    #[test]
    fn calculator_rejects_expressions_over_the_token_budget() {
        let expression = format!("{}1", "1+".repeat(256));
        assert!(evaluate(&expression).is_none());
    }

    #[test]
    fn calculator_accepts_expressions_within_the_budget() {
        let expression = format!("{}1", "1+".repeat(100));
        assert_eq!(evaluate(&expression), Some(101.0));
    }

    #[test]
    fn calculator_skips_queries_over_the_budget() {
        let query = format!("{}1{}", "(".repeat(1000), ")".repeat(1000));
        let mut response = empty_response(&query);
        Calculator.post_search(&mut response, &context(&query));
        assert!(response.answers.is_empty());
    }

    #[test]
    fn calculator_answers_arithmetic_queries() {
        let mut response = empty_response("2+2*2");
        Calculator.post_search(&mut response, &context("2+2*2"));
        assert_eq!(response.answers, vec!["2+2*2 = 6".to_string()]);
    }

    #[test]
    fn hostnames_adds_metadata() {
        let mut result = SearchResult::new("https://example.com/a", "a");
        Hostnames.on_result(&mut result, &context("x"));
        assert_eq!(
            result.metadata.get("hostname").map(String::as_str),
            Some("example.com")
        );
        assert_eq!(result.pretty_url.as_deref(), Some("example.com/a"));
    }

    #[test]
    fn tracker_parameters_are_removed() {
        let mut result = SearchResult::new("https://example.com/a?utm_source=x&keep=1", "a");
        TrackerUrlRemover.on_result(&mut result, &context("x"));
        assert_eq!(result.url, "https://example.com/a?keep=1");
    }

    #[test]
    fn hashes_are_identified() {
        let mut response = empty_response("d41d8cd98f00b204e9800998ecf8427e");
        HashPlugin.post_search(&mut response, &context("d41d8cd98f00b204e9800998ecf8427e"));
        assert_eq!(response.answers, vec!["Hash type: MD5".to_string()]);
    }

    #[test]
    fn doi_links_are_rewritten() {
        let mut result = SearchResult::new("https://doi.org/10.1000/182", "paper");
        OaDoiRewrite.on_result(&mut result, &context("x"));
        assert_eq!(result.url, "https://oadoi.org/10.1000/182");
    }
}
