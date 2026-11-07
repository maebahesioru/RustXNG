//! Probe: which user agent does youtube accept?
use std::time::Duration;

const URL: &str = "https://www.youtube.com/results?search_query=rust";

async fn try_ua(label: &str, ua: &str) {
    let client = reqwest::Client::builder().user_agent(ua).build().unwrap();
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
    try_ua("ff157", "Mozilla/5.0 (X11; Linux x86_64; rv:157.0) Gecko/20100101 Firefox/157.0").await;
    try_ua("ff133", "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0").await;
    try_ua("ff128", "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0").await;
    try_ua("chrome140", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36").await;
    try_ua("chrome131", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36").await;
}
