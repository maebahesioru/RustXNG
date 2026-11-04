//! Offline regressions for every HTTP route that reads third-party pages.
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

use super::{AppState, image_proxy, router};
use crate::aggregator::Searcher;
use crate::fetch::{PageReader, test_public_client};
use crate::network::Network;
use crate::settings::{EngineConfig, Settings};

struct Dns {
    answer: SocketAddr,
    calls: Arc<AtomicUsize>,
}

impl Resolve for Dns {
    fn resolve(&self, _: Name) -> Resolving {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self.answer;
        Box::pin(async move { Ok(Box::new([answer].into_iter()) as Addrs) })
    }
}

struct Server {
    address: SocketAddr,
    connections: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    /// Count TCP connections, including ones that never send an HTTP request.
    async fn start(response: String) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let response = response.clone();
                tokio::spawn(async move {
                    let mut request = [0; 8192];
                    let _ = stream.read(&mut request).await;
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Self {
            address,
            connections,
            task,
        }
    }

    fn url(&self, host: &str) -> String {
        format!("http://{host}:{}", self.address.port())
    }
}

fn response(body: &str, content_type: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

async fn state(
    target: &str,
    dns: Arc<Dns>,
    overrides: &[(String, SocketAddr)],
    images: bool,
) -> (AppState, Server) {
    let engine_body = serde_json::json!([{
        "url": target, "title": [{"value": "SSRF fixture"}],
        "extract": [{"value": "Safe engine snippet"}]
    }])
    .to_string();
    let engine = Server::start(response(&engine_body, "application/json")).await;
    let mut settings = Settings::defaults();
    let mut config = EngineConfig::new("mwmbl", "mwmbl", "mwm", &["general"]);
    config.base_url = Some(engine.url("127.0.0.1"));
    settings.engines = vec![config];
    settings.server.image_proxy = images;
    settings.server.limiter = crate::settings::LimiterSettings::Enabled(false);
    settings.research.enabled = false;
    settings.research.api_key.clear();
    settings.outgoing.retries = 0;
    let client = test_public_client(&settings.outgoing, overrides, dns);
    let mut searcher = Searcher::new(settings).unwrap();
    searcher.network = Arc::new(
        Network::from_settings(&searcher.settings)
            .unwrap()
            .with_test_media_client(client.clone()),
    );
    searcher.page_reader = PageReader::new(&searcher.settings.outgoing, &[])
        .unwrap()
        .with_test_client(client);
    (AppState::new(searcher), engine)
}

async fn get(state: AppState, uri: &str) -> (StatusCode, String) {
    let result = router(state)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = result.status();
    let bytes = result.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn assert_routes_block(target: &str, dns: Arc<Dns>, overrides: &[(String, SocketAddr)]) {
    for path in ["/image_proxy", "/favicon_proxy"] {
        let (state, _engine) = state(target, dns.clone(), overrides, true).await;
        // A valid signature ensures this reaches destination validation.
        let signature = image_proxy::signature(&state.settings().server.secret_key, target);
        let uri = format!(
            "{path}?url={}&h={signature}",
            crate::engine::util::encode(target)
        );
        let (status, _) = get(state, &uri).await;
        assert!(
            matches!(
                status,
                StatusCode::FORBIDDEN | StatusCode::BAD_GATEWAY | StatusCode::NOT_FOUND
            ),
            "{path}: {status}"
        );
    }
    // Raw pages and research must remain protected with image proxy disabled.
    for path in [
        "/search?q=fixture&format=json&full=1&include_raw_content=1",
        "/research?q=fixture&max_queries=1&max_sources=1",
    ] {
        let (state, engine) = state(target, dns.clone(), overrides, false).await;
        let (status, body) = get(state, path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert!(
            engine.connections.load(Ordering::SeqCst) > 0,
            "engine must execute"
        );
        assert!(
            body.contains("Safe engine snippet"),
            "a real engine result must remain: {body}"
        );
        assert!(!body.contains("PRIVATE_SENTINEL"), "{path}: {body}");
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        if path.starts_with("/search") {
            assert!(json["results"][0]["raw_content"].is_null());
        } else {
            assert_eq!(json["llm_used"], false);
        }
    }
}

#[tokio::test]
async fn every_fetch_route_rejects_private_dns_before_connecting() {
    let secret = Server::start(response("PRIVATE_SENTINEL", "text/html")).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let dns = Arc::new(Dns {
        answer: secret.address,
        calls: calls.clone(),
    });
    assert_routes_block(&secret.url("attacker.example"), dns, &[]).await;
    assert!(
        calls.load(Ordering::SeqCst) >= 4,
        "exercise DNS, not just URL rejection"
    );
    assert_eq!(secret.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn every_fetch_route_revalidates_literal_and_hostname_redirects() {
    for hostname in ["127.0.0.1", "attacker.example"] {
        let secret = Server::start(response("PRIVATE_SENTINEL", "image/png")).await;
        let target = secret.url(hostname);
        let redirect = Server::start(format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let dns = Arc::new(Dns {
            answer: secret.address,
            calls: calls.clone(),
        });
        // Only the entry host is pinned to our local fixture. The redirect
        // target goes through the normal URL and DNS validation.
        assert_routes_block(
            &redirect.url("entry.example"),
            dns,
            &[("entry.example".into(), redirect.address)],
        )
        .await;
        assert!(redirect.connections.load(Ordering::SeqCst) >= 4);
        if hostname == "attacker.example" {
            assert!(calls.load(Ordering::SeqCst) >= 4);
        }
        assert_eq!(secret.connections.load(Ordering::SeqCst), 0);
    }
}
