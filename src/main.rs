//! Binary entry point: `rustxng [--config FILE] [--bind ADDRESS] [--port PORT]`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

use rustxng::aggregator::Searcher;
use rustxng::settings::Settings;
use rustxng::version::version_string;
use rustxng::web::{self, AppState};

const HELP: &str = "rustxng - a Rust rewrite of the SearXNG metasearch engine

USAGE:
    rustxng [OPTIONS]
    rustxng mcp [--config FILE]    Serve MCP tools on stdin/stdout

OPTIONS:
    -c, --config <FILE>    Settings file (default: $SEARXNG_SETTINGS_PATH)
    -b, --bind <ADDRESS>   Address to listen on (default: server.bind_address)
    -p, --port <PORT>      Port to listen on (default: server.port)
    --check-update        Check the latest official release
    --update              Download, verify and install the latest release
    -v, --version          Print the version
    -h, --help             Print this help

ENVIRONMENT:
    SEARXNG_SETTINGS_PATH, SEARXNG_BIND_ADDRESS, SEARXNG_PORT, SEARXNG_SECRET,
    SEARXNG_BASE_URL, SEARXNG_DEBUG, SEARXNG_ENABLE_METRICS, RUST_LOG
";

struct Args {
    mcp: bool,
    config: Option<PathBuf>,
    bind: Option<String>,
    port: Option<u16>,
    update: Option<bool>,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut args = Args {
        mcp: false,
        config: None,
        bind: None,
        port: None,
        update: None,
    };
    let mut argv = std::env::args().skip(1).peekable();
    if argv.peek().map(String::as_str) == Some("mcp") {
        argv.next();
        args.mcp = true;
    }
    while let Some(argument) = argv.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(None);
            }
            "-v" | "--version" => {
                println!("{}", version_string());
                return Ok(None);
            }
            "--update" | "--check-update" => {
                if args.update.is_some() {
                    return Err("choose only one update action".into());
                }
                args.update = Some(argument == "--update");
            }
            "-c" | "--config" => {
                args.config = Some(PathBuf::from(
                    argv.next().ok_or("--config needs a file path")?,
                ));
            }
            "-b" | "--bind" => args.bind = Some(argv.next().ok_or("--bind needs an address")?),
            "-p" | "--port" => {
                let value = argv.next().ok_or("--port needs a number")?;
                args.port = Some(value.parse().map_err(|_| "--port needs a number")?);
            }
            other if other.starts_with("--config=") => {
                args.config = Some(PathBuf::from(&other["--config=".len()..]));
            }
            other if other.starts_with("--port=") => {
                args.port = Some(
                    other["--port=".len()..]
                        .parse()
                        .map_err(|_| "--port needs a number")?,
                );
            }
            other => return Err(format!("unknown argument {other:?} (try --help)")),
        }
    }
    Ok(Some(args))
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };

    if args.mcp && (args.update.is_some() || args.bind.is_some() || args.port.is_some()) {
        eprintln!("error: mcp only accepts --config");
        return ExitCode::from(2);
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,rustxng=debug")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    if let Some(install) = args.update {
        return match rustxng::update::run(install).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        };
    }

    let settings_path = args.config.or_else(Settings::path_from_env);
    let mut settings = match Settings::load(settings_path.as_deref()) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let address = match apply_listener(&mut settings, args.bind, args.port) {
        Ok(address) => address,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    let searcher = match Searcher::new(settings) {
        Ok(searcher) => searcher,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    if args.mcp {
        return match rustxng::mcp::serve_stdio(searcher).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    tracing::info!(engines = searcher.registry.len(), "{}", version_string());

    let state = AppState::new(searcher);
    // Track the last request so an idle sidecar can exit on its own.
    let idle_seconds = idle_timeout_seconds(&state);
    let activity = std::sync::Arc::new(AtomicU64::new(unix_now()));
    let touched = activity.clone();
    let app = web::router(state)
        // A search page is ~320 KB of HTML and a third of it is repeated
        // markup, so it compresses to about a quarter of its size. The layer
        // negotiates from Accept-Encoding and leaves the body untouched when
        // the client does not ask for a compressed response.
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let touched = touched.clone();
                async move {
                    touched.store(unix_now(), Ordering::Relaxed);
                    next.run(request).await
                }
            },
        ));

    let listener = match tokio::net::TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("error: could not listen on {address}: {error}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!("listening on http://{address}");

    if let Err(error) = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_reason(activity, idle_seconds))
    .await
    {
        eprintln!("error: server failed: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Apply the `--bind`/`--port` overrides to `settings.server` and return the
/// listen address.
///
/// The overrides are written back so everything derived from the settings
/// (notably the generated `Settings::base_url` used for feeds, OpenSearch and
/// proxied links) points at the port actually listened on. An explicitly
/// configured `server.base_url` is left untouched: it names the public URL,
/// which may differ from the local listener (reverse proxy, container port
/// mapping).
fn apply_listener(
    settings: &mut Settings,
    bind: Option<String>,
    port: Option<u16>,
) -> Result<SocketAddr, String> {
    if let Some(bind) = bind {
        settings.server.bind_address = bind;
    }
    if let Some(port) = port {
        settings.server.port = port;
    }
    let (bind, port) = (&settings.server.bind_address, settings.server.port);
    format!("{bind}:{port}")
        .parse()
        .map_err(|error| format!("invalid listen address {bind}:{port}: {error}"))
}

/// Seconds since the epoch.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// The idle timeout the settings or RUSTXNG_IDLE_TIMEOUT ask for.
fn idle_timeout_seconds(state: &AppState) -> u64 {
    if let Ok(value) = std::env::var("RUSTXNG_IDLE_TIMEOUT") {
        if let Ok(seconds) = value.trim().parse() {
            return seconds;
        }
    }
    state.searcher.settings.server.idle_timeout
}

/// Resolve when the process should stop: a signal, or an idle timeout.
async fn shutdown_reason(activity: std::sync::Arc<AtomicU64>, idle_seconds: u64) {
    if idle_seconds == 0 {
        shutdown_signal().await;
        return;
    }
    tokio::select! {
        () = shutdown_signal() => {}
        () = async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let idle = unix_now().saturating_sub(activity.load(Ordering::Relaxed));
                if idle >= idle_seconds {
                    tracing::info!(idle, "idle timeout reached, shutting down");
                    return;
                }
            }
        } => {}
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_on(bind: &str, port: u16, base_url: Option<&str>) -> Settings {
        let mut settings = Settings::default();
        settings.server.bind_address = bind.into();
        settings.server.port = port;
        settings.server.base_url = base_url.map(str::to_string);
        settings
    }

    #[test]
    fn cli_port_reaches_the_generated_base_url() {
        let mut settings = settings_on("127.0.0.1", 29483, None);
        let address = apply_listener(&mut settings, None, Some(9000)).unwrap();
        assert_eq!(address, "127.0.0.1:9000".parse::<SocketAddr>().unwrap());
        assert_eq!(settings.base_url(), "http://127.0.0.1:9000");
    }

    #[test]
    fn cli_bind_and_port_reach_the_generated_base_url() {
        let mut settings = settings_on("127.0.0.1", 29483, None);
        let address = apply_listener(&mut settings, Some("0.0.0.0".into()), Some(8123)).unwrap();
        assert_eq!(address, "0.0.0.0:8123".parse::<SocketAddr>().unwrap());
        // A wildcard bind is not a reachable host; the generated URL uses localhost.
        assert_eq!(settings.base_url(), "http://localhost:8123");
    }

    #[test]
    fn explicit_public_base_url_survives_listener_overrides() {
        let public = "https://search.example.org/";
        let mut settings = settings_on("127.0.0.1", 29483, Some(public));
        let address = apply_listener(&mut settings, Some("0.0.0.0".into()), Some(9000)).unwrap();
        assert_eq!(address, "0.0.0.0:9000".parse::<SocketAddr>().unwrap());
        assert_eq!(settings.server.base_url.as_deref(), Some(public));
        assert_eq!(settings.base_url(), public);
    }

    #[test]
    fn without_overrides_the_configured_listener_is_used() {
        let mut settings = settings_on("127.0.0.1", 29483, None);
        let address = apply_listener(&mut settings, None, None).unwrap();
        assert_eq!(address, "127.0.0.1:29483".parse::<SocketAddr>().unwrap());
        assert_eq!(settings.base_url(), "http://127.0.0.1:29483");
    }

    #[test]
    fn invalid_bind_address_is_reported() {
        let mut settings = settings_on("127.0.0.1", 29483, None);
        let error = apply_listener(&mut settings, Some("not an ip".into()), None).unwrap_err();
        assert!(error.contains("not an ip:29483"), "{error}");
    }
}
