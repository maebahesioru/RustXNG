//! Probe: youtube UA acceptance with the exact default headers.
use std::time::Duration;

const URL: &str = "https://www.youtube.com/results?search_query=rust";

async fn try_case(label: &str, ua: &str, dnt: bool, al: &str) {
    let client = reqwest::Client::builder().user_agent(ua).build().unwrap();
    let mut request = client.get(URL);
    request = request.header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8");
    request = request.header("Accept-Language", al);
    if dnt {
        request = request.header("DNT", "1");
    }
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
    let ff = "Mozilla/5.0 (X11; Linux x86_64; rv:157.0) Gecko/20100101 Firefox/157.0";
    try_case("ff_no_dnt", ff, false, "en-US,en;q=0.7").await;
    try_case("ff_with_dnt", ff, true, "en-US,en;q=0.7").await;
    try_case("ff_al_09", ff, false, "en-US,en;q=0.9").await;
    try_case("ff_curl_ua", "curl/8.14.1", false, "en-US,en;q=0.7").await;
}
