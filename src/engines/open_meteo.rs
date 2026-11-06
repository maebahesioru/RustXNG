//! Open Meteo engine, ported from searx/engines/open_meteo.py.
//!
//! Upstream resolves the place with the geocoding API before it builds the
//! forecast URL. The two calls are wired through build_request (geocoding)
//! and followup_request (forecast) so a plain place name works as well as
//! the latitude and longitude options. Only the current conditions are
//! answered.
// engine-id: open_meteo

use std::sync::Arc;

use serde_json::Value;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["weather"],
    shortcut: "om",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const API_URL: &str = "https://api.open-meteo.com";
const GEOCODING_URL: &str = "https://geocoding-api.open-meteo.com";

/// The current and hourly fields of upstream's data_of_interest tuple.
const DATA_OF_INTEREST: &[&str] = &[
    "temperature_2m",
    "apparent_temperature",
    "relative_humidity_2m",
    "apparent_temperature",
    "cloud_cover",
    "pressure_msl",
    "wind_speed_10m",
    "wind_direction_10m",
    "weather_code",
];


fn forecast_request(meta: &EngineMeta, latitude: f64, longitude: f64) -> EngineRequest {
    let base = meta.base_url.as_deref().unwrap_or(API_URL).trim_end_matches('/');
    let fields = DATA_OF_INTEREST.join(",");
    let pairs: Vec<(&str, String)> = vec![
        ("latitude", latitude.to_string()),
        ("longitude", longitude.to_string()),
        ("timeformat", "unixtime".to_string()),
        ("timezone", "auto".to_string()),
        ("format", "json".to_string()),
        ("current", fields.clone()),
        ("forecast_days", "3".to_string()),
        ("hourly", fields),
    ];
    EngineRequest::get(format!("{base}/v1/forecast?{}", query_string(&pairs)))
}

fn geocoding_request(text: &str) -> EngineRequest {
    let pairs: Vec<(&str, String)> = vec![
        ("name", text.to_string()),
        ("count", "1".to_string()),
        ("language", "en".to_string()),
        ("format", "json".to_string()),
    ];
    EngineRequest::get(format!("{GEOCODING_URL}/v1/search?{}", query_string(&pairs)))
}

/// Open-Meteo, the weather forecast API.
pub struct OpenMeteo {
    meta: EngineMeta,
}

impl OpenMeteo {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn coordinate(meta: &EngineMeta, key: &str) -> Option<f64> {
    meta.opt_f64(key).or_else(|| {
        meta.opt_str(key)
            .and_then(|value| value.parse::<f64>().ok())
    })
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

fn value_at(value: &Value, path: &str) -> Option<f64> {
    json_path(value, path).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    })
}

/// The WMO weather code map of upstream.
fn condition(code: u32) -> &'static str {
    match code {
        0 => "clear sky",
        1 => "fair",
        2 => "partly cloudy",
        3 => "cloudy",
        45 | 48 => "fog",
        51 | 53 | 55 | 61 => "light rain",
        56 | 66 => "light sleet showers",
        57 | 67 => "light sleet",
        63 => "rain",
        65 => "heavy rain",
        71 => "light sleet",
        73 => "sleet",
        75 => "heavy sleet",
        77 => "snow",
        80 => "light rain showers",
        81 => "rain showers",
        82 => "heavy rain showers",
        85 => "snow showers",
        86 => "heavy snow showers",
        95 => "rain and thunder",
        96 => "light snow and thunder",
        99 => "heavy snow and thunder",
        _ => "unknown",
    }
}

impl Engine for OpenMeteo {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        if let (Some(latitude), Some(longitude)) = (
            coordinate(&self.meta, "latitude"),
            coordinate(&self.meta, "longitude"),
        ) {
            return Ok(forecast_request(&self.meta, latitude, longitude));
        }
        let text = query.query_text();
        let text = text.trim();
        if text.is_empty() {
            return Err(EngineError::Message(
                "open_meteo needs a place name or the latitude and longitude options".to_string(),
            ));
        }
        Ok(geocoding_request(text))
    }

    fn followup_request(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Option<EngineRequest>, EngineError> {
        if coordinate(&self.meta, "latitude").is_some()
            && coordinate(&self.meta, "longitude").is_some()
        {
            return Ok(None);
        }
        let Ok(json) = response.json() else {
            return Ok(None);
        };
        let Some(first) = json
            .get("results")
            .and_then(|value| value.as_array())
            .and_then(|results| results.first())
        else {
            return Ok(None);
        };
        let latitude = first.get("latitude").and_then(|value| value.as_f64());
        let longitude = first.get("longitude").and_then(|value| value.as_f64());
        match (latitude, longitude) {
            (Some(latitude), Some(longitude)) => Ok(Some(forecast_request(
                &self.meta,
                latitude,
                longitude,
            ))),
            _ => Ok(None),
        }
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(temperature) = value_at(&json, "current.temperature_2m") else {
            return Ok(Vec::new());
        };
        let code = value_at(&json, "current.weather_code").unwrap_or(0.0) as u32;
        let location = match self.meta.opt_str("location") {
            Some(location) => location.to_string(),
            None => query.query_text(),
        };
        let location = location.trim();
        let location = if location.is_empty() {
            "Open-Meteo"
        } else {
            location
        };
        let mut parts = vec![
            format!("{location}: {} °C", number(temperature)),
            condition(code).to_string(),
        ];
        if let Some(feels_like) = value_at(&json, "current.apparent_temperature") {
            parts.push(format!("feels like {} °C", number(feels_like)));
        }
        if let Some(wind) = value_at(&json, "current.wind_speed_10m") {
            parts.push(format!("wind {} km/h", number(wind)));
        }
        if let Some(humidity) = value_at(&json, "current.relative_humidity_2m") {
            parts.push(format!("humidity {} %", number(humidity)));
        }
        if let Some(pressure) = value_at(&json, "current.pressure_msl") {
            parts.push(format!("pressure {} hPa", number(pressure)));
        }
        Ok(vec![EngineItem::Answer(parts.join(", "))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> EngineConfig {
        let mut config = EngineConfig::new("openmeteo", "open_meteo", "om", &["weather"]);
        config
            .extra
            .insert("latitude".to_string(), toml::Value::Float(52.52));
        config
            .extra
            .insert("longitude".to_string(), toml::Value::Float(13.41));
        config.extra.insert(
            "location".to_string(),
            toml::Value::String("Berlin".to_string()),
        );
        config
    }

    fn engine() -> Arc<OpenMeteo> {
        OpenMeteo::from_config(&config()).expect("open_meteo engine")
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    fn response(body: &str) -> EngineResponse {
        EngineResponse::new(
            200,
            "https://api.open-meteo.com/v1/forecast",
            body.as_bytes().to_vec(),
        )
    }

    #[test]
    fn builds_the_forecast_url() {
        let request = engine().build_request(&query("Berlin"), 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://api.open-meteo.com/v1/forecast?")
        );
        assert!(request.url.contains("latitude=52.52"));
        assert!(request.url.contains("longitude=13.41"));
        assert!(request.url.contains("current=temperature_2m"));
    }

    #[test]
    fn a_place_name_goes_through_the_geocoding_api() {
        let config = EngineConfig::new("openmeteo", "open_meteo", "om", &["weather"]);
        let engine = OpenMeteo::from_config(&config).unwrap();
        let request = engine.build_request(&query("Berlin"), 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://geocoding-api.open-meteo.com/v1/search?"),
            "{}",
            request.url
        );
        assert!(request.url.contains("name=Berlin"));
    }

    #[test]
    fn an_empty_message_without_coordinates_is_an_error() {
        let config = EngineConfig::new("openmeteo", "open_meteo", "om", &["weather"]);
        let engine = OpenMeteo::from_config(&config).unwrap();
        assert!(engine.build_request(&query(""), 1).is_err());
    }

    #[test]
    fn the_geocoding_answer_becomes_the_forecast_request() {
        let config = EngineConfig::new("openmeteo", "open_meteo", "om", &["weather"]);
        let engine = OpenMeteo::from_config(&config).unwrap();
        let body = r#"{"results":[{"name":"Berlin","latitude":52.52437,"longitude":13.41053,"country_code":"DE"}]}"#;
        let geocoded = response(body);
        let followup = engine
            .followup_request(&geocoded, &query("Berlin"))
            .unwrap()
            .expect("a followup request");
        assert!(followup.url.starts_with("https://api.open-meteo.com/v1/forecast?"));
        assert!(followup.url.contains("latitude=52.52437"));
        assert!(followup.url.contains("longitude=13.41053"));
        let nothing = response(r#"{"generationtime_ms":0.1}"#);
        assert!(engine
            .followup_request(&nothing, &query("Berlin"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn answers_the_current_conditions() {
        let body = r##"{"current":{"time":1700000000,"temperature_2m":21.4,"apparent_temperature":20.1,"relative_humidity_2m":55,"cloud_cover":10,"pressure_msl":1013.2,"wind_speed_10m":12.3,"wind_direction_10m":180,"weather_code":2}}"##;
        let items = engine().parse(&response(body), &query("Berlin")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Answer(answer) => assert_eq!(
                answer,
                "Berlin: 21.4 °C, partly cloudy, feels like 20.1 °C, wind 12.3 km/h, humidity 55 %, pressure 1013.2 hPa"
            ),
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[test]
    fn a_response_without_current_is_empty() {
        let items = engine().parse(&response("{}"), &query("Berlin")).unwrap();
        assert!(items.is_empty());
    }
}
