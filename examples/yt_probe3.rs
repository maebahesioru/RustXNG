//! Probe: isolate which client option breaks youtube.
use std::time::Duration;

const URL: &str = "https://www.youtube.com/results?search_query=rust";

async fn try_client(label: &str, client: reqwest::Client) {
    let request = client
        .get(URL)
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Accept", "text/html,application/xhtml+xml");
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
    try_client("plain", reqwest::Client::new()).await;
    try_client(
        "http1_only",
        reqwest::Client::builder().http1_only().build().unwrap(),
    )
    .await;
    try_client(
        "cookie_store",
        reqwest::Client::builder().cookie_store(true).build().unwrap(),
    )
    .await;
    try_client(
        "ua_firefox",
        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:157.0) Gecko/20100101 Firefox/157.0")
            .build()
            .unwrap(),
    )
    .await;
    try_client(
        "connect_timeout_20",
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap(),
    )
    .await;
    try_client(
        "pool_idle_20",
        reqwest::Client::builder()
            .pool_max_idle_per_host(20)
            .build()
            .unwrap(),
    )
    .await;
}
