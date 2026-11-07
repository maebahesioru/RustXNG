//! Probe: reach youtube through the real Network client.
use std::path::Path;
use std::time::Duration;

use rustxng::engine::EngineRequest;
use rustxng::network::Network;
use rustxng::settings::Settings;

#[tokio::main]
async fn main() {
    let settings = Settings::load(Some(Path::new(
        "/home/maebahesioru/ダウンロード/sx_research/rustxng_cfg/settings.toml",
    )))
    .expect("settings");
    let network = Network::from_settings(&settings).expect("network");
    let request = EngineRequest::get("https://www.youtube.com/results?search_query=rust")
        .cookie("CONSENT", "YES+")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Accept", "text/html,application/xhtml+xml");
    match network.fetch(request, Duration::from_secs(20), None, None).await {
        Ok(response) => println!(
            "ok status={} len={} yt={}",
            response.status,
            response.text().len(),
            response.text().contains("ytInitialData")
        ),
        Err(error) => println!("ERR {error:?}"),
    }
}
