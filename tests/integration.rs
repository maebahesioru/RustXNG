//! End-to-end tests: a fake engine served over HTTP, the whole search pipeline
//! and the public HTTP API.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, Uri, header};
use axum::routing::get;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rustxng::aggregator::Searcher;
use rustxng::settings::{EngineConfig, LimiterSettings, Settings};
use rustxng::web::{AppState, router};

/// The JSON payload the fake Mwmbl engine answers with.
const MWMBL_RESPONSE: &str = r#"[
  {
    "url": "https://example.com/rust",
    "title": [{"value": "The Rust "}, {"value": "Programming Language"}],
    "extract": [{"value": "A language empowering everyone."}]
  },
  {
    "url": "https://example.com/rust-book",
    "title": [{"value": "The Rust Book"}],
    "extract": [{"value": "Learn Rust."}]
  }
]"#;

/// Start the fake engine and return its base URL.
async fn start_fake_engine() -> String {
    let app = Router::new().route(
        "/api/v1/search/",
        get(|| async { ([(header::CONTENT_TYPE, "application/json")], MWMBL_RESPONSE) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{address}/api/v1")
}

/// Settings with a single engine pointing at the fake server.
fn settings_for(base_url: &str) -> Settings {
    let mut settings = Settings::defaults();
    let mut engine = EngineConfig::new("mwmbl", "mwmbl", "mwm", &["general"]);
    engine.base_url = Some(base_url.to_string());
    engine.timeout = Some(1.0);
    settings.engines = vec![engine];
    settings.search.autocomplete = "none".to_string();
    settings.search.timeout_limit = 1.0;
    settings.server.secret_key = "test".to_string();
    settings
}

fn app(settings: Settings) -> Router {
    let searcher = Searcher::new(settings).expect("searcher");
    router(AppState::new(searcher))
}

async fn body_text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

async fn get_uri(app: &Router, uri: &str) -> (StatusCode, String, axum::http::HeaderMap) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = body_text(response).await;
    (status, body, headers)
}

#[tokio::test]
async fn json_search_returns_merged_results() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));

    let (status, body, headers) = get_uri(&app, "/search?q=rust&format=json&full=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );

    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["search_query"], "rust");
    assert_eq!(json["results"].as_array().unwrap().len(), 2);
    assert_eq!(json["results"][0]["url"], "https://example.com/rust");
    assert_eq!(json["results"][0]["title"], "The Rust Programming Language");
    assert_eq!(json["results"][0]["engines"][0], "mwmbl");
    assert!(json["number_of_results"].as_u64().unwrap() >= 2);
}

#[tokio::test]
async fn agent_facing_filters_limit_and_scope_the_results() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));

    // max_results caps the JSON payload.
    let (_, body, _) = get_uri(&app, "/search?q=rust&format=json&max_results=1").await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["results"].as_array().unwrap().len(), 1);

    // include_domains keeps only matching hosts (subdomains included).
    let (_, body, _) = get_uri(
        &app,
        "/search?q=rust&format=json&include_domains=example.com",
    )
    .await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["results"].as_array().unwrap().len(), 2);

    let (_, body, _) = get_uri(
        &app,
        "/search?q=rust&format=json&include_domains=other.test",
    )
    .await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["results"].as_array().unwrap().len(), 0);

    // exclude_domains drops the known host again.
    let (_, body, _) = get_uri(
        &app,
        "/search?q=rust&format=json&exclude_domains=example.com",
    )
    .await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["results"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn research_returns_evidence_without_an_llm() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));

    let (status, body, headers) = get_uri(&app, "/research?q=rust").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );

    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    // No LLM configured: sub-queries still run and an evidence bundle returns.
    assert!(json["llm_used"] == false);
    assert!(json["report"].is_null());
    assert!(!json["queries"].as_array().unwrap().is_empty());
    let sources = json["sources"].as_array().unwrap();
    assert!(!sources.is_empty());
    assert!(sources[0]["url"].is_string());
    assert!(sources[0]["content"].is_string());

    // A missing question is a client error.
    let (status, _, _) = get_uri(&app, "/research").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn staged_strategy_returns_the_same_results() {
    let base_url = start_fake_engine().await;
    let mut settings = settings_for(&base_url);
    settings.search.strategy = rustxng::settings::SearchStrategy::Staged;
    settings.search.staged_tier = 1;
    settings.search.staged_min_results = 1;
    let app = app(settings);

    let (status, body, _) = get_uri(&app, "/search?q=rust&format=json").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["results"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn html_search_renders_the_result_page() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));

    let (status, body, headers) = get_uri(&app, "/search?q=rust").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert!(body.contains("The Rust Programming Language"));
    assert!(body.contains("example.com/rust"));
    assert!(body.contains("name=\"q\""));
    assert!(body.contains("mwmbl"));
}

#[tokio::test]
async fn html_search_rejects_executable_result_and_infobox_urls() {
    let upstream = Router::new()
        .route(
            "/api/v1/search/",
            get(|| async {
                axum::Json(serde_json::json!([
                    {"url": "javascript:void(0)", "title": "unsafe"},
                    {"url": "JaVaScRiPt:void(0)", "title": "unsafe case"},
                    {"url": "java\nscript:void(0)", "title": "unsafe whitespace"},
                    {"url": "https://example.com/safe", "title": "safe"},
                    {"url": "/docs/a.pdf", "title": "local document"}
                ]))
            }),
        )
        .route(
            "/",
            get(|| async {
                axum::Json(serde_json::json!({
                    "Heading": "reference",
                    "Abstract": "reference content",
                    "AbstractURL": "https://example.com/reference",
                    "AbstractSource": "safe reference",
                    "DefinitionURL": "JaVaScRiPt:void(0)",
                    "DefinitionSource": "unsafe reference"
                }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let mut settings = settings_for(&format!("http://{address}/api/v1"));
    let mut definitions =
        EngineConfig::new("definitions", "duckduckgo_definitions", "dd", &["general"]);
    definitions.base_url = Some(format!("http://{address}"));
    settings.engines.push(definitions);
    let app = app(settings);
    let (status, body, _) = get_uri(&app, "/search?q=example").await;
    assert_eq!(status, StatusCode::OK);
    let html = scraper::Html::parse_document(&body);
    let selector = scraper::Selector::parse("a.result-title, .infobox a").unwrap();
    let mut links: Vec<_> = html
        .select(&selector)
        .map(|anchor| anchor.value().attr("href").unwrap())
        .collect();
    links.sort_unstable();
    assert_eq!(
        links,
        vec![
            "/docs/a.pdf",
            "https://example.com/reference",
            "https://example.com/reference",
            "https://example.com/safe",
        ]
    );
}

#[tokio::test]
async fn csv_and_rss_formats_are_served() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));

    let (status, body, headers) = get_uri(&app, "/search?q=rust&format=csv").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/csv")
    );
    assert!(body.starts_with("title,url,content,host,engine,score,type"));
    assert!(body.contains("The Rust Programming Language"));

    let (status, body, headers) = get_uri(&app, "/search?q=rust&format=rss").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/xml")
    );
    assert!(body.contains("<rss version=\"2.0\""));
    assert!(body.contains("<link>https://example.com/rust</link>"));
}

#[tokio::test]
async fn static_pages_and_metadata_are_served() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));

    let (status, body, _) = get_uri(&app, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");

    let (status, body, _) = get_uri(&app, "/config").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["engines"][0]["name"], "mwmbl");
    assert_eq!(json["engines"][0]["shortcut"], "mwm");

    let (status, body, _) = get_uri(&app, "/opensearch.xml").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<OpenSearchDescription"));
    assert!(body.contains("/search?q={searchTerms}"));

    let (status, _, _) = get_uri(&app, "/static/style.css").await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _) = get_uri(&app, "/about").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("AGPL-3.0-or-later"));

    let (status, body, _) = get_uri(&app, "/stats").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Statistics"));
}

#[tokio::test]
async fn missing_query_and_unknown_routes_are_reported() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));

    let (status, _, _) = get_uri(&app, "/search?format=json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body, _) = get_uri(&app, "/definitely/not/a/route").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("page not found"));

    let (status, _, _) = get_uri(&app, "/search?q=rust&format=xml").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preferences_are_stored_in_cookies() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));

    let body = "action=save&language=ja&safesearch=2&category=general&enabled_engine=mwmbl&enabled_plugin=hostnames";
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/preferences")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_string())
        .collect();
    assert!(
        cookies
            .iter()
            .any(|cookie| cookie.starts_with("language=ja"))
    );
    assert!(
        cookies
            .iter()
            .any(|cookie| cookie.starts_with("safesearch=2"))
    );
}

#[tokio::test]
async fn an_unreachable_engine_is_reported_as_unresponsive() {
    // Port 9 is the discard service: nothing is listening there.
    let app = app(settings_for("http://127.0.0.1:9/api/v1"));
    let (status, body, _) = get_uri(&app, "/search?q=rust&format=json&full=1").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["results"].as_array().unwrap().is_empty());
    // Upstream shape: [[engine, "Translated error"], ...]
    let unresponsive = json["unresponsive_engines"].as_array().unwrap();
    assert_eq!(unresponsive[0][0], "mwmbl");
    assert!(
        unresponsive[0][1].as_str().unwrap().contains("rror")
            || unresponsive[0][1].as_str().unwrap().contains("imeout")
    );
}

#[tokio::test]
async fn bangs_select_engines_and_languages() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));

    let (status, body, _) = get_uri(&app, "/search?q=%21mwmbl+rust&format=json&full=1").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["results"].as_array().unwrap().len(), 2);

    // An unknown bang is not a bang: it stays part of the search terms.
    let (status, body, _) = get_uri(&app, "/search?q=%21nosuch+rust&format=json&full=1").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["search_query"], "!nosuch rust");
}

#[test]
fn the_default_settings_build_every_engine() {
    let searcher = Searcher::new(Settings::defaults()).expect("searcher");
    assert!(searcher.registry.len() >= 20);
    assert!(Arc::strong_count(&searcher.registry) >= 1);
}

#[tokio::test]
async fn metrics_endpoint_exposes_engine_counters() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));
    get_uri(&app, "/search?q=rust&format=json&full=1").await;

    let (status, body, headers) = get_uri(&app, "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/openmetrics-text")
    );
    assert!(body.contains("# HELP searxng_engines_request_count_total"));
    // The exact engine count depends on how many passes ran (the site-scoped
    // retry may query the same engine twice), so assert the shape, not a value.
    let result_line = body
        .lines()
        .find(|line| line.starts_with("searxng_engines_result_count_total{engine=\"mwmbl\"}"))
        .expect("the mwmbl result counter is exported");
    let count: u64 = result_line
        .rsplit(' ')
        .next()
        .and_then(|value| value.parse().ok())
        .expect("the counter value is a number");
    assert!(count >= 2, "expected at least two results, got {count}");
    let searches: u64 = body
        .lines()
        .find(|line| line.starts_with("searxng_search_requests_total"))
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse().ok())
        .expect("the search counter is exported");
    assert!(
        searches >= 1,
        "expected at least one search, got {searches}"
    );
    assert!(body.trim_end().ends_with("# EOF"));
}

#[tokio::test]
async fn image_proxy_refuses_unsigned_and_private_urls() {
    let base_url = start_fake_engine().await;
    let mut settings = settings_for(&base_url);
    settings.server.image_proxy = true;
    settings.server.secret_key = "test-secret".to_string();
    let app = app(settings);

    let (status, _, _) = get_uri(&app, "/image_proxy?url=https%3A%2F%2Fexample.com%2Fa.png").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "missing signature must be refused"
    );

    let (status, _, _) = get_uri(
        &app,
        "/image_proxy?url=https%3A%2F%2Fexample.com%2Fa.png&h=deadbeef",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "wrong signature must be refused"
    );

    // A correctly signed private address is still refused (SSRF guard), which
    // is stricter than upstream.
    let url = "http://127.0.0.1:9/secret";
    let signature = rustxng::web::image_proxy::signature("test-secret", url);
    let uri = format!(
        "/image_proxy?url={}&h={signature}",
        url.replace(':', "%3A").replace('/', "%2F")
    );
    let (status, _, _) = get_uri(&app, &uri).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "private addresses must be refused"
    );
}

#[tokio::test]
async fn image_proxy_is_disabled_by_default() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));
    let (status, _, _) = get_uri(
        &app,
        "/image_proxy?url=https%3A%2F%2Fexample.com%2Fa.png&h=x",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_rate_limiter_answers_429_after_the_limit() {
    let base_url = start_fake_engine().await;
    let mut settings = settings_for(&base_url);
    settings.server.limiter = LimiterSettings::Enabled(true);
    settings.server.limiter_max = 2;
    settings.server.limiter_window = 60;
    let app = app(settings);

    let mut statuses = Vec::new();
    for _ in 0..3 {
        let (status, _, _) = get_uri(&app, "/search?q=rust&format=json&full=1").await;
        statuses.push(status);
    }
    assert_eq!(
        statuses,
        vec![
            StatusCode::OK,
            StatusCode::OK,
            StatusCode::TOO_MANY_REQUESTS
        ]
    );

    // /healthz is not limited
    let (status, _, _) = get_uri(&app, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
}

/// The JSON payload the fake YaCy engine answers with.
const YACY_RESPONSE: &str = r#"{
  "channels": [{
    "items": [
      {"link": "https://example.com/yacy", "title": "YaCy Result", "description": "A result."}
    ]
  }]
}"#;

/// The JSON payload the fake declarative JSON engine answers with.
const JSON_ENGINE_RESPONSE: &str = r#"{
  "results": [
    {"url": "https://example.com/json", "title": "JSON Result", "content": "A result."}
  ]
}"#;

/// Start fake engines that record every request URI, and return the base
/// URL plus the recording.
async fn start_recording_engines() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_yacy = seen.clone();
    let seen_json = seen.clone();
    let app = Router::new()
        .route(
            "/yacysearch.json",
            get(move |uri: Uri| {
                let seen = seen_yacy.clone();
                async move {
                    seen.lock().unwrap().push(uri.to_string());
                    ([(header::CONTENT_TYPE, "application/json")], YACY_RESPONSE)
                }
            }),
        )
        .route(
            "/jsonsearch",
            get(move |uri: Uri| {
                let seen = seen_json.clone();
                async move {
                    seen.lock().unwrap().push(uri.to_string());
                    (
                        [(header::CONTENT_TYPE, "application/json")],
                        JSON_ENGINE_RESPONSE,
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), seen)
}

/// Settings with two language-aware fake engines: YaCy reads
/// `Query::engine_language`, the declarative JSON engine reads
/// `Query::locale` through its `{lang}` URL placeholder.
fn language_settings(base_url: &str) -> Settings {
    let mut settings = Settings::defaults();
    let mut yacy = EngineConfig::new("yacy", "yacy", "ya", &["general"]);
    yacy.base_url = Some(base_url.to_string());
    yacy.timeout = Some(1.0);
    let mut json = EngineConfig::new("fakejson", "json_engine", "fj", &["general"]);
    json.search_url = Some(format!("{base_url}/jsonsearch?q={{query}}&lang={{lang}}"));
    json.timeout = Some(1.0);
    for (key, value) in [
        ("results_query", "results"),
        ("url_query", "url"),
        ("title_query", "title"),
        ("content_query", "content"),
    ] {
        json.extra
            .insert(key.to_string(), toml::Value::String(value.to_string()));
    }
    settings.engines = vec![yacy, json];
    settings.search.autocomplete = "none".to_string();
    settings.search.timeout_limit = 1.0;
    settings.server.secret_key = "test".to_string();
    settings
}

#[tokio::test]
async fn language_parameter_reaches_the_engines() {
    let (base_url, seen) = start_recording_engines().await;
    let app = app(language_settings(&base_url));

    // The shape Firecrawl sends: q + language + format=json.
    let (status, body, _) = get_uri(&app, "/search?q=rust&format=json&language=fr").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["results"].as_array().unwrap().len(), 2);
    {
        let seen = seen.lock().unwrap();
        assert!(
            seen.iter()
                .any(|uri| uri.starts_with("/yacysearch.json") && uri.contains("lr=lang_fr")),
            "yacy saw: {seen:?}"
        );
        assert!(
            seen.iter()
                .any(|uri| uri.starts_with("/jsonsearch") && uri.contains("lang=fr")),
            "json engine saw: {seen:?}"
        );
    }

    // A `:lang` bang wins over the parameter, like upstream.
    seen.lock().unwrap().clear();
    let (status, _, _) = get_uri(&app, "/search?q=%3Aja+rust&format=json&language=fr").await;
    assert_eq!(status, StatusCode::OK);
    {
        let seen = seen.lock().unwrap();
        assert!(
            seen.iter()
                .any(|uri| uri.starts_with("/yacysearch.json") && uri.contains("lr=lang_ja")),
            "yacy saw: {seen:?}"
        );
        assert!(
            seen.iter()
                .any(|uri| uri.starts_with("/jsonsearch") && uri.contains("lang=ja")),
            "json engine saw: {seen:?}"
        );
    }

    // `auto` detects the query language and also reaches engines that use
    // the request locale directly, such as the JSON engine's `{lang}`.
    seen.lock().unwrap().clear();
    let (status, _, _) = get_uri(
        &app,
        "/search?q=%E6%97%A5%E6%9C%AC%E8%AA%9E&format=json&language=auto",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    {
        let seen = seen.lock().unwrap();
        assert!(
            seen.iter()
                .any(|uri| uri.starts_with("/yacysearch.json") && uri.contains("lr=lang_ja")),
            "yacy saw: {seen:?}"
        );
        assert!(
            seen.iter()
                .any(|uri| uri.starts_with("/jsonsearch") && uri.contains("lang=ja")),
            "json engine saw: {seen:?}"
        );
    }

    // An invalid language is a client error, like upstream.
    let (status, _, _) = get_uri(&app, "/search?q=rust&format=json&language=not-a-language").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body, _) = get_uri(&app, "/search?q=rust&format=json&language=xyz").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["error"], "invalid language code: xyz");
}

#[tokio::test]
async fn the_search_form_offers_a_time_range() {
    let base_url = start_fake_engine().await;
    let app = app(settings_for(&base_url));
    let (status, body, _) = get_uri(&app, "/search?q=rust&time_range=week").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("name=\"time_range\""));
    assert!(body.contains("value=\"week\" selected"));
    assert!(body.contains("Past week"));
}

/// Helper: create settings with no engines and resting enabled after 1 empty result.
fn resting_test_settings() -> Settings {
    let mut settings = Settings::defaults();
    settings.engines.clear();
    settings.search.empty_result_limit = 1;
    settings.search.empty_result_cooldown = 300;
    settings.server.secret_key = "test".to_string();
    settings
}

#[tokio::test]
async fn stats_endpoint_shows_resting_engines() {
    let settings = resting_test_settings();
    let searcher = Searcher::new(settings).expect("searcher");

    // Manually trigger resting by recording an empty success
    searcher.states.record_success(
        "testeng",
        0,
        std::time::Duration::from_millis(50),
        &searcher.settings.search,
    );

    let app = router(AppState::new(searcher));

    // /stats HTML should show the Resting column
    let (status, body, _) = get_uri(&app, "/stats").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<th>Resting</th>"),
        "stats should have Resting column"
    );
    assert!(
        body.contains("empty results"),
        "resting engine should show 'empty results'"
    );

    // /stats/errors JSON should include resting fields
    let (status, body, _) = get_uri(&app, "/stats/errors").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let engines = json.as_array().unwrap();
    let eng = engines.iter().find(|e| e["engine"] == "testeng").unwrap();
    assert!(
        eng["resting"].as_bool().unwrap(),
        "engine should be resting"
    );
    assert!(
        eng["resting_for"].as_f64().unwrap() > 0.0,
        "resting_for should be positive"
    );
    // suspended should be false (resting is distinct from failure suspension)
    assert!(
        !eng["suspended"].as_bool().unwrap(),
        "engine should not be suspended"
    );
}

#[tokio::test]
async fn metrics_endpoint_exposes_resting_gauges() {
    let settings = resting_test_settings();
    let searcher = Searcher::new(settings).expect("searcher");

    // Trigger resting
    searcher.states.record_success(
        "testeng",
        0,
        std::time::Duration::from_millis(50),
        &searcher.settings.search,
    );

    let app = router(AppState::new(searcher));

    let (status, body, _) = get_uri(&app, "/metrics").await;
    assert_eq!(status, StatusCode::OK);

    // Check HELP lines exist
    assert!(
        body.contains("# HELP searxng_engines_resting"),
        "metrics should include resting HELP"
    );
    assert!(
        body.contains("# HELP searxng_engines_resting_seconds"),
        "metrics should include resting_seconds HELP"
    );

    // Check gauge values for testeng
    let resting_line = body
        .lines()
        .find(|line| line.starts_with("searxng_engines_resting{engine=\"testeng\"}"))
        .expect("resting gauge should be exported");
    assert!(
        resting_line.ends_with(" 1"),
        "testeng should be resting (value=1)"
    );

    let seconds_line = body
        .lines()
        .find(|line| line.starts_with("searxng_engines_resting_seconds{engine=\"testeng\"}"))
        .expect("resting_seconds gauge should be exported");
    let seconds_value: f64 = seconds_line
        .rsplit(' ')
        .next()
        .and_then(|v| v.parse().ok())
        .expect("seconds value should be a number");
    assert!(seconds_value > 0.0, "resting_seconds should be positive");
}

#[tokio::test]
async fn stats_shows_recovery_from_resting() {
    let settings = resting_test_settings();
    let searcher = Searcher::new(settings).expect("searcher");
    let states = searcher.states.clone();

    // Trigger resting
    states.record_success(
        "testeng",
        0,
        std::time::Duration::from_millis(50),
        &searcher.settings.search,
    );

    let app = router(AppState::new(searcher));

    // Verify resting via /stats HTML
    let (status, body, _) = get_uri(&app, "/stats").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("empty results"),
        "engine should be resting initially"
    );

    // Verify resting via /stats/errors JSON
    let (_, body, _) = get_uri(&app, "/stats/errors").await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let eng = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["engine"] == "testeng")
        .unwrap();
    assert!(eng["resting"].as_bool().unwrap());

    // Verify resting via /metrics
    let (_, body, _) = get_uri(&app, "/metrics").await;
    let resting_line = body
        .lines()
        .find(|line| line.starts_with("searxng_engines_resting{engine=\"testeng\"}"))
        .unwrap();
    assert!(resting_line.ends_with(" 1"));

    // Recover by returning actual results (using the same Arc<EngineStates>)
    states.record_success(
        "testeng",
        5,
        std::time::Duration::from_millis(100),
        &rustxng::settings::Search {
            empty_result_limit: 1,
            empty_result_cooldown: 300,
            ..Default::default()
        },
    );

    // /stats HTML should no longer show "empty results"
    let (_, body, _) = get_uri(&app, "/stats").await;
    assert!(
        !body.contains("empty results"),
        "engine should not be resting after recovery"
    );

    // /stats/errors JSON should show resting=false, resting_for=0
    let (_, body, _) = get_uri(&app, "/stats/errors").await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let eng = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["engine"] == "testeng")
        .unwrap();
    assert!(
        !eng["resting"].as_bool().unwrap(),
        "engine should not be resting after recovery"
    );
    assert_eq!(eng["resting_for"].as_f64().unwrap(), 0.0);

    // /metrics should show resting=0, resting_seconds=0
    let (_, body, _) = get_uri(&app, "/metrics").await;
    let resting_line = body
        .lines()
        .find(|line| line.starts_with("searxng_engines_resting{engine=\"testeng\"}"))
        .unwrap();
    assert!(
        resting_line.ends_with(" 0"),
        "resting gauge should be 0 after recovery"
    );
    let seconds_line = body
        .lines()
        .find(|line| line.starts_with("searxng_engines_resting_seconds{engine=\"testeng\"}"))
        .unwrap();
    assert!(
        seconds_line.ends_with(" 0.000"),
        "resting_seconds should be 0 after recovery"
    );
}
