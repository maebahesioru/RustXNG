// SPDX-License-Identifier: AGPL-3.0-or-later
//! Page fetching for `read_page`, safe against server-side request forgery.
//!
//! Search results come from third parties, so a returned URL may point at a
//! private service, a hostname may resolve to one, and a public page may
//! redirect to one. This fetcher therefore:
//!
//! - refuses literal addresses that are not globally routable, IPv4 and IPv6,
//! - resolves hostnames itself and connects only to public addresses (the
//!   checked answers are the ones used, so DNS rebinding cannot swap them),
//! - checks every redirect hop the same way,
//! - stops reading a body after `MAX_BODY_BYTES` decoded bytes, and
//! - parses at most `MAX_CONCURRENT` pages at a time, off the async runtime.
//!
//! It does not use the shared response cache, whose entries may have been
//! fetched without these checks. Extracted text is kept per session instead,
//! so reading a page slice by slice fetches it once.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Decoded body bytes read from one page at most.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Pages fetched and parsed at the same time at most.
const MAX_CONCURRENT: usize = 4;
/// Extracted pages kept per session for paging.
const REMEMBERED_PAGES: usize = 8;
/// Time for connecting, redirects and the whole body.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_REDIRECTS: usize = 5;

/// Whether an address is globally routable (not loopback, private, link
/// local, shared, multicast, documentation or otherwise reserved).
pub fn is_global(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_global_v4(address),
        IpAddr::V6(address) => is_global_v6(address),
    }
}

fn is_global_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..128).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (18..20).contains(&b))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113))
}

fn is_global_v6(address: Ipv6Addr) -> bool {
    if let Some(v4) = address.to_ipv4_mapped() {
        return is_global_v4(v4);
    }
    let segments = address.segments();
    let embedded_v4 = |high: u16, low: u16| {
        Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8)
    };
    match segments[0] {
        // NAT64 (64:ff9b::/96) reaches the embedded IPv4 address.
        0x0064 if segments[1] == 0xff9b && segments[2..6] == [0; 4] => {
            is_global_v4(embedded_v4(segments[6], segments[7]))
        }
        // 6to4 (2002::/16) reaches the embedded IPv4 address.
        0x2002 => is_global_v4(embedded_v4(segments[1], segments[2])),
        // Teredo, benchmarking and other IETF protocol assignments; documentation.
        0x2001 => segments[1] >= 0x0200 && segments[1] != 0x0db8,
        // Only global unicast (2000::/3) is public; this also excludes
        // ::/8 (loopback, unspecified, IPv4-compatible), fc00::/7, fe80::/10
        // and ff00::/8.
        first => (0x2000..0x4000).contains(&first) && first != 0x3fff,
    }
}

/// Whether a URL may be fetched: http(s) and not a literal non-global address.
/// Hostnames are checked when they are resolved.
pub fn is_fetchable(url: &reqwest::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) => is_global_v4(address),
        Some(url::Host::Ipv6(address)) => is_global_v6(address),
        Some(url::Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain != "localhost" && !domain.ends_with(".localhost")
        }
        None => false,
    }
}

struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addresses: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

/// Resolve once, reject the entire answer if any address is non-global, and
/// pass those exact checked addresses to the connector (no second lookup).
struct PublicResolver(Arc<dyn Resolve>);

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let answer = self.0.resolve(name);
        Box::pin(async move {
            let addresses: Vec<SocketAddr> = answer.await?.collect();
            if addresses.is_empty() || addresses.iter().any(|a| !is_global(a.ip())) {
                return Err("DNS answer contains no addresses or a non-public address".into());
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

/// Build the direct-only client shared by untrusted page and media reads.
/// Configured engine endpoints never grant exceptions to this client.
pub(crate) fn public_client(
    outgoing: &crate::settings::Outgoing,
    overrides: &[(String, SocketAddr)],
) -> Result<(reqwest::Client, bool), String> {
    public_client_with_resolver(outgoing, overrides, Arc::new(SystemResolver))
}

fn public_client_with_resolver(
    outgoing: &crate::settings::Outgoing,
    overrides: &[(String, SocketAddr)],
    resolver: Arc<dyn Resolve>,
) -> Result<(reqwest::Client, bool), String> {
    let redirects = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if is_fetchable(attempt.url()) {
            attempt.follow()
        } else {
            attempt.error("redirect to a non-public address")
        }
    });
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(redirects)
        .dns_resolver(Arc::new(PublicResolver(resolver)))
        .timeout(FETCH_TIMEOUT)
        .connect_timeout(Duration::from_secs(4))
        .danger_accept_invalid_certs(!outgoing.verify)
        .user_agent(crate::network::default_user_agent());
    if !outgoing.enable_http2 {
        builder = builder.http1_only();
    }
    // A proxy can resolve names itself and bypass PublicResolver. Disable
    // implicit environment proxies. Explicit proxy configurations fail page
    // reads closed, while search remains available through the proxy.
    let proxy_blocked =
        !outgoing.proxies.is_empty() || outgoing.using_tor_proxy || environment_proxy_configured();
    if let Some(path) = &outgoing.ca_bundle {
        let pem = std::fs::read(path).map_err(|e| format!("cannot read CA bundle: {e}"))?;
        let certificates = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|e| format!("invalid CA bundle: {e}"))?;
        if certificates.is_empty() {
            return Err("CA bundle contains no certificates".into());
        }
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    for (host, address) in overrides {
        builder = builder.resolve(host, *address);
    }
    Ok((
        builder.build().map_err(|error| error.to_string())?,
        proxy_blocked,
    ))
}

/// Inject DNS before the production address check; address overrides are only
/// for an owned redirect entry server and bypass DNS deliberately.
#[cfg(test)]
pub(crate) fn test_public_client(
    outgoing: &crate::settings::Outgoing,
    overrides: &[(String, SocketAddr)],
    resolver: Arc<dyn Resolve>,
) -> reqwest::Client {
    public_client_with_resolver(outgoing, overrides, resolver)
        .unwrap()
        .0
}

/// Extracted pages as (URL, text), oldest first.
type Pages = VecDeque<(String, usize, std::time::Instant, PageContent)>;

enum PageBody {
    Text {
        url: reqwest::Url,
        content: String,
        html_media_type: Option<&'static str>,
    },
    Pdf(Vec<u8>),
}

/// Extracted page text and whether the readable source exceeded the limit.
#[derive(Clone, Debug)]
pub struct PageContent {
    pub text: Arc<str>,
    pub source_truncated: bool,
}

/// Fetches pages and keeps their extracted text for the session.
#[derive(Clone)]
pub struct PageReader {
    client: reqwest::Client,
    permits: Arc<tokio::sync::Semaphore>,
    pages: Arc<Mutex<Pages>>,
    proxy_blocked: bool,
    cache_ttl: Option<Duration>,
    renderer_command: Arc<[String]>,
}

impl PageReader {
    #[cfg(test)]
    pub(crate) fn with_test_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    /// A reader using the outgoing settings' TLS, HTTP/2 and proxy choices.
    /// `overrides` pin hostnames to addresses without checks; tests use it
    /// to serve pages from loopback.
    pub fn new(
        outgoing: &crate::settings::Outgoing,
        overrides: &[(String, SocketAddr)],
    ) -> Result<Self, String> {
        let (client, proxy_blocked) = public_client(outgoing, overrides)?;
        Ok(Self {
            client,
            proxy_blocked,
            cache_ttl: None,
            renderer_command: Arc::from([]),
            permits: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT)),
            pages: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    /// HTTP callers preserve their configured cache expiry; MCP callers keep
    /// the same text while paging within their session.
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = Some(ttl);
        self
    }

    /// Empty commands keep static HTML extraction.
    pub fn with_renderer(mut self, command: Vec<String>) -> Self {
        self.renderer_command = command.into();
        self.pages = Arc::new(Mutex::new(VecDeque::new()));
        self
    }

    /// The readable text of a page, up to `max_chars` characters (with a
    /// trailing '…' when there was more), or an explanation.
    pub async fn text(&self, url: &str, max_chars: usize) -> Result<Arc<str>, String> {
        Ok(self.read(url, max_chars).await?.text)
    }

    /// Fetch and extract a page. `source_truncated` is set when readable text
    /// exceeded `max_chars`; callers can keep it distinct from output slicing.
    pub async fn read(&self, url: &str, max_chars: usize) -> Result<PageContent, String> {
        if self.proxy_blocked {
            return Err("page reads are unavailable with configured proxies: destination validation requires direct connections".into());
        }
        if let Some(content) = self.remembered(url, max_chars) {
            return Ok(content);
        }
        let parsed = reqwest::Url::parse(url).map_err(|_| "not a valid URL".to_string())?;
        if !is_fetchable(&parsed) {
            return Err("only public http(s) pages can be read".into());
        }
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        let body = tokio::time::timeout(FETCH_TIMEOUT, self.fetch(parsed))
            .await
            .map_err(|_| "the page took too long to load".to_string())??;
        let text = match body {
            PageBody::Text {
                url,
                mut content,
                html_media_type,
            } => {
                if let Some(media_type) = html_media_type
                    && !self.renderer_command.is_empty()
                {
                    content = super::render::render(
                        &self.client,
                        &self.renderer_command,
                        url,
                        media_type,
                        content,
                    )
                    .await?;
                }
                tokio::task::spawn_blocking(move || {
                    let document = scraper::Html::parse_document(&content);
                    crate::content::main_text_limited(&document, max_chars)
                })
                .await
                .map_err(|error| error.to_string())?
            }
            PageBody::Pdf(bytes) => super::pdf::extract(bytes, max_chars).await?,
        };
        if text.is_empty() {
            return Err("the page has no readable text".into());
        }
        let text: Arc<str> = text.into();
        let content = PageContent {
            source_truncated: text.chars().count() > max_chars,
            text,
        };
        self.remember(url, max_chars, content.clone());
        Ok(content)
    }

    async fn fetch(&self, url: reqwest::Url) -> Result<PageBody, String> {
        let mut response = self
            .client
            .get(url)
            .header(
                "Accept",
                "text/html,application/xhtml+xml;q=0.9,application/pdf;q=0.9,text/plain;q=0.8",
            )
            .send()
            .await
            .map_err(|error| format!("the page could not be fetched: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("the page answered {}", response.status()));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("text/html")
            .to_ascii_lowercase();
        let media_type = content_type.split(';').next().unwrap_or_default().trim();
        let is_text = media_type.starts_with("text/") || media_type.contains("xhtml");
        let is_pdf = media_type == "application/pdf";
        if !(is_text || is_pdf || media_type == "application/octet-stream") {
            return Err(format!("not a text page ({content_type})"));
        }
        let html_media_type = match media_type {
            "text/html" => Some("text/html"),
            "application/xhtml+xml" => Some("application/xhtml+xml"),
            _ => None,
        };
        // Read at most MAX_BODY_BYTES decoded bytes, whatever Content-Length says.
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("the page could not be read: {error}"))?
        {
            let room = MAX_BODY_BYTES - body.len();
            if chunk.len() > room {
                return Err("page exceeds the 4 MiB decoded body limit".into());
            }
            body.extend_from_slice(&chunk);
        }
        if body.starts_with(b"%PDF-") {
            Ok(PageBody::Pdf(body))
        } else if is_pdf {
            Err("the response is not a PDF document".into())
        } else if is_text {
            Ok(PageBody::Text {
                url: response.url().clone(),
                content: String::from_utf8_lossy(&body).into_owned(),
                html_media_type,
            })
        } else {
            Err(format!("not a text page ({content_type})"))
        }
    }

    fn remembered(&self, url: &str, max_chars: usize) -> Option<PageContent> {
        let pages = self
            .pages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pages
            .iter()
            .find(|(known, limit, saved, _)| {
                known == url
                    && *limit == max_chars
                    && self.cache_ttl.is_none_or(|ttl| saved.elapsed() < ttl)
            })
            .map(|(_, _, _, content)| content.clone())
    }

    fn remember(&self, url: &str, max_chars: usize, content: PageContent) {
        let mut pages = self
            .pages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pages.retain(|(known, limit, _, _)| known != url || *limit != max_chars);
        pages.push_back((
            url.to_string(),
            max_chars,
            std::time::Instant::now(),
            content,
        ));
        if pages.len() > REMEMBERED_PAGES {
            pages.pop_front();
        }
    }
}

/// Environment proxies are deliberately rejected: using one would delegate
/// DNS resolution and destination selection to the proxy, bypassing our pin.
fn environment_proxy_configured() -> bool {
    [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ]
    .iter()
    .any(|name| {
        std::env::var(name)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_page_cache_expires_while_session_paging_stays_stable() {
        let content = PageContent {
            text: Arc::from("old page"),
            source_truncated: false,
        };
        let session = PageReader::new(&crate::settings::Outgoing::default(), &[]).unwrap();
        session.remember("https://example.org/", 100, content.clone());
        assert!(session.remembered("https://example.org/", 100).is_some());
        let http = session.with_cache_ttl(Duration::ZERO);
        assert!(http.remembered("https://example.org/", 100).is_none());
    }

    #[test]
    fn only_global_addresses_are_public() {
        for private in [
            "0.0.0.0",
            "10.1.2.3",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::127.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::808:808",
            "2002:7f00:1::",
            "2001::1",
            "2001:db8::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "100::1",
        ] {
            assert!(!is_global(private.parse().unwrap()), "{private}");
        }
        for public in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.215.14",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2002:808:808::",
            "2606:4700::1111",
            "2a00:1450::1",
            "2001:4860::8888",
        ] {
            assert!(is_global(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn literal_hosts_and_schemes_are_checked() {
        for refused in [
            "http://127.0.0.1/",
            "http://[::ffff:127.0.0.1]:8080/",
            "http://[fe80::1]/",
            "http://localhost/",
            "http://a.localhost/",
            "http://LOCALHOST./",
            "file:///etc/passwd",
            "ftp://example.com/",
            "http://169.254.169.254/latest",
        ] {
            assert!(
                !is_fetchable(&reqwest::Url::parse(refused).unwrap()),
                "{refused}"
            );
        }
        for allowed in [
            "https://example.com/",
            "http://8.8.8.8/",
            "https://[2606:4700::1111]/",
        ] {
            assert!(
                is_fetchable(&reqwest::Url::parse(allowed).unwrap()),
                "{allowed}"
            );
        }
    }

    #[tokio::test]
    async fn configured_proxies_fail_closed() {
        let mut outgoing = crate::settings::Outgoing::default();
        outgoing
            .proxies
            .insert("all".into(), "http://127.0.0.1:8080".into());
        let reader = PageReader::new(&outgoing, &[]).unwrap();
        let error = reader.text("https://example.com/", 100).await.unwrap_err();
        assert!(error.contains("configured proxies"));
    }

    #[tokio::test]
    async fn names_resolving_to_private_addresses_are_refused() {
        let error = PublicResolver(Arc::new(SystemResolver))
            .resolve("localhost".parse().unwrap())
            .await
            .err()
            .expect("localhost must not resolve");
        assert!(error.to_string().contains("non-public address"), "{error}");
    }

    struct Answers(Mutex<VecDeque<Vec<SocketAddr>>>);

    impl Resolve for Answers {
        fn resolve(&self, _: Name) -> Resolving {
            let addresses = self.0.lock().unwrap().pop_front().unwrap();
            Box::pin(async move { Ok(Box::new(addresses.into_iter()) as Addrs) })
        }
    }

    #[tokio::test]
    async fn dns_answers_are_checked_as_a_whole_and_never_resolved_twice() {
        let public: SocketAddr = "8.8.8.8:80".parse().unwrap();
        for unsafe_answer in [
            vec![],
            vec!["127.0.0.1:80".parse().unwrap()],
            vec![public, "127.0.0.1:80".parse().unwrap()],
            vec!["[fd00::1]:80".parse().unwrap(), public],
            vec!["[::ffff:127.0.0.1]:80".parse().unwrap()],
            vec!["[64:ff9b::a9fe:a9fe]:80".parse().unwrap()],
        ] {
            let inner = Arc::new(Answers(Mutex::new(VecDeque::from([
                vec![public],
                unsafe_answer,
            ]))));
            let resolver = PublicResolver(inner.clone());
            let checked: Vec<_> = resolver
                .resolve("example.test".parse().unwrap())
                .await
                .unwrap()
                .collect();
            assert_eq!(checked, [public]);
            assert_eq!(
                inner.0.lock().unwrap().len(),
                1,
                "one lookup per connection"
            );
            assert!(
                resolver
                    .resolve("example.test".parse().unwrap())
                    .await
                    .is_err()
            );
            assert!(inner.0.lock().unwrap().is_empty());
        }
    }
}
