use std::time::Duration;

#[tokio::main]
async fn main() {
    let url = "https://www.goodreads.com/search?q=the%20hobbit&page=1";
    let ua = "Mozilla/5.0 (X11; Linux x86_64; rv:157.0) Gecko/20100101 Firefox/157.0".to_string();
    let accepts: Vec<(&str, &str)> = vec![
        ("star", "*/*"),
        ("json", "application/json"),
        ("text-plain", "text/plain"),
        (
            "html-noq",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ),
    ];
    for (name, acc) in accepts {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(2))
            .user_agent(ua.clone())
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert(
                    reqwest::header::ACCEPT,
                    reqwest::header::HeaderValue::from_str(acc).unwrap(),
                );
                h
            })
            .build()
            .unwrap();
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                let loc = resp.headers().get("location").is_some();
                let body = resp.text().await.unwrap_or_default();
                println!(
                    "[{name}] {status} loc={loc} body={}B legacy_bookTitle={}",
                    body.len(),
                    body.matches("bookTitle").count()
                );
            }
            Err(e) => println!("[{name}] ERROR {e}"),
        }
    }
}
