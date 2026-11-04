// SPDX-License-Identifier: AGPL-3.0-or-later

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Response, Uri, header};
use axum::{Router, routing::get};
use rustxng::fetch::{MAX_BODY_BYTES, PageReader};
use rustxng::settings::Outgoing;

#[tokio::test]
async fn pdf_pages_preserve_japanese_text_limits_and_cached_reads() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let app = Router::new().fallback(get(move |uri: Uri| {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            let content_type = match uri.path() {
                "/pdf" | "/oversized" | "/not-pdf" => Some("application/pdf; charset=binary"),
                "/binary" => Some("application/octet-stream"),
                "/mislabelled" => Some("text/plain"),
                _ => None,
            };
            let mut response = Response::builder();
            if let Some(content_type) = content_type {
                response = response.header(header::CONTENT_TYPE, content_type);
            }
            let body = match uri.path() {
                "/oversized" => {
                    let mut bytes = b"%PDF-".to_vec();
                    bytes.resize(MAX_BODY_BYTES + 1, b' ');
                    Body::from(bytes)
                }
                "/not-pdf" => Body::from("not a PDF document"),
                _ => Body::from(include_bytes!("fixtures/japanese.pdf").as_slice()),
            };
            response.body(body).unwrap()
        }
    }));
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
    let expected = "日本語の政策資料 本文抽出を検証します。";
    for path in ["pdf", "binary", "mislabelled", "missing"] {
        let url = format!("http://example.test:{}/{path}", address.port());
        let before = calls.load(Ordering::SeqCst);
        let page = reader.read(&url, 5).await.unwrap();
        assert_eq!(&*page.text, "日本語の政…");
        assert!(page.source_truncated);
        assert_eq!(&*reader.read(&url, 5).await.unwrap().text, &*page.text);
        assert_eq!(calls.load(Ordering::SeqCst), before + 1);

        let whole = reader.read(&url, 100).await.unwrap();
        assert_eq!(&*whole.text, expected);
        assert!(!whole.source_truncated);
    }
    for (path, error) in [
        ("oversized", "4 MiB decoded body limit"),
        ("not-pdf", "not a PDF document"),
    ] {
        let url = format!("http://example.test:{}/{path}", address.port());
        assert!(reader.read(&url, 100).await.unwrap_err().contains(error));
    }
    server.abort();
}
