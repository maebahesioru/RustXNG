// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use super::page::{MAX_BODY_BYTES, is_fetchable};

const RENDER_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REQUESTS: usize = 64;
const MAX_RESOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROTOCOL_BYTES: u64 = (MAX_BODY_BYTES * 2) as u64;
static RENDERERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Message {
    Fetch {
        id: u64,
        url: String,
        method: String,
    },
    Html {
        body: String,
    },
    Error {
        message: String,
    },
}

pub(super) async fn render(
    client: &reqwest::Client,
    command: &[String],
    url: reqwest::Url,
    media_type: &'static str,
    html: String,
) -> Result<String, String> {
    let client = client.clone();
    let command = command.to_vec();
    let (cancel, mut cancelled) = tokio::sync::oneshot::channel::<()>();
    // Keep cleanup alive after caller cancellation.
    let result = tokio::spawn(async move {
        let _permit = tokio::select! {
            permit = RENDERERS.acquire() => permit.map_err(|error| error.to_string())?,
            _ = &mut cancelled => return Err("page rendering cancelled".into()),
        };
        run(&client, &command, url, media_type, html, &mut cancelled).await
    })
    .await
    .map_err(|error| format!("page renderer task failed: {error}"))?;
    drop(cancel);
    result
}

async fn run(
    client: &reqwest::Client,
    command: &[String],
    url: reqwest::Url,
    media_type: &str,
    html: String,
    cancelled: &mut tokio::sync::oneshot::Receiver<()>,
) -> Result<String, String> {
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut child = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .env("TMPDIR", directory.path())
        .env("TEMP", directory.path())
        .env("TMP", directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("cannot start page renderer: {error}"))?;
    let mut input = child.stdin.take().ok_or("page renderer has no stdin")?;
    let output = child.stdout.take().ok_or("page renderer has no stdout")?;
    let mut lines = BufReader::new(output.take(MAX_PROTOCOL_BYTES)).lines();
    let rendering = async {
        write_message(
            &mut input,
            &json!({"url": url.as_str(), "html": html,
                "content_type": media_type, "max_bytes": MAX_BODY_BYTES}),
        )
        .await?;
        let mut requests = 0;
        let mut remaining = MAX_RESOURCE_BYTES;
        let rendered = loop {
            let line = lines
                .next_line()
                .await
                .map_err(|error| format!("cannot read page renderer: {error}"))?
                .ok_or("page renderer exited without HTML")?;
            let message: Message = serde_json::from_str(&line)
                .map_err(|_| "invalid page renderer response".to_string())?;
            match message {
                Message::Fetch { id, url, method } => {
                    requests += 1;
                    if requests > MAX_REQUESTS {
                        return Err("page rendering exceeds 64 resource requests".into());
                    }
                    let (status, headers, body) =
                        resource(client, &url, &method, remaining).await?;
                    remaining -= body.len();
                    write_message(
                        &mut input,
                        &json!({"id": id, "status": status, "headers": headers,
                            "body": STANDARD.encode(body)}),
                    )
                    .await?;
                }
                Message::Html { body } => {
                    let bytes = STANDARD
                        .decode(body)
                        .map_err(|_| "invalid rendered HTML encoding")?;
                    if bytes.len() > MAX_BODY_BYTES {
                        return Err("rendered HTML exceeds the 4 MiB body limit".into());
                    }
                    break String::from_utf8(bytes).map_err(|_| "rendered HTML is not UTF-8")?;
                }
                Message::Error { message } => {
                    let message: String = message.chars().take(1024).collect();
                    return Err(format!("page rendering failed: {message}"));
                }
            }
        };
        drop(input);
        let status = child
            .wait()
            .await
            .map_err(|error| format!("cannot wait for page renderer: {error}"))?;
        if !status.success() {
            return Err(format!("page renderer exited with {status}"));
        }
        Ok(rendered)
    };
    let result = tokio::select! {
        _ = cancelled => Err("page rendering cancelled".into()),
        result = tokio::time::timeout(RENDER_TIMEOUT, rendering) => result
            .map_err(|_| "page rendering exceeded 15 seconds".to_string())
            .and_then(|result| result),
    };
    if result.is_err()
        && !matches!(
            tokio::time::timeout(Duration::from_secs(3), child.wait()).await,
            Ok(Ok(_))
        )
    {
        let _ = child.kill().await;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::remove_dir_all(directory.path()) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) if tokio::time::Instant::now() >= deadline => {
                return Err(format!(
                    "cannot remove page renderer temporary files: {error}"
                ));
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    result
}

async fn write_message(
    input: &mut tokio::process::ChildStdin,
    message: &serde_json::Value,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    input
        .write_all(&bytes)
        .await
        .map_err(|error| format!("cannot write to page renderer: {error}"))
}

async fn resource(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    remaining: usize,
) -> Result<(u16, BTreeMap<String, String>, Vec<u8>), String> {
    let url = reqwest::Url::parse(url).map_err(|_| "invalid rendered resource URL")?;
    if !is_fetchable(&url) {
        return Err("only public http(s) rendering resources can be read".into());
    }
    let method = match method {
        "GET" => reqwest::Method::GET,
        "HEAD" => reqwest::Method::HEAD,
        _ => return Err("page rendering only supports GET and HEAD resources".into()),
    };
    let mut response = client
        .request(method, url.clone())
        .send()
        .await
        .map_err(|error| format!("rendering resource could not be fetched: {error}"))?;
    if response.url() != &url {
        return Err("redirected rendering resources are not supported".into());
    }
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "content-type"
                    | "access-control-allow-origin"
                    | "access-control-allow-credentials"
                    | "access-control-expose-headers"
            )
        })
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_string()))
        })
        .collect();
    let limit = MAX_BODY_BYTES.min(remaining);
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if chunk.len() > limit - body.len() {
            return Err("page rendering exceeds its decoded resource byte limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, headers, body))
}
