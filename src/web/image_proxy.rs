//! Image proxy, ported from the image proxy of `searx/webapp.py` and
//! `searx/webutils.py`.
//!
//! Upstream signs the proxied URL with an HMAC over `server.secret_key`; we do
//! the same with HMAC-SHA256. Unlike upstream we additionally refuse private
//! addresses, so the proxy cannot be used to reach the local network.

use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::AppState;
use crate::engine::EngineRequest;

/// Maximum size of a proxied image (8 MiB, like upstream's `image_proxy_max_size`).
pub const MAX_SIZE: usize = 8 * 1024 * 1024;

/// Query parameters of `/image_proxy`.
#[derive(Debug, Clone, Deserialize)]
pub struct ImageProxyParams {
    pub url: String,
    pub h: Option<String>,
}

/// HMAC-SHA256 of `message` under `key`.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key_block = [0u8; 64];
    if key.len() > 64 {
        let digest = Sha256::digest(key);
        key_block[..32].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for index in 0..64 {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner);
    outer.finalize().into()
}

/// Hex encoded HMAC-SHA256 signature of a URL.
pub fn signature(secret: &str, url: &str) -> String {
    let digest = hmac_sha256(secret.as_bytes(), url.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Constant time comparison of a URL signature.
pub fn is_valid(secret: &str, url: &str, candidate: &str) -> bool {
    let expected = signature(secret, url);
    if expected.len() != candidate.len() {
        return false;
    }
    expected
        .bytes()
        .zip(candidate.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// The signed proxy URL of an image, used by the templates.
pub fn proxied_url(base_url: &str, secret: &str, url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    format!(
        "{base}/image_proxy?url={}&h={}",
        crate::engine::util::encode(url),
        signature(secret, url)
    )
}

/// Whether a URL may be fetched by the proxy (public http/https host only).
pub fn is_public_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_matches(['[', ']']).to_lowercase();
    if host == "localhost" || host.ends_with(".local") || host == "::1" || host == "0.0.0.0" {
        return false;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(address) => is_public_address(address),
        Err(_) => true,
    }
}

fn is_public_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(address) => {
            !(address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_broadcast()
                || address.is_documentation()
                || address.is_unspecified())
        }
        std::net::IpAddr::V6(address) => !(address.is_loopback() || address.is_unspecified()),
    }
}

/// `GET /image_proxy`.
pub async fn handler(
    State(state): State<AppState>,
    Query(params): Query<ImageProxyParams>,
    headers: axum::http::HeaderMap,
) -> Response {
    let settings = state.settings();
    if !settings.server.image_proxy {
        return (
            StatusCode::FORBIDDEN,
            "the image proxy is disabled on this instance",
        )
            .into_response();
    }
    let Some(candidate) = params.h.as_deref() else {
        return (StatusCode::FORBIDDEN, "missing signature").into_response();
    };
    if !is_valid(&settings.server.secret_key, &params.url, candidate) {
        return (StatusCode::FORBIDDEN, "invalid signature").into_response();
    }
    if !is_public_url(&params.url) {
        return (StatusCode::FORBIDDEN, "the URL is not a public http(s) URL").into_response();
    }

    let mut request = EngineRequest::get(params.url.clone());
    if let Some(referer) = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
    {
        request = request.header("Referer", referer);
    }
    let response = match state
        .searcher
        .network
        .fetch_image(
            request,
            std::time::Duration::from_secs_f64(settings.outgoing.request_timeout.max(1.0)),
            MAX_SIZE,
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return (StatusCode::BAD_GATEWAY, format!("upstream error: {error}")).into_response();
        }
    };
    let content_type = response
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();
    if !content_type.starts_with("image/") {
        return (StatusCode::BAD_GATEWAY, "upstream did not return an image").into_response();
    }
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "max-age=86400".to_string()),
        ],
        response.body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_stable_and_verified() {
        let signature = signature("secret", "https://example.com/a.png");
        assert_eq!(signature.len(), 64);
        assert!(is_valid("secret", "https://example.com/a.png", &signature));
        assert!(!is_valid("secret", "https://example.com/b.png", &signature));
        assert!(!is_valid("other", "https://example.com/a.png", &signature));
    }

    #[test]
    fn private_hosts_are_refused() {
        assert!(!is_public_url("http://127.0.0.1/x.png"));
        assert!(!is_public_url("http://10.0.0.5/x.png"));
        assert!(!is_public_url("http://192.168.1.1/x.png"));
        assert!(!is_public_url("http://169.254.1.1/x.png"));
        assert!(!is_public_url("http://localhost/x.png"));
        assert!(!is_public_url("file:///etc/passwd"));
        assert!(is_public_url("https://upload.wikimedia.org/x.png"));
    }

    #[test]
    fn proxy_urls_are_signed() {
        let url = proxied_url(
            "http://localhost:8888",
            "secret",
            "https://example.com/a b.png",
        );
        assert!(url.starts_with("http://localhost:8888/image_proxy?url="));
        assert!(url.contains("&h="));
    }
}
