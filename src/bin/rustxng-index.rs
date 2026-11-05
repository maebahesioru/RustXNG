//! Crawl a seed list into the local index, and query it from the terminal.
//!
//!   rustxng-index crawl --seeds seeds.txt --out ./index-data --max-pages 2000
//!       [--sitemaps] [--within-seed-path] [--skip-indexed] [--max-per-host N] [--workers N]
//!   rustxng-index search --index ./index-data "tokio runtime"
//!   rustxng-index stats --index ./index-data
//!
//! Requests stay on the seed sites, obey robots and page directives, and share
//! per-host pacing. Downloads, pending URLs, and sitemap traversal are bounded.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rustxng::crawl::{self, Directives};
use rustxng::localindex::{LocalIndex, Page, has_cjk};
use rustxng::robots::Robots;
use scraper::{Html, Selector};

/// Skip files that are not pages.
const SKIP_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".css", ".js", ".json", ".zip",
    ".gz", ".tar", ".pdf", ".mp4", ".mp3", ".woff", ".woff2", ".ttf", ".xml", ".rss", ".txt",
];

/// Decoded body bytes read from one page.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Most redirects followed for one request.
const MAX_REDIRECTS: usize = 5;
const ROBOTS_MAX_BYTES: usize = 512 * 1024;
const SITEMAP_MAX_BYTES: usize = 10 * 1024 * 1024;
const SITEMAP_MAX_LOCATIONS: usize = 50_000;
/// Longest pause after a 429/503 without a usable `Retry-After`.
const DEFAULT_BACKOFF: Duration = Duration::from_secs(30);
/// Longest `Retry-After` we are willing to honour.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().cloned() else {
        eprintln!("usage: rustxng-index <crawl|search|stats> [options]");
        std::process::exit(2);
    };
    let options = Options::parse(&args[1..]);
    let result = match command.as_str() {
        "crawl" => crawl(&options),
        "search" => search(&options),
        "stats" => stats(&options),
        other => {
            eprintln!("unknown command: {other}");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

/// Long form flags, kept tiny so the binary has no extra dependencies.
struct Options {
    seeds: Option<PathBuf>,
    index: PathBuf,
    max_pages: usize,
    max_depth: usize,
    /// Pause between two requests to one host (a host's `Crawl-delay` wins when
    /// it is longer).
    delay_ms: u64,
    workers: usize,
    use_sitemaps: bool,
    /// Stay under the directory of the seed URLs (per host).
    within_seed_path: bool,
    /// Do not download pages the index already holds.
    skip_indexed: bool,
    /// Most pages taken from one host, so no single site fills the index.
    max_per_host: usize,
    max_sitemaps: usize,
    max_queue: usize,
    /// Mail address or URL where the crawled sites can reach us.
    contact: String,
    limit: usize,
    query: Vec<String>,
}

impl Options {
    fn parse(args: &[String]) -> Self {
        let mut options = Self {
            seeds: None,
            index: PathBuf::from("index-data"),
            max_pages: 500,
            max_depth: 2,
            delay_ms: 1000,
            workers: 4,
            use_sitemaps: false,
            within_seed_path: false,
            skip_indexed: false,
            max_per_host: 2000,
            max_sitemaps: 64,
            max_queue: 10_000,
            contact: String::new(),
            limit: 10,
            query: Vec::new(),
        };
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            let mut next = || {
                index += 1;
                args.get(index).cloned()
            };
            match arg.as_str() {
                "--seeds" => options.seeds = next().map(PathBuf::from),
                "--index" | "--out" => {
                    if let Some(value) = next() {
                        options.index = PathBuf::from(value);
                    }
                }
                "--max-pages" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.max_pages = value;
                    }
                }
                "--max-depth" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.max_depth = value;
                    }
                }
                "--delay-ms" => {
                    if let Some(value) = next().and_then(|v| v.parse::<u64>().ok()) {
                        options.delay_ms = value;
                    }
                }
                "--max-per-host" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.max_per_host = value.max(1);
                    }
                }
                "--max-sitemaps" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.max_sitemaps = value;
                    }
                }
                "--max-queue" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.max_queue = value.max(1);
                    }
                }
                "--contact" => {
                    if let Some(value) = next() {
                        options.contact = value;
                    }
                }
                "--sitemaps" => options.use_sitemaps = true,
                "--within-seed-path" => options.within_seed_path = true,
                "--skip-indexed" => options.skip_indexed = true,
                "--workers" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.workers = value.clamp(1, 32);
                    }
                }
                "--limit" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.limit = value;
                    }
                }
                other if !other.starts_with("--") => options.query.push(other.to_string()),
                _ => {}
            }
            index += 1;
        }
        options
    }
}

fn search(options: &Options) -> Result<(), String> {
    let index = LocalIndex::open(&options.index)?;
    let query = options.query.join(" ");
    if query.is_empty() {
        return Err("no query given".to_string());
    }
    let started = Instant::now();
    let hits = index.search(&query, options.limit)?;
    let elapsed = started.elapsed();
    println!(
        "{} hits in {:.1} ms (index has {} documents)",
        hits.len(),
        elapsed.as_secs_f64() * 1000.0,
        index.document_count()
    );
    for (rank, hit) in hits.iter().enumerate() {
        println!("{:>3}. {:.3}  {}", rank + 1, hit.score, hit.url);
        if !hit.title.is_empty() {
            println!("      {}", hit.title);
        }
    }
    Ok(())
}

fn stats(options: &Options) -> Result<(), String> {
    let index = LocalIndex::open(&options.index)?;
    println!("documents: {}", index.document_count());
    Ok(())
}

/// The title of a page, falling back to the first heading.
fn page_title(html: &Html) -> String {
    for selector in ["title", "h1"] {
        if let Ok(parsed) = Selector::parse(selector) {
            if let Some(element) = html.select(&parsed).next() {
                let text = element.text().collect::<Vec<_>>().join(" ");
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !text.is_empty() {
                    return text;
                }
            }
        }
    }
    String::new()
}

/// Absolute, normalised http(s) links of the page that may be followed.
fn links(html: &Html, base: &url::Url) -> Vec<String> {
    let Ok(selector) = Selector::parse("a[href]") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for element in html.select(&selector) {
        if crawl::rel_forbids_following(element.value().attr("rel").unwrap_or("")) {
            continue;
        }
        let Some(href) = element.value().attr("href") else {
            continue;
        };
        let Ok(resolved) = base.join(href) else {
            continue;
        };
        let path = resolved.path().to_lowercase();
        if SKIP_EXTENSIONS
            .iter()
            .any(|extension| path.ends_with(extension))
        {
            continue;
        }
        if let Some(clean) = crawl::normalize_url(resolved.as_str()) {
            found.push(clean);
        }
    }
    found
}

/// `host` or `host:port`: the unit robots.txt and politeness apply to.
fn authority(url: &url::Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// `www.example.com` and `example.com` count as the same site.
fn bare_host(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

fn build_client(contact: &str) -> Result<reqwest::blocking::Client, String> {
    rustxng::fetch::public_blocking_client(&crawl::user_agent(contact))
}

fn same_site(source: &url::Url, target: &url::Url) -> bool {
    source.host_str().map(bare_host) == target.host_str().map(bare_host)
        && source.port() == target.port()
        && (source.scheme() == target.scheme()
            || (source.scheme() == "http" && target.scheme() == "https"))
}

/// Read at most `limit` bytes of a response body.
fn read_capped(response: reqwest::blocking::Response, limit: usize) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    response
        .take(limit as u64 + 1)
        .read_to_end(&mut body)
        .ok()?;
    (body.len() <= limit).then_some(body)
}

/// What a worker hands back to the writer.
struct Fetched {
    /// The canonical address the page is indexed under.
    url: String,
    aliases: Vec<String>,
    title: String,
    body: String,
    lang: String,
    links: Vec<String>,
    depth: usize,
}

/// The outcome of asking for one page.
enum Fetch {
    Page(Fetched),
    /// Not indexed, but its links may still be worth following.
    Links(Vec<String>),
    Remove {
        urls: Vec<String>,
        links: Vec<String>,
    },
    /// The host asked us to slow down.
    Backoff,
    Skipped,
}

enum IndexUpdate {
    Page(Fetched),
    Remove(Vec<String>),
}

struct InFlight<'a>(&'a AtomicUsize);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Resource {
    Robots,
    Sitemap,
    Page,
}

#[derive(Debug)]
enum FetchError {
    RobotsBlocked,
    Backoff,
    Failed,
}

/// Counters printed at the end of a crawl.
#[derive(Default)]
struct Counters {
    robots_blocked: AtomicUsize,
    noindex: AtomicUsize,
    duplicates: AtomicUsize,
    failed: AtomicUsize,
    backoffs: AtomicUsize,
    host_capped: AtomicUsize,
    queue_capped: AtomicUsize,
}

/// The `host[:port]` a URL belongs to.
fn host_key(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| authority(&parsed))
        .unwrap_or_default()
}

/// Pending URLs grouped by host.
///
/// One FIFO queue lets a crowded host (a documentation site with thousands of
/// links) fill the front, so every worker ends up waiting for that host's
/// politeness delay while the other hosts starve. Grouping by host and always
/// serving a host that is ready keeps all workers busy.
#[derive(Default)]
struct Frontier {
    hosts: HashMap<String, VecDeque<(String, usize)>>,
    /// Hosts with pending URLs, in round-robin order.
    order: VecDeque<String>,
    len: usize,
    scheduled: HashSet<String>,
}

impl Frontier {
    fn push(&mut self, url: String, depth: usize) {
        if !self.scheduled.insert(url.clone()) {
            return;
        }
        let host = host_key(&url);
        let pending = self.hosts.entry(host.clone()).or_default();
        if pending.is_empty() {
            self.order.push_back(host);
        }
        pending.push_back((url, depth));
        self.len += 1;
    }

    /// Take a URL of the first host whose next slot has come; when every host
    /// is still cooling down, the one that becomes ready first.
    fn pop(&mut self, slots: &HashMap<String, Instant>) -> Option<(String, usize)> {
        let now = Instant::now();
        let position = self
            .order
            .iter()
            .position(|host| slots.get(host).is_none_or(|slot| *slot <= now))
            .or_else(|| {
                (0..self.order.len()).min_by_key(|index| slots.get(&self.order[*index]).copied())
            })?;
        let host = self.order.remove(position)?;
        let pending = self.hosts.get_mut(&host)?;
        let item = pending.pop_front();
        if pending.is_empty() {
            self.hosts.remove(&host);
        } else {
            self.order.push_back(host);
        }
        self.len -= usize::from(item.is_some());
        item
    }
}

/// Shared crawl state. Workers pull URLs, the main thread writes documents.
struct Crawl {
    queue: Mutex<Frontier>,
    robots: Mutex<HashMap<String, Arc<OnceLock<Arc<Robots>>>>>,
    /// Earliest time the next request to a host may start.
    next_slot: Mutex<HashMap<String, Instant>>,
    wake: Condvar,
    stopped: AtomicBool,
    /// Pages claimed per host, against `max_per_host`.
    per_host: Mutex<HashMap<String, usize>>,
    in_flight: AtomicUsize,
    counters: Counters,
    /// With `--within-seed-path`: the path prefixes each host may be crawled
    /// under (the directories of its seeds). `None` crawls whole hosts.
    scope: Option<HashMap<String, Vec<String>>>,
    /// URLs taken from sitemaps: the only ones `--skip-indexed` may skip,
    /// because their links are not needed to discover the rest of the site.
    from_sitemap: Mutex<HashSet<String>>,
    max_queue: usize,
}

impl Crawl {
    fn new() -> Self {
        Self {
            queue: Mutex::new(Frontier::default()),
            robots: Mutex::new(HashMap::new()),
            next_slot: Mutex::new(HashMap::new()),
            wake: Condvar::new(),
            stopped: AtomicBool::new(false),
            per_host: Mutex::new(HashMap::new()),
            in_flight: AtomicUsize::new(0),
            counters: Counters::default(),
            scope: None,
            from_sitemap: Mutex::new(HashSet::new()),
            max_queue: 10_000,
        }
    }

    /// Give back a per-host slot claimed for a page that was not fetched.
    fn refund_host(&self, url: &str) {
        if let Ok(mut counts) = self.per_host.lock() {
            if let Some(count) = counts.get_mut(&host_key(url)) {
                *count = count.saturating_sub(1);
            }
        }
    }

    /// Whether `url` lies under one of its host's seed directories.
    fn in_scope(&self, url: &str) -> bool {
        let Some(scope) = &self.scope else {
            return true;
        };
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        scope
            .get(bare_host(&host_key(url)))
            .is_some_and(|prefixes| {
                prefixes
                    .iter()
                    .any(|prefix| parsed.path().starts_with(prefix.as_str()))
            })
    }

    fn claim(&self, max_per_host: usize) -> Option<(String, usize)> {
        let mut queue = self.queue.lock().ok()?;
        loop {
            if self.stopped.load(Ordering::Acquire) {
                return None;
            }
            let popped = {
                let slots = self.next_slot.lock().ok()?;
                queue.pop(&slots)
            };
            let (url, depth) = popped?;
            let within_cap = self
                .per_host
                .lock()
                .map(|mut counts| {
                    let count = counts.entry(host_key(&url)).or_insert(0);
                    *count += 1;
                    *count <= max_per_host
                })
                .unwrap_or(true);
            if within_cap {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                return Some((url, depth));
            }
            self.counters.host_capped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Wait until this host may be asked again, so several workers still keep
    /// one request per delay per host.
    fn wait_for_host(&self, host: &str, delay: Duration) -> bool {
        let mut slots = self.next_slot.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if self.stopped.load(Ordering::Acquire) {
                return false;
            }
            let now = Instant::now();
            let slot = slots.entry(host.to_string()).or_insert(now);
            if *slot <= now {
                let Some(next) = now.checked_add(delay) else {
                    return false;
                };
                *slot = next;
                return true;
            }
            let pause = slot.saturating_duration_since(now);
            (slots, _) = self
                .wake
                .wait_timeout(slots, pause)
                .unwrap_or_else(|p| p.into_inner());
        }
    }

    fn stop(&self) {
        let _slots = self.next_slot.lock().unwrap_or_else(|p| p.into_inner());
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_all();
    }

    /// Keep every worker away from this host for a while.
    fn back_off(&self, host: &str, pause: Duration) {
        if let Ok(mut slots) = self.next_slot.lock() {
            let until = Instant::now() + pause;
            let slot = slots.entry(host.to_string()).or_insert(until);
            if *slot < until {
                *slot = until;
            }
        }
    }

    /// The robots.txt policy of a host, fetched once.
    fn robots_for(
        &self,
        client: &reqwest::blocking::Client,
        url: &url::Url,
        delay: Duration,
    ) -> Arc<Robots> {
        let cell = {
            let mut policies = self.robots.lock().unwrap_or_else(|p| p.into_inner());
            Arc::clone(
                policies
                    .entry(url.origin().ascii_serialization())
                    .or_default(),
            )
        };
        Arc::clone(cell.get_or_init(|| {
            let mut robots_url = url.clone();
            robots_url.set_path("/robots.txt");
            robots_url.set_query(None);
            robots_url.set_fragment(None);
            let policy = match self.request(client, robots_url, delay, Resource::Robots) {
                Ok(response) => {
                    let status = response.status().as_u16();
                    match read_capped(response, ROBOTS_MAX_BYTES) {
                        Some(body) => Robots::from_response(
                            status,
                            &String::from_utf8_lossy(&body),
                            crawl::PRODUCT,
                        ),
                        None => Robots::deny_all(),
                    }
                }
                Err(_) => Robots::deny_all(),
            };
            Arc::new(policy)
        }))
    }

    fn request(
        &self,
        client: &reqwest::blocking::Client,
        mut url: url::Url,
        delay: Duration,
        resource: Resource,
    ) -> Result<reqwest::blocking::Response, FetchError> {
        let original = url.clone();
        for hop in 0..=MAX_REDIRECTS {
            if !rustxng::fetch::is_fetchable(&url)
                || !same_site(&original, &url)
                || (resource == Resource::Page && !self.in_scope(url.as_str()))
            {
                return Err(FetchError::Failed);
            }
            let host = host_key(url.as_str());
            let spacing = if resource == Resource::Robots {
                delay
            } else {
                let robots = self.robots_for(client, &url, delay);
                let target = &url[url::Position::BeforePath..url::Position::AfterQuery];
                if !robots.allowed(target) {
                    return Err(FetchError::RobotsBlocked);
                }
                robots.crawl_delay().map_or(delay, |own| own.max(delay))
            };
            if !self.wait_for_host(&host, spacing) {
                return Err(FetchError::Failed);
            }
            let response = client
                .get(url.clone())
                .send()
                .map_err(|_| FetchError::Failed)?;
            if matches!(response.status().as_u16(), 429 | 503) {
                let pause = retry_after(&response);
                self.back_off(&host, pause);
                return Err(FetchError::Backoff);
            }
            if !response.status().is_redirection() {
                return Ok(response);
            }
            if hop == MAX_REDIRECTS {
                return Err(FetchError::Failed);
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(FetchError::Failed)?;
            let next = url.join(location).map_err(|_| FetchError::Failed)?;
            let next = crawl::normalize_url(next.as_str()).ok_or(FetchError::Failed)?;
            url = url::Url::parse(&next).map_err(|_| FetchError::Failed)?;
        }
        Err(FetchError::Failed)
    }
}

fn sitemap_urls(
    client: &reqwest::blocking::Client,
    state: &Crawl,
    seed: &url::Url,
    options: &Options,
) -> Vec<String> {
    let delay = Duration::from_millis(options.delay_ms);
    let limit = options.max_per_host.min(options.max_pages);
    let mut pending = VecDeque::new();
    let mut scheduled = HashSet::new();
    let mut pages = HashSet::new();
    let mut out = Vec::new();
    let robots = state.robots_for(client, seed, delay);
    for location in robots.sitemaps() {
        let Some(location) = crawl::normalize_url(location) else {
            continue;
        };
        let Ok(url) = url::Url::parse(&location) else {
            continue;
        };
        if scheduled.len() < options.max_sitemaps
            && same_site(seed, &url)
            && rustxng::fetch::is_fetchable(&url)
            && scheduled.insert(location)
        {
            pending.push_back((url, 0));
        }
    }
    while let Some((url, depth)) = pending.pop_front() {
        if out.len() >= limit {
            break;
        }
        let Ok(response) = state.request(client, url, delay, Resource::Sitemap) else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Some(bytes) = read_capped(response, SITEMAP_MAX_BYTES) else {
            continue;
        };
        let Ok((is_index, urls)) = crawl::sitemap_locations(&bytes, SITEMAP_MAX_LOCATIONS) else {
            continue;
        };
        for location in urls {
            let Ok(url) = url::Url::parse(&location) else {
                continue;
            };
            if !same_site(seed, &url) || !rustxng::fetch::is_fetchable(&url) {
                continue;
            }
            if is_index {
                if depth < 2 && scheduled.len() < options.max_sitemaps && scheduled.insert(location)
                {
                    pending.push_back((url, depth + 1));
                }
            } else if state.in_scope(&location) && pages.insert(location.clone()) {
                out.push(location);
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    out
}

/// The directory of a seed URL's path: `/3/` for `/3/`, `/doc/` for
/// `/doc/index.html`, `/` for `/docs.html`.
fn seed_directory(url: &str) -> String {
    let path = url::Url::parse(url)
        .map(|parsed| parsed.path().to_string())
        .unwrap_or_else(|_| "/".to_string());
    match path.rfind('/') {
        Some(end) => path[..=end].to_string(),
        None => "/".to_string(),
    }
}

fn crawl(options: &Options) -> Result<(), String> {
    let client = build_client(&options.contact)?;
    crawl_with_client(options, client)
}

fn crawl_with_client(options: &Options, client: reqwest::blocking::Client) -> Result<(), String> {
    if options.max_pages == 0 {
        return Err("--max-pages must be positive".into());
    }
    let seeds_path = options.seeds.as_ref().ok_or("crawl needs --seeds <file>")?;
    let seeds = std::fs::read_to_string(seeds_path)
        .map_err(|error| format!("read {seeds_path:?}: {error}"))?;
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    for line in seeds.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(url) = crawl::normalize_url(line) {
            let parsed = url::Url::parse(&url).map_err(|error| error.to_string())?;
            if !rustxng::fetch::is_fetchable(&parsed) {
                return Err(format!("seed is not a public HTTP(S) URL: {url}"));
            }
            queue.push_back((url, 0));
        }
    }
    if queue.is_empty() {
        return Err("no seeds".to_string());
    }
    let delay = Duration::from_millis(options.delay_ms);
    let mut state = Crawl::new();
    state.max_queue = options.max_queue;
    if options.within_seed_path {
        let mut scope: HashMap<String, Vec<String>> = HashMap::new();
        for (url, _) in &queue {
            scope
                .entry(bare_host(&host_key(url)).to_string())
                .or_default()
                .push(seed_directory(url));
        }
        state.scope = Some(scope);
    }
    let state = Arc::new(state);

    if options.use_sitemaps {
        let mut expanded = Vec::new();
        let mut asked = HashSet::new();
        for (url, _) in &queue {
            let Ok(parsed) = url::Url::parse(url) else {
                continue;
            };
            if !asked.insert(parsed.origin()) {
                continue;
            }
            let mut from_host = sitemap_urls(&client, &state, &parsed, options);
            from_host.truncate(options.max_pages.saturating_sub(expanded.len()));
            println!("sitemaps of {}: {} urls", host_key(url), from_host.len());
            expanded.extend(from_host);
            if expanded.len() >= options.max_pages {
                break;
            }
        }
        if let Ok(mut listed) = state.from_sitemap.lock() {
            listed.extend(expanded.iter().cloned());
        }
        queue.extend(expanded.into_iter().map(|url| (url, 0)));
    }
    for (url, depth) in queue {
        queue_links(&state, vec![url], depth);
    }

    let index = LocalIndex::open_for_write(&options.index)?;
    let crawl = state;
    let (sender, receiver) = std::sync::mpsc::sync_channel(options.workers);
    let max_depth = options.max_depth;
    let max_per_host = options.max_per_host;
    // `--skip-indexed`: a resumed crawl does not download pages it already
    // holds (their links are then not followed; sitemaps cover that).
    let known = if options.skip_indexed {
        Some(Arc::new(LocalIndex::open(&options.index)?))
    } else {
        None
    };

    let mut workers = Vec::new();
    for _ in 0..options.workers {
        let crawl = Arc::clone(&crawl);
        let known = known.clone();
        let sender = sender.clone();
        let client = client.clone();
        workers.push(std::thread::spawn(move || {
            loop {
                if crawl.stopped.load(Ordering::Acquire) {
                    return;
                }
                let Some((url, depth)) = crawl.claim(max_per_host) else {
                    if crawl.in_flight.load(Ordering::SeqCst) == 0 {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                };
                let _active = InFlight(&crawl.in_flight);
                let listed = crawl
                    .from_sitemap
                    .lock()
                    .map(|listed| listed.contains(&url))
                    .unwrap_or(false);
                if listed && known.as_ref().is_some_and(|index| index.has_url(&url)) {
                    crawl.refund_host(&url);
                    continue;
                }
                let outcome = fetch(&client, &url, depth, max_depth, &crawl, delay);
                let update = match outcome {
                    Fetch::Page(mut page) => {
                        queue_links(&crawl, std::mem::take(&mut page.links), page.depth + 1);
                        Some(IndexUpdate::Page(page))
                    }
                    Fetch::Links(found) => {
                        queue_links(&crawl, found, depth + 1);
                        None
                    }
                    Fetch::Remove { urls, links } => {
                        queue_links(&crawl, links, depth + 1);
                        Some(IndexUpdate::Remove(urls))
                    }
                    Fetch::Backoff => {
                        crawl.counters.backoffs.fetch_add(1, Ordering::Relaxed);
                        None
                    }
                    Fetch::Skipped => None,
                };
                if let Some(update) = update {
                    if sender.send(update).is_err() {
                        return;
                    }
                }
            }
        }));
    }
    drop(sender);

    let mut indexed = 0;
    let writing = (|| -> Result<(), String> {
        let mut fingerprints = HashSet::new();
        let mut content_hashes = HashMap::new();
        for update in &receiver {
            let (page, removals) = match update {
                IndexUpdate::Page(mut page) => {
                    let mut aliases = std::mem::take(&mut page.aliases);
                    aliases.retain(|url| url != &page.url);
                    (Some(page), aliases)
                }
                IndexUpdate::Remove(urls) => (None, urls),
            };
            for url in removals {
                if let Some(hash) = content_hashes.remove(&url) {
                    fingerprints.remove(&hash);
                }
                index.remove(&url)?;
            }
            let Some(page) = page else {
                continue;
            };
            if let Some(hash) = content_hashes.remove(&page.url) {
                fingerprints.remove(&hash);
            }
            let fingerprint = crawl::content_fingerprint(&page.body);
            if !fingerprints.insert(fingerprint) {
                index.remove(&page.url)?;
                crawl.counters.duplicates.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            content_hashes.insert(page.url.clone(), fingerprint);
            index.add(&Page {
                url: &page.url,
                title: &page.title,
                body: &page.body,
                lang: &page.lang,
                snippet: "",
            })?;
            indexed += 1;
            if indexed % 50 == 0 {
                index.commit()?;
                let queued = crawl.queue.lock().map(|q| q.len).unwrap_or(0);
                println!("indexed {indexed} pages (queue {queued})");
                let _ = std::io::stdout().flush();
            }
            if indexed >= options.max_pages {
                break;
            }
        }
        index.commit()
    })();
    crawl.stop();
    drop(receiver);
    let mut worker_failed = false;
    for worker in workers {
        worker_failed |= worker.join().is_err();
    }
    if worker_failed {
        return Err("crawler worker panicked".into());
    }
    writing?;

    let counters = &crawl.counters;
    println!(
        "crawled {indexed} pages, index now has {} documents",
        index.document_count()
    );
    println!(
        "skipped: {} by robots.txt, {} noindex, {} duplicate text, {} over the per-host cap, {} over the queue cap; {} failed, {} back-offs",
        counters.robots_blocked.load(Ordering::Relaxed),
        counters.noindex.load(Ordering::Relaxed),
        counters.duplicates.load(Ordering::Relaxed),
        counters.host_capped.load(Ordering::Relaxed),
        counters.queue_capped.load(Ordering::Relaxed),
        counters.failed.load(Ordering::Relaxed),
        counters.backoffs.load(Ordering::Relaxed),
    );
    Ok(())
}

fn queue_links(crawl: &Crawl, links: Vec<String>, depth: usize) {
    if let Ok(mut queue) = crawl.queue.lock() {
        for link in links {
            if !crawl.in_scope(&link) || queue.scheduled.contains(&link) {
                continue;
            }
            if queue.len >= crawl.max_queue {
                crawl.counters.queue_capped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            queue.push(link, depth);
        }
    }
}

/// How long to stay away after a 429/503, from `Retry-After` when it is given
/// in seconds.
fn retry_after(response: &reqwest::blocking::Response) -> Duration {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_BACKOFF)
        .min(MAX_BACKOFF)
}

/// The language of a page: its `lang` attribute, else a script heuristic.
fn page_language(html: &Html, title: &str, body: &str) -> String {
    if let Ok(selector) = Selector::parse("html[lang]") {
        if let Some(lang) = html
            .select(&selector)
            .next()
            .and_then(|element| element.value().attr("lang"))
        {
            let primary = lang.split(['-', '_']).next().unwrap_or("").to_lowercase();
            if (2..=3).contains(&primary.len()) {
                return primary;
            }
        }
    }
    if has_cjk(title) || has_cjk(body) {
        "ja".to_string()
    } else {
        "en".to_string()
    }
}

/// Fetch one page and extract what the index needs.
fn fetch(
    client: &reqwest::blocking::Client,
    url: &str,
    depth: usize,
    max_depth: usize,
    crawl: &Crawl,
    delay: Duration,
) -> Fetch {
    let counters = &crawl.counters;
    let Ok(parsed) = url::Url::parse(url) else {
        counters.failed.fetch_add(1, Ordering::Relaxed);
        return Fetch::Skipped;
    };
    let response = match crawl.request(client, parsed, delay, Resource::Page) {
        Ok(response) => response,
        Err(FetchError::RobotsBlocked) => {
            counters.robots_blocked.fetch_add(1, Ordering::Relaxed);
            return Fetch::Skipped;
        }
        Err(FetchError::Backoff) => return Fetch::Backoff,
        Err(FetchError::Failed) => {
            counters.failed.fetch_add(1, Ordering::Relaxed);
            return Fetch::Skipped;
        }
    };
    let base = response.url().clone();
    let mut aliases = vec![url.to_string()];
    if base.as_str() != url {
        aliases.push(base.to_string());
    }
    if matches!(response.status().as_u16(), 404 | 410) {
        return Fetch::Remove {
            urls: aliases,
            links: Vec::new(),
        };
    }
    if !response.status().is_success() {
        counters.failed.fetch_add(1, Ordering::Relaxed);
        return Fetch::Skipped;
    }
    let mut directives = response
        .headers()
        .get_all("x-robots-tag")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(crawl::header_directives)
        .fold(Directives::default(), |all, one| Directives {
            noindex: all.noindex || one.noindex,
            nofollow: all.nofollow || one.nofollow,
        });
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let media_type = content_type.split(';').next().unwrap_or("").trim();
    if !matches!(media_type, "text/html" | "application/xhtml+xml") {
        return if directives.noindex {
            Fetch::Remove {
                urls: aliases,
                links: Vec::new(),
            }
        } else {
            Fetch::Skipped
        };
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES as u64)
    {
        counters.failed.fetch_add(1, Ordering::Relaxed);
        return if directives.noindex {
            Fetch::Remove {
                urls: aliases,
                links: Vec::new(),
            }
        } else {
            Fetch::Skipped
        };
    }
    let Some(bytes) = read_capped(response, MAX_BODY_BYTES) else {
        counters.failed.fetch_add(1, Ordering::Relaxed);
        return if directives.noindex {
            Fetch::Remove {
                urls: aliases,
                links: Vec::new(),
            }
        } else {
            Fetch::Skipped
        };
    };
    let body = String::from_utf8_lossy(&bytes).into_owned();
    let html = Html::parse_document(&body);
    let meta = crawl::meta_directives(&html);
    directives.noindex |= meta.noindex;
    directives.nofollow |= meta.nofollow;

    let found = if depth < max_depth && !directives.nofollow {
        links(&html, &base)
            .into_iter()
            .filter(|link| {
                url::Url::parse(link)
                    .map(|target| {
                        same_site(&base, &target) && rustxng::fetch::is_fetchable(&target)
                    })
                    .unwrap_or(false)
            })
            .collect()
    } else {
        Vec::new()
    };
    if directives.noindex {
        counters.noindex.fetch_add(1, Ordering::Relaxed);
        return Fetch::Remove {
            urls: aliases,
            links: found,
        };
    }
    let title = page_title(&html);
    let text = rustxng::content::main_text_limited(&html, 20_000);
    if title.is_empty() && text.is_empty() {
        return Fetch::Links(found);
    }
    let canonical = crawl::canonical_url(&html, &base)
        .or_else(|| crawl::normalize_url(base.as_str()))
        .unwrap_or_else(|| base.to_string());
    let lang = page_language(&html, &title, &text);
    Fetch::Page(Fetched {
        url: canonical,
        aliases,
        title,
        body: text,
        lang,
        links: found,
        depth,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_scope_is_the_seed_directory() {
        assert_eq!(seed_directory("https://docs.python.org/3/"), "/3/");
        assert_eq!(seed_directory("https://www.sqlite.org/docs.html"), "/");
        assert_eq!(seed_directory("https://go.dev/doc/index.html"), "/doc/");
        let mut crawl = Crawl::new();
        crawl.scope = Some(HashMap::from([(
            host_key("https://docs.python.org/3/"),
            vec!["/3/".to_string()],
        )]));
        assert!(crawl.in_scope("https://docs.python.org/3/library/re.html"));
        assert!(!crawl.in_scope("https://docs.python.org/ja/3/library/re.html"));
        assert!(!crawl.in_scope("https://example.org/3/"));
    }

    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::{SocketAddr, TcpListener, TcpStream};

    fn mock_client(address: SocketAddr, contact: &str) -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve("example.test", address)
            .resolve("www.example.test", address)
            .user_agent(crawl::user_agent(contact))
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }

    struct TestServer {
        address: SocketAddr,
        base: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn new(handler: impl Fn(&str, &str, &str) -> String + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let base = format!("http://example.test:{}", address.port());
            let requests: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
            let log = Arc::clone(&requests);
            let origin = base.clone();
            let worker = std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(&stream);
                    let mut request = String::new();
                    if reader.read_line(&mut request).unwrap() == 0 {
                        break;
                    }
                    let path = request.split_whitespace().nth(1).unwrap();
                    let mut host = String::new();
                    for header in reader.lines() {
                        let header = header.unwrap();
                        if header.is_empty() {
                            break;
                        }
                        if let Some(value) = header.to_ascii_lowercase().strip_prefix("host:") {
                            host = value.trim().into();
                        }
                    }
                    log.lock().unwrap().push((host.clone(), path.into()));
                    let _ = stream.write_all(handler(&host, path, &origin).as_bytes());
                }
            });
            Self {
                address,
                base,
                requests,
                worker: Some(worker),
            }
        }

        fn client(&self) -> reqwest::blocking::Client {
            mock_client(self.address, "")
        }

        fn url(&self, path: &str) -> url::Url {
            url::Url::parse(&format!("{}{path}", self.base)).unwrap()
        }

        fn paths(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|(_, path)| path.clone())
                .collect()
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            drop(TcpStream::connect(self.address).unwrap());
            let result = self.worker.take().unwrap().join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }

    fn reply(status: u16, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn redirects_check_robots_before_reading_the_target() {
        let server = TestServer::new(|_, path, _| match path {
            "/robots.txt" => reply(200, "", "User-agent: *\nDisallow: /private/*$\n"),
            "/start" => reply(302, "Location: /private/secret\r\n", ""),
            _ => panic!("forbidden request: {path}"),
        });
        let state = Crawl::new();
        assert!(matches!(
            state.request(
                &server.client(),
                server.url("/start"),
                Duration::ZERO,
                Resource::Page
            ),
            Err(FetchError::RobotsBlocked)
        ));
        assert_eq!(server.paths(), ["/robots.txt", "/start"]);
    }

    #[test]
    fn redirects_cannot_switch_ports_or_reach_literal_private_addresses() {
        let private = TcpListener::bind("127.0.0.1:0").unwrap();
        private.set_nonblocking(true).unwrap();
        let address = private.local_addr().unwrap();
        let server = TestServer::new(move |_, path, _| match path {
            "/robots.txt" => reply(200, "", ""),
            "/port" => reply(
                302,
                &format!(
                    "Location: http://example.test:{}/secret\r\n",
                    address.port()
                ),
                "",
            ),
            "/private" => reply(302, &format!("Location: http://{address}/secret\r\n"), ""),
            _ => panic!("unexpected request: {path}"),
        });
        let state = Crawl::new();
        for path in ["/port", "/private"] {
            assert!(matches!(
                state.request(
                    &server.client(),
                    server.url(path),
                    Duration::ZERO,
                    Resource::Page
                ),
                Err(FetchError::Failed)
            ));
        }
        assert_eq!(
            private.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(server.paths(), ["/robots.txt", "/port", "/private"]);
    }

    #[test]
    fn a_www_redirect_uses_the_new_origins_robots_policy() {
        let server = TestServer::new(|host, path, base| match path {
            "/robots.txt" if host.starts_with("www.") => {
                reply(200, "", "User-agent: *\nDisallow: /\n")
            }
            "/robots.txt" => reply(200, "", ""),
            "/start" => reply(
                302,
                &format!(
                    "Location: {}/final\r\n",
                    base.replace("example.test", "www.example.test")
                ),
                "",
            ),
            _ => panic!("forbidden request: {path}"),
        });
        assert!(matches!(
            Crawl::new().request(
                &server.client(),
                server.url("/start"),
                Duration::ZERO,
                Resource::Page
            ),
            Err(FetchError::RobotsBlocked)
        ));
        assert_eq!(server.paths(), ["/robots.txt", "/start", "/robots.txt"]);
    }

    #[test]
    fn redirected_pages_resolve_links_against_the_final_address() {
        let server = TestServer::new(|_, path, _| match path {
            "/robots.txt" => reply(200, "", ""),
            "/old" => reply(302, "Location: /new/\r\n", ""),
            "/new/" => reply(
                200,
                "Content-Type: text/html\r\n",
                "<title>New</title><a href='child'>child</a>",
            ),
            _ => panic!("unexpected request: {path}"),
        });
        let state = Crawl::new();
        let Fetch::Page(page) = fetch(
            &server.client(),
            server.url("/old").as_str(),
            0,
            1,
            &state,
            Duration::ZERO,
        ) else {
            panic!("redirected page was not indexed");
        };
        assert_eq!(page.url, server.url("/new/").as_str());
        assert_eq!(page.links, [server.url("/new/child").to_string()]);
    }

    #[test]
    fn unreadable_or_oversized_robots_never_becomes_allow_all() {
        for oversized in [false, true] {
            let server = TestServer::new(move |_, path, _| {
                assert_eq!(path, "/robots.txt");
                if oversized {
                    reply(200, "", &"x".repeat(ROBOTS_MAX_BYTES + 1))
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort"
                        .into()
                }
            });
            assert!(matches!(
                Crawl::new().request(
                    &server.client(),
                    server.url("/page"),
                    Duration::ZERO,
                    Resource::Page
                ),
                Err(FetchError::RobotsBlocked)
            ));
            assert_eq!(server.paths(), ["/robots.txt"]);
        }
    }

    #[test]
    fn noindex_headers_remove_entries_even_when_the_body_cannot_be_read() {
        for declared in [100, MAX_BODY_BYTES + 1] {
            let server = TestServer::new(move |_, path, _| {
                if path == "/robots.txt" {
                    return reply(200, "", "");
                }
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nX-Robots-Tag: noindex\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\nshort"
                )
            });
            let outcome = fetch(
                &server.client(),
                server.url("/page").as_str(),
                0,
                1,
                &Crawl::new(),
                Duration::ZERO,
            );
            let Fetch::Remove { urls, links } = outcome else {
                panic!("noindex header was ignored");
            };
            assert_eq!(urls, [server.url("/page").to_string()]);
            assert!(links.is_empty());
        }
    }

    #[test]
    fn sitemaps_cannot_redirect_crawls_to_another_origin() {
        let server = TestServer::new(|_, path, _| {
            assert_eq!(path, "/robots.txt");
            reply(
                200,
                "",
                "Sitemap: http://127.0.0.1/private\nSitemap: https://foreign.test/index.xml\n",
            )
        });
        let mut options = Options::parse(&[]);
        options.delay_ms = 0;
        let urls = sitemap_urls(&server.client(), &Crawl::new(), &server.url("/"), &options);
        assert!(urls.is_empty());
        assert_eq!(server.paths(), ["/robots.txt"]);
    }

    #[test]
    fn sitemap_requests_are_bounded_even_when_no_pages_are_in_scope() {
        let server = TestServer::new(|_, path, base| {
            if path == "/robots.txt" {
                return reply(200, "", &format!("Sitemap: {base}/index.xml\n"));
            }
            let mut xml = format!("<sitemapindex><sitemap><loc>{base}/index.xml</loc></sitemap>");
            for number in 0..100 {
                xml.push_str(&format!(
                    "<sitemap><loc>{base}/map-{number}.xml</loc></sitemap>"
                ));
            }
            xml.push_str("</sitemapindex>");
            reply(200, "", &xml)
        });
        let mut options = Options::parse(&[]);
        options.delay_ms = 0;
        options.max_sitemaps = 4;
        let mut state = Crawl::new();
        state.scope = Some(HashMap::from([(
            host_key(server.url("/").as_str()),
            vec!["/docs/".into()],
        )]));
        assert!(sitemap_urls(&server.client(), &state, &server.url("/docs/"), &options).is_empty());
        let paths = server.paths();
        assert_eq!(
            paths.iter().filter(|path| path.ends_with(".xml")).count(),
            4
        );
        assert_eq!(paths.iter().filter(|path| *path == "/index.xml").count(), 1);
    }

    #[test]
    fn sitemap_page_limits_apply_after_scope_filtering() {
        let server = TestServer::new(|_, path, base| {
            if path == "/robots.txt" {
                return reply(200, "", &format!("Sitemap: {base}/index.xml\n"));
            }
            let mut xml = String::from("<urlset>");
            for number in 0..70 {
                xml.push_str(&format!("<url><loc>{base}/outside/{number}</loc></url>"));
            }
            xml.push_str(&format!("<url><loc>{base}/docs/kept</loc></url></urlset>"));
            reply(200, "", &xml)
        });
        let mut options = Options::parse(&[]);
        options.delay_ms = 0;
        options.max_pages = 1;
        let mut state = Crawl::new();
        state.scope = Some(HashMap::from([(
            host_key(server.url("/").as_str()),
            vec!["/docs/".into()],
        )]));
        assert_eq!(
            sitemap_urls(&server.client(), &state, &server.url("/docs/"), &options),
            [server.url("/docs/kept").to_string()]
        );
    }

    #[test]
    fn the_frontier_bounds_pending_urls_and_deduplicates_cycles() {
        let mut state = Crawl::new();
        state.max_queue = 2;
        queue_links(&state, vec!["https://example.test/a".into(); 100], 0);
        queue_links(
            &state,
            vec![
                "https://example.test/b".into(),
                "https://example.test/c".into(),
            ],
            0,
        );
        assert_eq!(state.queue.lock().unwrap().len, 2);
        assert_eq!(state.counters.queue_capped.load(Ordering::Relaxed), 1);
        let first = state.queue.lock().unwrap().pop(&HashMap::new()).unwrap();
        queue_links(&state, vec![first.0], 1);
        assert_eq!(state.queue.lock().unwrap().len, 1);
    }

    #[test]
    fn shutdown_wakes_workers_waiting_for_backoff() {
        let state = Arc::new(Crawl::new());
        state.back_off("example.test", MAX_BACKOFF);
        let waiting = Arc::clone(&state);
        let worker =
            std::thread::spawn(move || waiting.wait_for_host("example.test", Duration::ZERO));
        state.stop();
        assert!(!worker.join().unwrap());
    }

    #[test]
    fn recrawls_update_changed_pages_and_remove_noindex_gone_and_duplicate_content() {
        let stage = Arc::new(AtomicUsize::new(0));
        let current = Arc::clone(&stage);
        let server = TestServer::new(move |_, path, _| {
            if path == "/robots.txt" {
                return reply(200, "", "");
            }
            let second = current.load(Ordering::SeqCst) == 1;
            let (status, body) = match (path, second) {
                ("/", _) => (
                    200,
                    "<title>Root</title><a href='/changed'>changed</a><a href='/hidden'>hidden</a><a href='/gone'>gone</a><a href='/copy'>copy</a>",
                ),
                ("/changed", false) => (200, "<title>Changing</title><p>obsoletevalue</p>"),
                ("/changed", true) => (200, "<title>Changing</title><p>replacementvalue</p>"),
                ("/hidden", false) => (200, "<title>Hidden</title><p>withdrawnvalue</p>"),
                ("/hidden", true) => (
                    200,
                    "<meta name='robots' content='noindex'><title>Hidden</title><p>withdrawnvalue</p>",
                ),
                ("/gone", false) => (200, "<title>Gone</title><p>deletedvalue</p>"),
                ("/gone", true) => (410, "gone"),
                ("/copy", false) => (200, "<title>Copy</title><p>stalevalue</p>"),
                ("/copy", true) => (200, "<title>Copy</title><p>replacementvalue</p>"),
                _ => panic!("unexpected request: {path}"),
            };
            reply(status, "Content-Type: text/html\r\n", body)
        });
        let directory = tempfile::tempdir().unwrap();
        let seeds = directory.path().join("seeds.txt");
        std::fs::write(&seeds, server.url("/").as_str()).unwrap();
        let mut options = Options::parse(&[]);
        options.seeds = Some(seeds);
        options.index = directory.path().join("index");
        options.delay_ms = 0;
        options.workers = 1;
        crawl_with_client(&options, server.client()).unwrap();
        assert_eq!(
            LocalIndex::open(&options.index).unwrap().document_count(),
            5
        );
        stage.store(1, Ordering::SeqCst);
        crawl_with_client(&options, server.client()).unwrap();
        let index = LocalIndex::open(&options.index).unwrap();
        assert_eq!(index.document_count(), 2);
        for old in [
            "obsoletevalue",
            "withdrawnvalue",
            "deletedvalue",
            "stalevalue",
        ] {
            assert!(
                index.search(old, 10).unwrap().is_empty(),
                "stale text: {old}"
            );
        }
        let hits = index.search("replacementvalue", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, server.url("/changed").as_str());
    }

    #[test]
    fn literal_private_seeds_never_open_a_connection() {
        let server = TestServer::new(|_, path, _| panic!("unexpected request: {path}"));
        let directory = tempfile::tempdir().unwrap();
        let seeds = directory.path().join("seeds.txt");
        std::fs::write(&seeds, format!("http://{}/", server.address)).unwrap();
        let mut options = Options::parse(&[]);
        options.seeds = Some(seeds);
        options.index = directory.path().join("index");
        let error = crawl_with_client(&options, server.client()).unwrap_err();
        assert!(error.contains("not a public HTTP(S) URL"), "{error}");
        assert!(server.paths().is_empty());
    }

    #[test]
    fn recrawling_a_redirect_removes_its_old_index_entry() {
        let moved = Arc::new(AtomicBool::new(false));
        let current = Arc::clone(&moved);
        let server = TestServer::new(move |_, path, _| match path {
            "/robots.txt" => reply(200, "", ""),
            "/old" if current.load(Ordering::SeqCst) => reply(302, "Location: /new\r\n", ""),
            "/old" => reply(
                200,
                "Content-Type: text/html\r\n",
                "<title>Old</title><p>oldvalue</p>",
            ),
            "/new" => reply(
                200,
                "Content-Type: text/html\r\n",
                "<title>New</title><p>newvalue</p>",
            ),
            _ => panic!("unexpected request: {path}"),
        });
        let directory = tempfile::tempdir().unwrap();
        let seeds = directory.path().join("seeds.txt");
        std::fs::write(&seeds, server.url("/old").as_str()).unwrap();
        let mut options = Options::parse(&[]);
        options.seeds = Some(seeds);
        options.index = directory.path().join("index");
        options.delay_ms = 0;
        options.workers = 1;
        crawl_with_client(&options, server.client()).unwrap();
        moved.store(true, Ordering::SeqCst);
        crawl_with_client(&options, server.client()).unwrap();
        let index = LocalIndex::open(&options.index).unwrap();
        assert_eq!(index.document_count(), 1);
        assert!(index.search("oldvalue", 5).unwrap().is_empty());
        assert_eq!(
            index.search("newvalue", 5).unwrap()[0].url,
            server.url("/new").as_str()
        );
    }

    #[test]
    fn crawl_follows_links_from_new_and_indexed_pages() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!("http://example.test:{}/", address.port());
        let directory = tempfile::tempdir().unwrap();
        let seeds = directory.path().join("seeds.txt");
        std::fs::write(&seeds, &url).unwrap();
        let mut cases = Vec::new();
        for name in ["fresh", "resumed"] {
            let mut options = Options::parse(&[]);
            options.seeds = Some(seeds.clone());
            options.index = directory.path().join(name);
            options.workers = 1;
            options.delay_ms = 0;
            options.max_pages = 3;
            options.max_depth = 2;
            if name == "resumed" {
                let index = LocalIndex::open_for_write(&options.index).unwrap();
                index
                    .add(&Page {
                        url: &url,
                        title: "Root",
                        body: "Child",
                        lang: "en",
                        snippet: "",
                    })
                    .unwrap();
                index.commit().unwrap();
            }
            cases.push(options);
        }

        let server = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut request = String::new();
                if reader.read_line(&mut request).unwrap() == 0 {
                    break;
                }
                for header in reader.lines() {
                    if header.unwrap().is_empty() {
                        break;
                    }
                }
                let body = match request.split_whitespace().nth(1).unwrap() {
                    "/" => "<title>Root</title><a href='/child'>Child</a>",
                    "/child" => "<title>Child</title><a href='/grandchild'>Grandchild</a>",
                    "/grandchild" => "<title>Grandchild</title><p>The last page</p>",
                    _ => "",
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let results: Vec<_> = cases
            .iter()
            .map(|options| {
                crawl_with_client(options, mock_client(address, &options.contact))?;
                LocalIndex::open(&options.index).map(|index| index.document_count())
            })
            .collect();
        drop(TcpStream::connect(address).unwrap());
        server.join().unwrap();
        assert_eq!(results, vec![Ok(3), Ok(3)]);
    }

    #[test]
    fn crawl_obeys_robots_noindex_nofollow_and_announces_itself() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let seeds = directory.path().join("seeds.txt");
        std::fs::write(&seeds, format!("http://example.test:{}/", address.port())).unwrap();
        let mut options = Options::parse(&[]);
        options.seeds = Some(seeds);
        options.index = directory.path().join("index");
        options.workers = 1;
        options.delay_ms = 0;
        options.max_pages = 20;
        options.max_depth = 3;
        options.contact = "ops@example.org".into();

        let requests: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let log = Arc::clone(&requests);
        let server = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut request = String::new();
                if reader.read_line(&mut request).unwrap() == 0 {
                    break;
                }
                let path = request.split_whitespace().nth(1).unwrap().to_string();
                let mut agent = String::new();
                for header in reader.lines() {
                    let header = header.unwrap();
                    if header.is_empty() {
                        break;
                    }
                    if let Some(value) = header.to_lowercase().strip_prefix("user-agent:") {
                        agent = value.trim().to_string();
                    }
                }
                log.lock().unwrap().push((path.clone(), agent));
                let (kind, extra, body) = match path.as_str() {
                    "/robots.txt" => (
                        "text/plain",
                        "",
                        "User-agent: *\nDisallow: /private\n".to_string(),
                    ),
                    "/" => (
                        "text/html",
                        "",
                        "<title>Root</title><a href='/ok'>ok</a><a href='/private/secret'>p</a>\
                         <a href='/hidden'>h</a><a rel='nofollow' href='/nofollowed'>n</a>\
                         <a href='/header-noindex'>x</a>"
                            .to_string(),
                    ),
                    "/ok" => (
                        "text/html",
                        "",
                        "<title>Fine</title><p>fine page</p>".to_string(),
                    ),
                    "/hidden" => (
                        "text/html",
                        "",
                        "<meta name='robots' content='noindex'><title>Hidden</title>\
                         <a href='/after-hidden'>a</a>"
                            .to_string(),
                    ),
                    "/after-hidden" => (
                        "text/html",
                        "",
                        "<title>After</title><p>reachable through a noindex page</p>".to_string(),
                    ),
                    "/header-noindex" => (
                        "text/html",
                        "X-Robots-Tag: noindex\r\n",
                        "<title>Header</title><p>never indexed</p>".to_string(),
                    ),
                    _ => ("text/html", "", String::new()),
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });

        crawl_with_client(&options, mock_client(address, &options.contact)).unwrap();
        drop(TcpStream::connect(address).unwrap());
        server.join().unwrap();

        let index = LocalIndex::open(&options.index).unwrap();
        let mut found: Vec<String> = ["Root", "Fine", "After", "Hidden", "Header", "never"]
            .iter()
            .flat_map(|word| index.search(word, 5).unwrap())
            .map(|hit| hit.title)
            .collect();
        found.sort();
        found.dedup();
        assert_eq!(found, vec!["After", "Fine", "Root"], "indexed titles");

        let requests = requests.lock().unwrap();
        let paths: Vec<&str> = requests.iter().map(|(path, _)| path.as_str()).collect();
        assert!(
            !paths.contains(&"/private/secret"),
            "robots.txt ignored: {paths:?}"
        );
        assert!(
            !paths.contains(&"/nofollowed"),
            "rel=nofollow ignored: {paths:?}"
        );
        assert!(
            paths.contains(&"/after-hidden"),
            "links of a noindex page are followed"
        );
        for (path, agent) in requests.iter() {
            assert!(
                agent.starts_with("rustxng-index/") && agent.contains("ops@example.org"),
                "{path} was fetched as {agent:?}"
            );
        }
    }

    #[test]
    fn a_ready_host_is_served_before_a_cooling_crowded_one() {
        let mut frontier = Frontier::default();
        for page in 0..5 {
            frontier.push(format!("https://busy.example/{page}"), 0);
        }
        frontier.push("https://quiet.example/".to_string(), 0);
        assert_eq!(frontier.len, 6);
        let mut slots = HashMap::new();
        slots.insert(
            "busy.example".to_string(),
            Instant::now() + Duration::from_secs(60),
        );
        let (url, _) = frontier.pop(&slots).unwrap();
        assert_eq!(url, "https://quiet.example/");
        // Only the cooling host is left: it is still served, earliest first.
        let (url, _) = frontier.pop(&slots).unwrap();
        assert_eq!(url, "https://busy.example/0");
        assert_eq!(frontier.len, 4);
    }

    #[test]
    fn ready_hosts_take_turns() {
        let mut frontier = Frontier::default();
        for page in 0..2 {
            frontier.push(format!("https://a.example/{page}"), 0);
            frontier.push(format!("https://b.example/{page}"), 0);
        }
        let slots = HashMap::new();
        let order: Vec<String> = (0..4).map(|_| frontier.pop(&slots).unwrap().0).collect();
        assert_eq!(
            order,
            [
                "https://a.example/0",
                "https://b.example/0",
                "https://a.example/1",
                "https://b.example/1"
            ]
        );
        assert!(frontier.pop(&slots).is_none());
        assert_eq!(frontier.len, 0);
    }
}
