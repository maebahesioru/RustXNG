//! Stack Exchange engine, ported from `searx/engines/stackexchange.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "q&a"],
    shortcut: "so",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_API: &str = "https://api.stackexchange.com/2.3/search/advanced";

/// Stack Exchange (Stack Overflow by default).
pub struct StackExchange {
    meta: EngineMeta,
}

impl StackExchange {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// The Stack Exchange site, `stackoverflow` unless configured otherwise.
    fn site(&self) -> String {
        self.meta
            .opt_str("api_site")
            .map(str::to_string)
            .unwrap_or_else(|| "stackoverflow".to_string())
    }
}

impl Engine for StackExchange {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        // With a time range the newest questions are asked for, otherwise
        // relevance wins (upstream supports fromdate/todate too).
        let mut pairs: Vec<(&str, String)> = vec![
            ("order", "desc".to_string()),
            (
                "sort",
                if query.time_range.is_some() {
                    "creation".to_string()
                } else {
                    // upstream default api_sort is "activity"
                    self.meta
                        .opt_str("api_sort")
                        .unwrap_or("activity")
                        .to_string()
                },
            ),
            ("q", query.query_text()),
            ("site", self.site()),
            ("pagesize", "10".to_string()),
            ("page", pageno.to_string()),
        ];
        if let Some(seconds) = crate::query::TimeRange::seconds(query.time_range) {
            let from = chrono::Utc::now().timestamp().saturating_sub(seconds);
            pairs.push(("fromdate", from.to_string()));
        }
        Ok(
            EngineRequest::get(format!("{SEARCH_API}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        if let Some(message) = json.get("error_message").and_then(|value| value.as_str()) {
            return Err(EngineError::Message(format!(
                "stack exchange api: {message}"
            )));
        }
        let items = match json_path(&json, "items").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let title = strip_tags(&json_str(item, "title"));
            let url = json_str(item, "link");
            let parsed = url::Url::parse(&url).ok();
            if title.is_empty()
                || parsed.is_none_or(|url| {
                    !matches!(url.scheme(), "http" | "https") || url.host_str().is_none()
                })
            {
                continue;
            }
            let score = json_str(item, "score");
            let answers = json_str(item, "answer_count");
            let answered = item
                .get("is_answered")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let mut content = Vec::new();
            if answered && !answers.is_empty() {
                content.push(format!("{answers} answers"));
            }
            if !score.is_empty() {
                content.push(format!("score {score}"));
            }
            let owner = json_str(item, "owner.display_name");
            results.push(
                ResultBuilder::new(url, title)
                    .content(content.join(", "))
                    .author(owner)
                    .metadata("author_url", json_str(item, "owner.link"))
                    .metadata("source_license", json_str(item, "content_license"))
                    .metadata(
                        "source_terms_url",
                        "https://stackoverflow.com/legal/api-terms-of-use",
                    )
                    .metadata("score", score)
                    .metadata("answers", answers)
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    #[test]
    fn preserves_api_links_and_item_license_across_network_sites() {
        let mut config = EngineConfig::new("math", "stackexchange", "math", &["general"]);
        config.extra.insert("api_site".into(), "math".into());
        let engine = StackExchange::from_config(&config).unwrap();
        let response = EngineResponse::new(
            200,
            SEARCH_API,
            br#"{"items":[
              {"question_id":123,"title":"A question","link":"https://math.stackexchange.com/questions/123/title",
               "content_license":"CC BY-SA 3.0","owner":{"display_name":"Author","link":"https://math.stackexchange.com/users/9/author"}},
              {"question_id":456,"title":"Another","link":"https://ja.stackoverflow.com/questions/456/title"},
              {"question_id":789,"title":"Missing link"},
              {"title":"Invalid link","link":"javascript:alert(1)"}
            ]}"#.to_vec(),
        );
        let query = Query::parse("question", &EngineRegistry::default(), &[]);
        let items = engine.parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(first) = &items[0] else {
            panic!("expected result");
        };
        assert_eq!(
            first.url,
            "https://math.stackexchange.com/questions/123/title"
        );
        assert_eq!(first.author.as_deref(), Some("Author"));
        assert_eq!(first.metadata["source_license"], "CC BY-SA 3.0");
        assert_eq!(
            first.metadata["author_url"],
            "https://math.stackexchange.com/users/9/author"
        );
        let EngineItem::Result(second) = &items[1] else {
            panic!("expected result");
        };
        assert_eq!(
            second.url,
            "https://ja.stackoverflow.com/questions/456/title"
        );
        assert!(!second.metadata.contains_key("source_license"));
    }
}
