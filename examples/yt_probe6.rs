//! Probe: does the Firefox version number matter to youtube?
use std::time::Duration;

const URL: &str = "https://www.youtube.com/results?search_query=rust";

async fn try_ua(label: &str, ua: &str) {
    let client = reqwest::Client::builder().user_agent(ua).build().unwrap();
    let request = client.get(URL).header("Accept-Language", "en-US,en;q=0.9");
    match tokio::time::timeout(Duration::from_secs(20), request.send()).await {
        Ok(Ok(response)) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            println!("{label}: {status} len={} yt={}", body.len(), body.contains("ytInitialData"));
        }
        Ok(Err(error)) => println!("{label}: ERR {error}"),
        Err(_) => println!("{label}: TIMEOUT"),
    }
}

#[tokio::main]
async fn main() {
    try_ua("ff145", "Mozilla/5.0 (X11; Linux x86_64; rv:145.0) Gecko/20100101 Firefox/145.0").await;
    try_ua("ff143", "Mozilla/5.0 (X11; Linux x86_64; rv:143.0) Gecko/20100101 Firefox/143.0").await;
    try_ua("ff140", "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0").await;
}
