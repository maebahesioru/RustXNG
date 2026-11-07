//! Probe: does the shared client reach youtube at all?
use std::time::Duration;

#[tokio::main]
async fn main() {
    let outgoing = rustxng::settings::Outgoing::default();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    for (label, url, cookie, lang) in [
        ("plain", "https://www.youtube.com/results?search_query=rust", false, false),
        ("cookie", "https://www.youtube.com/results?search_query=rust", true, false),
        ("headers", "https://www.youtube.com/results?search_query=rust", true, true),
    ] {
        let mut request = client.get(url);
        if cookie {
            request = request.header("cookie", "CONSENT=YES+");
        }
        if lang {
            request = request
                .header("accept-language", "en-US,en;q=0.9")
                .header("accept", "text/html,application/xhtml+xml");
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                println!("{label}: {status} {} ytInitialData={}", body.len(), body.contains("ytInitialData"));
            }
            Err(error) => println!("{label}: ERR {error}"),
        }
    }
    let _ = outgoing;
}
