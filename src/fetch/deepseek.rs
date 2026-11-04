//! DeepSeek Fetch API backend (`POST /v1/tools/fetch`).
//!
//! The API receives a public URL and returns the page as Markdown. This module
//! owns the provider-specific validation, retries and error mapping.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

use crate::fetch::{FetchedPage, PageSource};
use crate::settings::{DeepSeekFetchSettings, Outgoing};

/// Decoded response body bytes accepted from one Fetch call.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Longest URL the DeepSeek Fetch API accepts, in characters.
const MAX_URL_CHARS: usize = 8_192;
/// Fetches tried for retryable statuses (408, 429 and 5xx).
const ATTEMPTS: usize = 2;

/// A DeepSeek Fetch client.
pub struct DeepSeekSource {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    timeout: Duration,
}

impl DeepSeekSource {
    pub fn from_settings(
        settings: &DeepSeekFetchSettings,
        timeout: f64,
        outgoing: &Outgoing,
    ) -> Result<Self, String> {
        let configured = settings.api_key.trim();
        let api_key = if configured.is_empty() {
            std::env::var("DEEPSEEK_API_KEY").unwrap_or_default()
        } else {
            configured.to_string()
        };
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() {
            return Err(
                "fetch.provider = \"deepseek\" requires fetch.deepseek.api_key \
                 or DEEPSEEK_API_KEY"
                    .into(),
            );
        }
        let base_url = settings.base_url.trim();
        let base_url = if base_url.is_empty() {
            "https://api.deepseek.com".to_string()
        } else {
            base_url.trim_end_matches('/').to_string()
        };
        let timeout = Self::validated_timeout(timeout)?;
        let client = crate::network::api_client(outgoing).map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            base_url,
            api_key,
            timeout,
        })
    }

    fn validated_timeout(timeout: f64) -> Result<Duration, String> {
        if !timeout.is_finite() || timeout <= 0.0 {
            return Err("fetch.timeout must be a finite positive duration".into());
        }
        Duration::try_from_secs_f64(timeout.max(1.0))
            .map_err(|_| "fetch.timeout is outside the representable duration range".into())
    }

    fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/tools/fetch")
        } else {
            format!("{base}/v1/tools/fetch")
        }
    }

    fn is_supported_url(url: &reqwest::Url) -> bool {
        matches!(url.scheme(), "http" | "https")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.as_str().chars().count() <= MAX_URL_CHARS
    }

    async fn fetch_inner(
        &self,
        url: &reqwest::Url,
        max_chars: usize,
    ) -> Result<FetchedPage, String> {
        if !Self::is_supported_url(url) {
            return Err(
                "only public http(s) URLs without credentials can be sent to DeepSeek".into(),
            );
        }
        let mut last_error = String::new();
        for attempt in 0..ATTEMPTS {
            match self.try_fetch(url, max_chars).await {
                Ok(content) => return Ok(content),
                Err((retryable, message)) => {
                    last_error = message;
                    if !retryable || attempt + 1 == ATTEMPTS {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
        Err(last_error)
    }

    async fn try_fetch(
        &self,
        url: &reqwest::Url,
        max_chars: usize,
    ) -> Result<FetchedPage, (bool, String)> {
        let body = serde_json::json!({ "url": url.as_str() });
        let request = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&body);
        let mut response = tokio::time::timeout(self.timeout, request.send())
            .await
            .map_err(|_| (true, "DeepSeek Fetch request timed out".to_string()))?
            .map_err(|error| (true, format!("DeepSeek Fetch request failed: {error}")))?;
        let status = response.status();
        let status_code = status.as_u16();
        let bytes = tokio::time::timeout(self.timeout, async {
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| (true, format!("DeepSeek Fetch response failed: {error}")))?
            {
                if chunk.len() > MAX_BODY_BYTES - bytes.len() {
                    return Err((
                        false,
                        "DeepSeek Fetch response exceeds the 4 MiB limit".to_string(),
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok::<Vec<u8>, (bool, String)>(bytes)
        })
        .await
        .map_err(|_| (true, "DeepSeek Fetch response timed out".to_string()))??;
        if !status.is_success() {
            let retryable = matches!(status_code, 408 | 429 | 500..=599);
            let message = Self::error_message(&bytes);
            return Err((
                retryable,
                format!("DeepSeek Fetch HTTP {status_code}: {message}"),
            ));
        }
        let payload: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            (
                false,
                format!("DeepSeek Fetch returned invalid JSON: {error}"),
            )
        })?;
        let content = payload
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if content.trim().is_empty() {
            return Err((
                false,
                "DeepSeek Fetch returned no readable text".to_string(),
            ));
        }
        let source_truncated = content.chars().count() > max_chars;
        Ok(FetchedPage {
            text: Arc::from(crate::content::truncate(content, max_chars)),
            source_truncated,
        })
    }

    /// A bounded, single-line error message. A JSON `error.message` is
    /// preferred; HTML or plain text is sanitised and capped so a gateway page
    /// cannot flood the caller or the logs.
    fn error_message(bytes: &[u8]) -> String {
        if let Ok(payload) = serde_json::from_slice::<serde_json::Value>(bytes) {
            if let Some(message) = payload
                .pointer("/error/message")
                .and_then(serde_json::Value::as_str)
            {
                return Self::sanitize_message(message);
            }
        }
        let text = String::from_utf8_lossy(bytes);
        let message = Self::sanitize_message(&text);
        if message.is_empty() {
            "no error body".to_string()
        } else {
            message
        }
    }

    fn sanitize_message(text: &str) -> String {
        text.chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(200)
            .collect()
    }
}

impl PageSource for DeepSeekSource {
    fn fetch<'a>(
        &'a self,
        url: &'a reqwest::Url,
        max_chars: usize,
    ) -> BoxFuture<'a, Result<FetchedPage, String>> {
        Box::pin(self.fetch_inner(url, max_chars))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::routing::post;

    fn settings(base_url: String) -> DeepSeekFetchSettings {
        DeepSeekFetchSettings {
            base_url,
            api_key: "KEY".to_string(),
        }
    }

    #[tokio::test]
    async fn returns_markdown_and_truncates_it() {
        use axum::Json;

        let app = Router::new().route(
            "/v1/tools/fetch",
            post(
                |headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                    let authorized = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        == Some("Bearer KEY");
                    if !authorized {
                        return (
                            StatusCode::UNAUTHORIZED,
                            Json(serde_json::json!({"error": {"message": "bad key"}})),
                        );
                    }
                    assert_eq!(body["url"].as_str(), Some("https://example.com/article"));
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "title": "Example",
                            "content": "abcdefghij"
                        })),
                    )
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let source =
            DeepSeekSource::from_settings(&settings(base_url), 5.0, &Outgoing::default()).unwrap();
        let page = source
            .fetch(
                &reqwest::Url::parse("https://example.com/article").unwrap(),
                5,
            )
            .await
            .expect("DeepSeek page");
        assert_eq!(&*page.text, "abcde…");
        assert!(page.source_truncated);
    }

    #[tokio::test]
    async fn reports_the_api_error_without_retrying_payment_errors() {
        use axum::Json;

        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let app = Router::new().route(
            "/v1/tools/fetch",
            post(move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::PAYMENT_REQUIRED,
                        Json(serde_json::json!({"error": {"message": "Insufficient balance"}})),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let source =
            DeepSeekSource::from_settings(&settings(base_url), 5.0, &Outgoing::default()).unwrap();
        let error = source
            .fetch(
                &reqwest::Url::parse("https://example.com/article").unwrap(),
                100,
            )
            .await
            .unwrap_err();
        assert!(error.contains("402"), "{error}");
        assert!(error.contains("Insufficient balance"), "{error}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_non_json_5xx_before_returning_the_successful_page() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let app = Router::new().route(
            "/v1/tools/fetch",
            post(move || {
                let counter = counter.clone();
                async move {
                    if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                        (
                            StatusCode::BAD_GATEWAY,
                            [(header::CONTENT_TYPE, "text/html")],
                            "<html>bad gateway</html>".to_string(),
                        )
                    } else {
                        (
                            StatusCode::OK,
                            [(header::CONTENT_TYPE, "application/json")],
                            r#"{"content":"recovered"}"#.to_string(),
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let source =
            DeepSeekSource::from_settings(&settings(base_url), 5.0, &Outgoing::default()).unwrap();
        let page = source
            .fetch(
                &reqwest::Url::parse("https://example.com/article").unwrap(),
                100,
            )
            .await
            .expect("the retry should succeed");
        assert_eq!(&*page.text, "recovered");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_follow_redirects_from_the_api_endpoint() {
        for redirect_status in [StatusCode::FOUND, StatusCode::TEMPORARY_REDIRECT] {
            let private_hits = Arc::new(AtomicUsize::new(0));
            let private_counter = private_hits.clone();
            let private_app = Router::new().fallback(move || {
                let private_counter = private_counter.clone();
                async move {
                    private_counter.fetch_add(1, Ordering::SeqCst);
                    "owned-private-target"
                }
            });
            let private_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let private_address = private_listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(private_listener, private_app).await.unwrap() });

            let redirect_target = format!("http://{private_address}/secret");
            let app = Router::new().route(
                "/v1/tools/fetch",
                post(move || {
                    let redirect_target = redirect_target.clone();
                    async move { (redirect_status, [(header::LOCATION, redirect_target)], "") }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

            let source =
                DeepSeekSource::from_settings(&settings(base_url), 5.0, &Outgoing::default())
                    .unwrap();
            let error = source
                .fetch(
                    &reqwest::Url::parse("https://example.com/article").unwrap(),
                    100,
                )
                .await
                .unwrap_err();
            assert!(
                error.contains(&redirect_status.as_u16().to_string()),
                "{redirect_status}: {error}"
            );
            assert_eq!(
                private_hits.load(Ordering::SeqCst),
                0,
                "{redirect_status}: the API client must not follow a redirect to a private listener"
            );
        }
    }

    #[test]
    fn rejects_non_finite_and_out_of_range_timeouts() {
        for timeout in [
            0.0,
            -1.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
            f64::MAX,
        ] {
            assert!(
                DeepSeekSource::from_settings(
                    &settings("https://api.deepseek.com".into()),
                    timeout,
                    &Outgoing::default(),
                )
                .is_err(),
                "fetch timeout {timeout} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_credentials_and_long_urls() {
        for url in [
            "https://user:pass@example.com/article",
            "ftp://example.com/article",
        ] {
            assert!(!DeepSeekSource::is_supported_url(
                &reqwest::Url::parse(url).unwrap()
            ));
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_CHARS));
        assert!(!DeepSeekSource::is_supported_url(
            &reqwest::Url::parse(&long).unwrap()
        ));
        assert!(DeepSeekSource::is_supported_url(
            &reqwest::Url::parse("https://example.com/article").unwrap()
        ));
    }

    #[test]
    fn requires_a_key() {
        if std::env::var("DEEPSEEK_API_KEY").is_ok() {
            return;
        }
        let mut config = settings("https://api.deepseek.com".into());
        config.api_key.clear();
        assert!(DeepSeekSource::from_settings(&config, 5.0, &Outgoing::default()).is_err());
    }
}
