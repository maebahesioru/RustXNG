use reqwest::header::{HeaderMap, HeaderValue};

fn headers(referer: &'static str, origin: &'static str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        "Accept",
        HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
    );
    h.insert(
        "Accept-Language",
        HeaderValue::from_static("en-US,en;q=0.7"),
    );
    h.insert("Referer", HeaderValue::from_static(referer));
    h.insert("Origin", HeaderValue::from_static(origin));
    h.insert("Sec-Fetch-Dest", HeaderValue::from_static("document"));
    h.insert("Sec-Fetch-Mode", HeaderValue::from_static("navigate"));
    h.insert("Sec-Fetch-Site", HeaderValue::from_static("same-origin"));
    h.insert("Sec-Fetch-User", HeaderValue::from_static("?1"));
    h
}

fn vqd_of(text: &str) -> Option<String> {
    let index = text.find("vqd")?;
    let mut rest = &text[index + 3..];
    loop {
        let trimmed = rest.trim_start_matches(['"', '\'', '=', ':', ' ']);
        if trimmed.len() == rest.len() {
            break;
        }
        rest = trimmed;
    }
    let token: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    if token.contains('-') {
        Some(token)
    } else {
        None
    }
}

async fn get(client: &reqwest::Client, label: &str, url: &str) -> (u16, String) {
    match client
        .get(url)
        .headers(headers("https://duckduckgo.com/", "https://duckduckgo.com"))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            println!(
                "{label}: {status} bytes={} result__a={} anomaly={}",
                text.len(),
                text.matches("result__a").count(),
                text.contains("anomaly")
            );
            (status, text)
        }
        Err(err) => {
            println!("{label}: ERROR {err}");
            (0, String::new())
        }
    }
}

#[tokio::main]
async fn main() {
    let client = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:157.0) Gecko/20100101 Firefox/157.0",
        )
        .cookie_store(true)
        .build()
        .unwrap();

    for index in 0..3 {
        get(
            &client,
            &format!("html-get-{index}"),
            &format!("https://html.duckduckgo.com/html/?q=rust+programming+{index}"),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    get(
        &client,
        "html-get-page2",
        "https://html.duckduckgo.com/html/?q=rust+programming&s=10",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let (_, page) = get(
        &client,
        "vqd-page",
        "https://duckduckgo.com/?q=rust&iar=images&iax=images&ia=images",
    )
    .await;
    let vqd = vqd_of(&page).unwrap_or_default();
    println!("vqd={vqd}");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    if !vqd.is_empty() {
        get(
            &client,
            "v.js",
            &format!("https://duckduckgo.com/v.js?l=us-en&o=json&q=rust&vqd={vqd}&f=,,,&p=1"),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        get(
            &client,
            "news.js",
            &format!("https://duckduckgo.com/news.js?l=us-en&o=json&q=rust&vqd={vqd}&f=,,,&p=1"),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let form = [
            ("q", "rust"),
            ("vqd", vqd.as_str()),
            ("f", ",,,"),
            ("o", "json"),
            ("p", "1"),
        ];
        match client
            .post("https://duckduckgo.com/i.js")
            .headers(headers("https://duckduckgo.com/", "https://duckduckgo.com"))
            .form(&form)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                println!(
                    "i.js-post: {status} bytes={} results={} challenge={}",
                    text.len(),
                    text.matches("\"image\"").count(),
                    text.contains("botnet_challenge")
                );
            }
            Err(err) => println!("i.js-post: ERROR {err}"),
        }
    }
}
