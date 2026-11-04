// SPDX-License-Identifier: AGPL-3.0-or-later

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::http::header;
use axum::response::Redirect;
use axum::{Router, routing::get};
use rustxng::fetch::{MAX_BODY_BYTES, PageReader};
use rustxng::settings::Outgoing;

#[tokio::test]
#[ignore = "requires Node.js and tools/page-renderer's Firefox installation"]
async fn javascript_pages_keep_guarded_fetches_limits_and_cached_text() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://example.test:{}", address.port());
    let canary = Arc::new(AtomicUsize::new(0));
    let connections = canary.clone();
    let page_calls = Arc::new(AtomicUsize::new(0));
    let calls = page_calls.clone();
    let text = "日本語の動的本文。".repeat(1000);
    let body = text.clone();
    let app = Router::new()
        .route("/start", get(|| async { Redirect::temporary("/article/index.html") }))
        .route("/article/index.html", get(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { ([(header::CONTENT_TYPE, "text/html")],
                "<main>Loading</main><script src='app.js'></script>") }
        }))
        .route("/article/app.js", get(|| async { (
            [(header::CONTENT_TYPE, "application/javascript")],
            "fetch('body.json').then(r => r.json()).then(p => document.querySelector('main').textContent = p.text)",
        ) }))
        .route("/article/body.json", get(move || async move {
            axum::Json(serde_json::json!({"text": body}))
        }))
        .route("/article.xhtml", get(|| async { (
            [(header::CONTENT_TYPE, "application/xhtml+xml; charset=utf-8")],
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Fixture</title></head>
                <body><main>Loading</main><script>
                if (1 &lt; 2) document.querySelector('main').textContent = '日本語のXHTML本文';
                </script></body></html>"#,
        ) }))
        .route("/private", get(move || async move { (
            [(header::CONTENT_TYPE, "text/html")],
            format!("<main>Loading</main><script src='http://{address}/canary'></script>"),
        ) }))
        .route("/canary", get(move || {
            connections.fetch_add(1, Ordering::SeqCst);
            async { "private response" }
        }))
        .route("/redirected-resource", get(|| async { (
            [(header::CONTENT_TYPE, "text/html")],
            "<main>Loading</main><script src='/script-redirect'></script>",
        ) }))
        .route("/script-redirect", get(|| async { Redirect::temporary("/article/app.js") }))
        .route("/large-resource", get(|| async { (
            [(header::CONTENT_TYPE, "text/html")],
            "<main>Loading</main><script src='/large.js'></script>",
        ) }))
        .route("/large.js", get(|| async { (
            [(header::CONTENT_TYPE, "application/javascript")],
            " ".repeat(MAX_BODY_BYTES + 1),
        ) }))
        .route("/large-dom", get(|| async { (
            [(header::CONTENT_TYPE, "text/html")],
            format!("<main>Loading</main><script>document.querySelector('main').textContent = 'x'.repeat({})</script>", MAX_BODY_BYTES),
        ) }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let reader =
        PageReader::new(&Outgoing::default(), &[("example.test".into(), address)]).unwrap();
    let url = format!("{origin}/start");
    assert_eq!(&*reader.read(&url, 20000).await.unwrap().text, "Loading");
    let reader = reader.with_renderer(vec![
        "node".into(),
        format!(
            "{}/tools/page-renderer/render.cjs",
            env!("CARGO_MANIFEST_DIR")
        ),
    ]);
    let page = reader.read(&url, 20000).await.unwrap();
    assert_eq!(&*page.text, text);
    assert!(!page.source_truncated);
    assert_eq!(&*reader.read(&url, 20000).await.unwrap().text, text);
    assert_eq!(page_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        &*reader
            .read(&format!("{origin}/article.xhtml"), 100)
            .await
            .unwrap()
            .text,
        "日本語のXHTML本文"
    );
    for (path, expected) in [
        ("private", "only public http(s) rendering resources"),
        ("redirected-resource", "redirected rendering resources"),
        ("large-resource", "decoded resource byte limit"),
        ("large-dom", "rendered HTML"),
    ] {
        let error = reader
            .read(&format!("{origin}/{path}"), 20000)
            .await
            .unwrap_err();
        assert!(error.contains(expected), "{path}: {error}");
    }
    assert_eq!(canary.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn missing_renderer_is_an_error_for_html_only() {
    let app = Router::new()
        .route(
            "/html",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    "<main>Article</main>",
                )
            }),
        )
        .route(
            "/text",
            get(|| async { ([(header::CONTENT_TYPE, "text/plain")], "Article") }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let reader = PageReader::new(&Outgoing::default(), &[("example.test".into(), address)])
        .unwrap()
        .with_renderer(vec![
            directory
                .path()
                .join("missing-renderer")
                .to_string_lossy()
                .into_owned(),
        ]);
    let origin = format!("http://example.test:{}", address.port());
    assert_eq!(
        &*reader
            .read(&format!("{origin}/text"), 100)
            .await
            .unwrap()
            .text,
        "Article"
    );
    assert!(
        reader
            .read(&format!("{origin}/html"), 100)
            .await
            .unwrap_err()
            .contains("cannot start page renderer")
    );
    server.abort();
}

#[tokio::test]
#[ignore = "requires Node.js and tools/page-renderer's Firefox installation"]
async fn cancelling_a_render_removes_the_browser_profile() {
    let pending = Arc::new(tokio::sync::Notify::new());
    let started = pending.clone();
    let app = Router::new()
        .route(
            "/article",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    "<main>Waiting</main><script src='/waiting.js'></script>",
                )
            }),
        )
        .route(
            "/waiting.js",
            get(move || {
                started.notify_one();
                async { std::future::pending::<&'static str>().await }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("profile-root");
    let reader = PageReader::new(&Outgoing::default(), &[("example.test".into(), address)])
        .unwrap()
        .with_renderer(vec![
            "node".into(),
            "-e".into(),
            "require('node:fs').writeFileSync(process.argv[1], require('node:os').tmpdir()); require(process.argv[2]);".into(),
            marker.to_string_lossy().into_owned(),
            format!("{}/tools/page-renderer/render.cjs", env!("CARGO_MANIFEST_DIR")),
        ]);
    let task = tokio::spawn(async move {
        reader
            .read(
                &format!("http://example.test:{}/article", address.port()),
                100,
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(15), pending.notified())
        .await
        .unwrap();
    let profile = std::path::PathBuf::from(std::fs::read_to_string(marker).unwrap());
    assert!(std::fs::read_dir(&profile).unwrap().next().is_some());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while profile.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("cancelled render retained its profile");
    server.abort();
}
