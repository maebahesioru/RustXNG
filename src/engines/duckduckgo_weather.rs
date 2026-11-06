//! DuckDuckGo weather engine, ported from searx/engines/duckduckgo_weather.py.
//!
//! The forecast endpoint returns a JSONP-ish document; the payload sits between
//! the first and the last newline.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

// engine-id: duckduckgo_weather

const BASE_URL: &str = "https://duckduckgo.com/js/spice/forecast";

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["weather"],
    shortcut: "",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// DuckDuckGo forecast search.
pub struct DuckDuckGoWeather {
    meta: EngineMeta,
}

impl DuckDuckGoWeather {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for DuckDuckGoWeather {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let language = locale_language(&query.locale);
        let language = match language.as_str() {
            "" | "all" => "en".to_string(),
            other => other.to_string(),
        };
        Ok(EngineRequest::get(format!(
            "{base}/{}/{}",
            encode(&query.query_text()),
            language
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        // The payload is the JSON object inside the JSONP wrapper.
        let (Some(start), Some(end)) = (body.find('{'), body.rfind('}')) else {
            return Ok(Vec::new());
        };
        if end <= start {
            return Ok(Vec::new());
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&body[start..=end]) else {
            return Ok(Vec::new());
        };
        let location = json_str(&json, "location");
        if location.is_empty() {
            return Ok(Vec::new());
        }
        // The live payload carries `conditionCode` ("Rain") instead of the
        // `summary` the fixture used, so a test-only field must not be the only
        // thing the answer can be built from.
        let summary = [
            "summary",
            "currentWeather.summary",
            "currentWeather.conditionCode",
        ]
        .iter()
        .map(|path| json_str(&json, path))
        .find(|value| !value.is_empty())
        .unwrap_or_default();
        let temperature = ["currentWeather.temperature", "currentWeather.temp"]
            .iter()
            .map(|path| json_str(&json, path))
            .find(|value| !value.is_empty())
            .unwrap_or_default();
        let mut answer = format!("{location}: {summary}");
        if !temperature.is_empty() {
            answer = format!("{location}: {temperature} - {summary}");
        }
        let item = EngineItem::Answer(format!("{} ({})", answer.trim(), query.query_text()));
        Ok(vec![item])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<DuckDuckGoWeather> {
        DuckDuckGoWeather::from_config(&EngineConfig::new(
            "duckduckgo_weather",
            "duckduckgo_weather",
            "",
            &["weather"],
        ))
        .expect("engine")
    }

    #[test]
    fn builds_the_forecast_url() {
        let query = Query::parse("berlin", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(
            request.url,
            "https://duckduckgo.com/js/spice/forecast/berlin/en"
        );
    }

    #[test]
    fn parses_the_forecast_payload() {
        let body = "\nddg_spice_forecast_berlin({\"location\":\"Berlin\",\"currentWeather\":{\"summary\":\"Clear\",\"temperature\":14},\"forecastHourly\":{\"hours\":[]}});\n";
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("berlin", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Answer(answer) => {
                assert!(answer.contains("Berlin"), "{answer}");
                assert!(answer.contains("Clear"), "{answer}");
            }
            _ => panic!("expected an answer"),
        }
    }
}
