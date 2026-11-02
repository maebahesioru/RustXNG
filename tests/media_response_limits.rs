use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, Uri, header};
use futures::StreamExt;
use tower::ServiceExt;

use rustxng::web::{AppState, favicons, image_proxy, router};
use rustxng::{Searcher, Settings};

#[tokio::test]
async fn media_proxies_refuse_remote_dns_proxies_without_connecting() {
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    let upstream = Router::new().fallback(move |uri: Uri| {
        counter.fetch_add(1, Ordering::SeqCst);
        let limit = if uri.path() == "/image" {
            image_proxy::MAX_SIZE
        } else {
            favicons::MAX_SIZE
        };
        async move {
            let chunks =
                futures::stream::once(async move { Ok::<_, Infallible>(vec![b'x'; limit + 1]) })
                    .chain(futures::stream::pending());
            (
                [(header::CONTENT_TYPE, "image/png")],
                Body::from_stream(chunks),
            )
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let origin = format!("http://example.com:{}", address.port());
    let mut settings = Settings::defaults();
    settings.engines.clear();
    settings.server.image_proxy = true;
    settings.server.secret_key = "media-test".into();
    settings.outgoing.request_timeout = 10.0;
    settings
        .outgoing
        .proxies
        .insert("all".into(), format!("http://{address}"));
    let app = router(AppState::new(Searcher::new(settings).unwrap()));
    for (uri, expected) in [
        (
            image_proxy::proxied_url("", "media-test", &format!("{origin}/image")),
            StatusCode::BAD_GATEWAY,
        ),
        (
            favicons::favicon_url("", "media-test", &origin),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            app.clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()),
        )
        .await
        .expect("media proxies must reject before the body ends")
        .unwrap();
        assert_eq!(response.status(), expected);
    }
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    server.abort();
}
