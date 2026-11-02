//! Favicon resolver and proxy, ported from upstream's 'searx/favicons/'.
//!
//! Upstream keeps a resolver plugin per favicon service (DuckDuckGo, Google,
//! ...) and computes a 'data:' URL for favicons that are already cached.  This
//! port does the simple thing: 'favicon_url()' returns a signed link to
//! '/favicon_proxy' for the *origin* of a result URL, and the handler asks that
//! origin itself for '/favicon.ico' (and for '/favicon.png' as a fallback) - no
//! third party favicon service is involved at all.
//!
//! Like the image proxy, the request is signed with HMAC-SHA256 over
//! 'server.secret_key' and private hosts are refused, so the proxy cannot be
//! used to reach the local network.
//!
//! The Lead declares 'pub mod favicons;' in 'src/web/mod.rs' and adds the route
//! '.route("/favicon_proxy", get(favicons::handler))' to 'web::router()'.

use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::AppState;
use super::image_proxy::{is_public_url, is_valid, signature};
use crate::engine::EngineRequest;

/// The path of the favicon proxy (upstream's 'favicon_proxy' route).
pub const PROXY_PATH: &str = "/favicon_proxy";

/// Maximum size of a proxied favicon (half a megabyte is plenty).
pub const MAX_SIZE: usize = 512 * 1024;

/// 'Cache-Control: max-age' of the answer; upstream defaults to seven days
/// ('FaviconProxyConfig.max_age').
pub const CACHE_MAX_AGE: u64 = 60 * 60 * 24 * 7;

/// The paths that are tried on the origin, in order.
pub static CANDIDATE_PATHS: &[&str] = &["/favicon.ico", "/favicon.png"];

/// Query parameters of '/favicon_proxy'.
#[derive(Debug, Clone, Deserialize)]
pub struct FaviconParams {
    /// The origin of the result URL ('https://example.com').
    pub url: String,
    /// The HMAC of the origin.
    pub h: Option<String>,
}

/// The origin (scheme, host and port) of an http(s) URL, or 'None' for any
/// other URL.  Only the origin is fetched, so a result URL with a path, query or
/// fragment is accepted as well.
pub fn origin_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?;
    // An IPv6 host has to keep its brackets in the origin.
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    match parsed.port() {
        Some(port) => Some(format!("{}://{}:{}", parsed.scheme(), host, port)),
        None => Some(format!("{}://{}", parsed.scheme(), host)),
    }
}

/// The signed favicon proxy URL of a result URL, used by the templates.
///
/// An empty string is returned for a result URL that is not a public http(s)
/// URL (upstream returns an empty string when no resolver is configured).
pub fn favicon_url(base_url: &str, secret: &str, result_url: &str) -> String {
    let Some(origin) = origin_of(result_url) else {
        return String::new();
    };
    if !is_public_url(&origin) {
        return String::new();
    }
    let base = base_url.trim_end_matches('/');
    format!(
        "{base}{PROXY_PATH}?url={}&h={}",
        crate::engine::util::encode(&origin),
        signature(secret, &origin)
    )
}

/// 'GET /favicon_proxy'.
pub async fn handler(
    State(state): State<AppState>,
    Query(params): Query<FaviconParams>,
    _headers: HeaderMap,
) -> Response {
    let secret = state.settings().server.secret_key.clone();
    let Some(origin) = origin_of(&params.url) else {
        return (
            StatusCode::FORBIDDEN,
            "the URL is not a public http(s) origin",
        )
            .into_response();
    };
    let Some(candidate) = params.h.as_deref() else {
        return (StatusCode::FORBIDDEN, "missing signature").into_response();
    };
    if !is_valid(&secret, &origin, candidate) {
        return (StatusCode::FORBIDDEN, "invalid signature").into_response();
    }
    if !is_public_url(&origin) {
        return (StatusCode::FORBIDDEN, "the URL is not a public http(s) URL").into_response();
    }

    for path in CANDIDATE_PATHS {
        let url = format!("{origin}{path}");
        if let Some((content_type, body)) = fetch_favicon(&state, &url).await {
            return (
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, format!("max-age={CACHE_MAX_AGE}")),
                ],
                body,
            )
                .into_response();
        }
    }
    (StatusCode::NOT_FOUND, "no favicon found").into_response()
}

/// Fetch one candidate; redirects are followed by the network client.  'None'
/// when the answer is not a (small enough) image.
async fn fetch_favicon(state: &AppState, url: &str) -> Option<(String, Vec<u8>)> {
    let timeout = Duration::from_secs_f64(state.settings().outgoing.request_timeout.max(1.0));
    let request = EngineRequest::get(url.to_string()).header("Accept", "image/*");
    let response = state
        .searcher
        .network
        .fetch_image(request, timeout, MAX_SIZE)
        .await
        .ok()?;
    if response.body.is_empty() {
        return None;
    }
    let content_type = response
        .header("content-type")
        .unwrap_or_default()
        .to_string();
    if !content_type.starts_with("image/") {
        return None;
    }
    Some((content_type, response.body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::Searcher;
    use crate::settings::Settings;

    fn state() -> AppState {
        AppState::new(Searcher::new(Settings::defaults()).expect("searcher"))
    }

    fn secret() -> String {
        Settings::defaults().server.secret_key
    }

    #[test]
    fn origins_are_extracted_from_result_urls() {
        assert_eq!(
            origin_of("https://example.com/a/b?c=1#d").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("http://example.com:8080/x").as_deref(),
            Some("http://example.com:8080")
        );
        assert_eq!(origin_of("ftp://example.com/x"), None);
        assert_eq!(origin_of("not a url"), None);
        assert_eq!(origin_of("file:///etc/passwd"), None);
    }

    #[test]
    fn the_favicon_link_is_signed_over_the_origin() {
        let link = favicon_url(
            "http://localhost:8888/",
            &secret(),
            "https://example.com/page",
        );
        assert!(
            link.starts_with(
                "http://localhost:8888/favicon_proxy?url=https%3A%2F%2Fexample.com&h="
            )
        );
        let h = link.rsplit_once("&h=").unwrap().1;
        assert!(is_valid(&secret(), "https://example.com", h));
        assert!(!is_valid(&secret(), "https://example.org", h));
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn private_and_unknown_origins_have_no_favicon_link() {
        assert_eq!(
            favicon_url("http://localhost:8888", &secret(), "http://127.0.0.1/"),
            ""
        );
        assert_eq!(
            favicon_url("http://localhost:8888", &secret(), "http://10.0.0.1/"),
            ""
        );
        assert_eq!(
            favicon_url("http://localhost:8888", &secret(), "file:///etc/passwd"),
            ""
        );
        assert_eq!(
            favicon_url("http://localhost:8888", &secret(), "not a url"),
            ""
        );
    }

    #[test]
    fn constants_match_the_upstream_configuration() {
        assert_eq!(PROXY_PATH, "/favicon_proxy");
        assert_eq!(CACHE_MAX_AGE, 604_800);
        assert_eq!(CANDIDATE_PATHS, &["/favicon.ico", "/favicon.png"]);
        const { assert!(MAX_SIZE >= 64 * 1024) };
    }

    #[tokio::test]
    async fn the_handler_refuses_bad_requests_without_network_access() {
        let state = state();
        // No signature at all.
        let response = handler(
            State(state.clone()),
            Query(FaviconParams {
                url: "https://example.com/".to_string(),
                h: None,
            }),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // Wrong signature.
        let response = handler(
            State(state.clone()),
            Query(FaviconParams {
                url: "https://example.com/".to_string(),
                h: Some("0".repeat(64)),
            }),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // A correctly signed private origin is refused as well.
        let origin = "http://127.0.0.1";
        let response = handler(
            State(state),
            Query(FaviconParams {
                url: origin.to_string(),
                h: Some(signature(&secret(), origin)),
            }),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
