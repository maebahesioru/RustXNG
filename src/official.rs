//! Official websites of the entities a search found on Wikipedia.
//!
//! A navigational query ("npm official", "気象庁 天気予報") wants the entity's
//! own site. Without a web search engine the site is still known: Wikidata
//! records it as "official website" (P856) on the item behind the Wikipedia
//! article the search already found. One SPARQL request to the Wikidata Query
//! Service (a documented public endpoint) maps the top articles to their
//! official sites, so this works without any web search provider or API key.

use std::time::Duration;

use crate::engine::EngineRequest;
use crate::network::Network;
use crate::results::SearchResult;

/// The Wikidata Query Service endpoint.
const ENDPOINT: &str = "https://query.wikidata.org/sparql";

/// At most this many articles are looked up per search.
pub const MAX_ARTICLES: usize = 3;

/// A Wikipedia article: wiki language and title (spaces, not underscores).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Article {
    pub language: String,
    pub title: String,
}

/// The article behind a Wikipedia URL (`https://<lang>.wikipedia.org/wiki/<title>`).
pub fn article_of(url: &str) -> Option<Article> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let language = host.strip_suffix(".wikipedia.org")?;
    if language.is_empty()
        || language.contains('.')
        || !language
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    let encoded = parsed.path().strip_prefix("/wiki/")?;
    if encoded.is_empty() || parsed.query().is_some() {
        return None;
    }
    let title = percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .ok()?
        .replace('_', " ");
    // Namespaced pages (Category:, File:, …) are not articles. Article
    // titles may contain a colon ("Star Wars: Andor"), so only known
    // namespace prefixes are refused.
    const NAMESPACES: &[&str] = &[
        "category:",
        "file:",
        "image:",
        "template:",
        "help:",
        "portal:",
        "wikipedia:",
        "talk:",
        "user:",
        "special:",
        "draft:",
        "module:",
        "mediawiki:",
    ];
    let lowered = title.to_lowercase();
    if title.trim().is_empty() || NAMESPACES.iter().any(|prefix| lowered.starts_with(prefix)) {
        return None;
    }
    Some(Article {
        language: language.to_string(),
        title,
    })
}

/// The article IRI as Wikidata stores it (`schema:about` subjects): the title
/// with underscores, percent-encoded like MediaWiki's `wfUrlencode`, which
/// keeps `;@$!*(),/~:` literal.
pub fn article_iri(article: &Article) -> String {
    format!(
        "https://{}.wikipedia.org/wiki/{}",
        article.language,
        crate::engines::wikipedia::wiki_title_path(&article.title)
    )
}

/// What a search can want from the item behind an article.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Official website (P856).
    Site,
    /// Source code repository (P1324), for "... github".
    Repository,
    /// arXiv ID (P818), for "... arxiv" / "... paper".
    Paper,
}

impl Kind {
    fn property(self) -> &'static str {
        match self {
            Self::Site => "P856",
            Self::Repository => "P1324",
            Self::Paper => "P818",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Site => "site",
            Self::Repository => "repo",
            Self::Paper => "arxiv",
        }
    }

    /// The link for a property value (an arXiv ID becomes its abstract page).
    fn url(self, value: &str) -> Option<String> {
        match self {
            Self::Paper => {
                let id = value.trim();
                (!id.is_empty() && !id.contains(char::is_whitespace))
                    .then(|| format!("https://arxiv.org/abs/{id}"))
            }
            _ => (value.starts_with("https://") || value.starts_with("http://"))
                .then(|| value.to_string()),
        }
    }

    fn label(self, title: &str) -> (String, String) {
        match self {
            Self::Site => (
                format!("{title} (official website)"),
                format!("Official website of {title}, as recorded on Wikidata."),
            ),
            Self::Repository => (
                format!("{title} (source code repository)"),
                format!("Source code repository of {title}, as recorded on Wikidata."),
            ),
            Self::Paper => (
                format!("{title} (arXiv)"),
                format!("The arXiv paper of {title}, as recorded on Wikidata."),
            ),
        }
    }
}

/// The kinds a query asks for, most wanted first. The official website is
/// always included; a repository or paper only when the query names one.
pub fn kinds_for(query: &str) -> Vec<Kind> {
    let words: Vec<String> = query
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(str::to_string)
        .collect();
    let has = |names: &[&str]| words.iter().any(|word| names.contains(&word.as_str()));
    let mut kinds = Vec::new();
    if has(&[
        "github",
        "gitlab",
        "codeberg",
        "repo",
        "repository",
        "source",
    ]) {
        kinds.push(Kind::Repository);
    }
    if has(&["arxiv", "paper", "論文"]) {
        kinds.push(Kind::Paper);
    }
    kinds.push(Kind::Site);
    kinds
}

/// The SPARQL query mapping articles to the wanted links of their items.
pub fn sparql(articles: &[Article], kinds: &[Kind]) -> String {
    let values: Vec<String> = articles
        .iter()
        .map(|article| format!("<{}>", article_iri(article)))
        .collect();
    let branches: Vec<String> = kinds
        .iter()
        .map(|kind| {
            format!(
                "{{ ?item wdt:{} ?value . BIND(\"{}\" AS ?kind) }}",
                kind.property(),
                kind.tag()
            )
        })
        .collect();
    format!(
        "SELECT ?article ?kind ?value WHERE {{ VALUES ?article {{ {} }} \
         ?article schema:about ?item . {} }} LIMIT 60",
        values.join(" "),
        branches.join(" UNION ")
    )
}

/// Parse the SPARQL answer into (article, kind, url): for every article in
/// order, one link per wanted kind (kinds in the order given), no URL twice.
pub fn parse(
    json: &serde_json::Value,
    articles: &[Article],
    kinds: &[Kind],
) -> Vec<(Article, Kind, String)> {
    let Some(rows) = json
        .pointer("/results/bindings")
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };
    let field = |row: &serde_json::Value, name: &str| {
        row.pointer(&format!("/{name}/value"))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let mut found: Vec<(Article, Kind, String)> = Vec::new();
    for kind in kinds {
        for article in articles {
            let iri = article_iri(article);
            let url = rows.iter().find_map(|row| {
                if field(row, "article").as_deref() != Some(iri.as_str())
                    || field(row, "kind").as_deref() != Some(kind.tag())
                {
                    return None;
                }
                kind.url(&field(row, "value")?)
            });
            let Some(url) = url else { continue };
            if found.iter().any(|(_, _, known)| *known == url) {
                continue;
            }
            found.push((article.clone(), *kind, url));
        }
    }
    found
}

/// Look up the wanted links of `articles` (best first). Any failure or a
/// slow endpoint yields nothing: this only ever adds results.
///
pub async fn lookup(
    network: &Network,
    articles: &[Article],
    kinds: &[Kind],
    timeout: Duration,
) -> Vec<SearchResult> {
    lookup_inner(network, None, None, articles, kinds, timeout, "").await
}

/// Reuse the search cache and validate optional localized entry points through
/// the public-page reader (including DNS, redirects and robots.txt).
pub(crate) async fn lookup_cached(
    network: &Network,
    cache: &crate::cache::Cache,
    reader: &crate::fetch::PageReader,
    articles: &[Article],
    kinds: &[Kind],
    timeout: Duration,
    locale: &str,
) -> Vec<SearchResult> {
    lookup_inner(
        network,
        Some(cache),
        Some(reader),
        articles,
        kinds,
        timeout,
        locale,
    )
    .await
}

async fn lookup_inner(
    network: &Network,
    cache: Option<&crate::cache::Cache>,
    reader: Option<&crate::fetch::PageReader>,
    articles: &[Article],
    kinds: &[Kind],
    timeout: Duration,
    locale: &str,
) -> Vec<SearchResult> {
    let started = std::time::Instant::now();
    if articles.is_empty() || kinds.is_empty() || timeout.is_zero() {
        return Vec::new();
    }
    let query = sparql(articles, kinds);
    let url = format!(
        "{ENDPOINT}?format=json&query={}",
        crate::engine::util::encode(&query)
    );
    let request = EngineRequest::get(url).header("Accept", "application/sparql-results+json");
    let response = match network.fetch(request, timeout, cache, None).await {
        Ok(response) if response.status == 200 => response,
        Ok(response) => {
            tracing::debug!(status = response.status, "official site lookup refused");
            return Vec::new();
        }
        Err(error) => {
            tracing::debug!(%error, "official site lookup failed");
            return Vec::new();
        }
    };
    let Ok(json) = response.json() else {
        return Vec::new();
    };
    let language = locale
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_string();
    let found = parse(&json, articles, kinds);
    let mut results = Vec::new();
    // Probe only the first (most relevant) official site, and only a bare
    // root: a deep path has its own language routing.
    let mut probe: Option<(usize, String)> = None;
    for (index, (article, kind, url)) in found.iter().enumerate() {
        let (title, content) = kind.label(&article.title);
        let mut result = SearchResult::new(url.clone(), title);
        result.content = content;
        results.push(result);
        if probe.is_none()
            && *kind == Kind::Site
            && (2..=3).contains(&language.len())
            && language.bytes().all(|c| c.is_ascii_alphabetic())
            && language != "en"
            && path_is_root(url)
        {
            probe = Some((index, format!("{}/{}", url.trim_end_matches("/"), language)));
        }
    }
    if let Some((index, variant)) = probe {
        let remaining = timeout.saturating_sub(started.elapsed());
        if let Some(reader) = reader.filter(|reader| reader.is_direct())
            && reachable(reader, &variant, &language, remaining.min(PROBE_TIMEOUT)).await
        {
            let (article, kind, _) = &found[index];
            let (title, content) = kind.label(&article.title);
            let mut result = SearchResult::new(variant, title);
            result.content = format!("{content} ({language} entry point)");
            results.push(result);
        }
    }
    results
}

/// The largest time one language-variant probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_millis(600);

/// Whether the URL points at the site root (empty or `/` path).
fn path_is_root(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .map(|parsed| {
            let path = parsed.path();
            (path.is_empty() || path == "/")
                && parsed.query().is_none()
                && parsed.fragment().is_none()
                && parsed.username().is_empty()
                && parsed.password().is_none()
        })
        .unwrap_or(false)
}

/// Whether the URL answers 200 within the timeout.
async fn reachable(
    reader: &crate::fetch::PageReader,
    url: &str,
    language: &str,
    timeout: Duration,
) -> bool {
    if timeout.is_zero() {
        return false;
    }
    let Ok(language) = crate::fetch::AcceptLanguage::parse(language) else {
        return false;
    };
    matches!(
        tokio::time::timeout(timeout, reader.read_with_language(url, 512, &language)).await,
        Ok(Ok(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn localized_probe_refuses_private_destinations_and_zero_budget() {
        let reader =
            crate::fetch::PageReader::new(&crate::settings::Outgoing::default(), &[]).unwrap();
        assert!(
            !reachable(
                &reader,
                "http://127.0.0.1:9/ja",
                "ja",
                Duration::from_millis(50)
            )
            .await
        );
        assert!(!reachable(&reader, "https://example.org/ja", "ja", Duration::ZERO).await);
    }

    #[test]
    fn language_probe_only_uses_unambiguous_roots() {
        assert!(path_is_root("https://example.org/"));
        assert!(!path_is_root("https://example.org/?redirect=1"));
        assert!(!path_is_root("https://example.org/#fragment"));
        assert!(!path_is_root("https://user@example.org/"));
        assert!(!path_is_root("https://example.org/docs"));
    }

    #[test]
    fn articles_are_read_from_wikipedia_urls() {
        assert_eq!(
            article_of("https://ja.wikipedia.org/wiki/%E6%B0%97%E8%B1%A1%E5%BA%81"),
            Some(Article {
                language: "ja".into(),
                title: "気象庁".into()
            })
        );
        assert_eq!(
            article_of("https://en.wikipedia.org/wiki/Rust_%28programming_language%29")
                .unwrap()
                .title,
            "Rust (programming language)"
        );
        assert!(article_of("https://en.wikipedia.org/wiki/Category:Japan").is_none());
        assert_eq!(
            article_of("https://en.wikipedia.org/wiki/Star_Wars%3A_Andor")
                .unwrap()
                .title,
            "Star Wars: Andor"
        );
        assert!(article_of("https://en.wikipedia.org/w/index.php?curid=1").is_none());
        assert!(article_of("https://example.org/wiki/Npm").is_none());
        assert!(article_of("https://evil.example.wikipedia.org.attacker/wiki/X").is_none());
    }

    #[test]
    fn iris_match_the_wikidata_encoding() {
        let article = |language: &str, title: &str| Article {
            language: language.into(),
            title: title.into(),
        };
        assert_eq!(
            article_iri(&article("en", "Rust (programming language)")),
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert_eq!(
            article_iri(&article("ja", "気象庁")),
            "https://ja.wikipedia.org/wiki/%E6%B0%97%E8%B1%A1%E5%BA%81"
        );
        assert_eq!(
            article_iri(&article("en", "AT&T \"x\" ?#")),
            "https://en.wikipedia.org/wiki/AT%26T_%22x%22_%3F%23"
        );
        assert!(
            sparql(&[article("en", "Npm")], &[Kind::Site])
                .contains("<https://en.wikipedia.org/wiki/Npm>")
        );
    }

    #[test]
    fn answers_follow_the_wanted_kinds_and_skip_duplicates() {
        let articles = vec![
            Article {
                language: "en".into(),
                title: "Npm".into(),
            },
            Article {
                language: "en".into(),
                title: "Tokio (software)".into(),
            },
        ];
        let row = |title: &str, kind: &str, value: &str| {
            serde_json::json!({
                "article": {"value": format!("https://en.wikipedia.org/wiki/{title}")},
                "kind": {"value": kind},
                "value": {"value": value}
            })
        };
        let json = serde_json::json!({"results": {"bindings": [
            row("Tokio_(software)", "site", "https://tokio.rs/"),
            row("Tokio_(software)", "repo", "https://github.com/tokio-rs/tokio"),
            row("Npm", "site", "ftp://bad"),
            row("Npm", "site", "https://www.npmjs.com/"),
            row("Npm", "arxiv", "1706.03762"),
        ]}});
        let found = parse(&json, &articles, &[Kind::Repository, Kind::Site]);
        let urls: Vec<&str> = found.iter().map(|(_, _, url)| url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://github.com/tokio-rs/tokio",
                "https://www.npmjs.com/",
                "https://tokio.rs/"
            ]
        );
        let found = parse(&json, &articles, &[Kind::Paper]);
        assert_eq!(found[0].2, "https://arxiv.org/abs/1706.03762");
    }

    #[test]
    fn the_query_says_which_links_it_wants() {
        assert_eq!(kinds_for("npm 公式"), vec![Kind::Site]);
        assert_eq!(
            kinds_for("tokio-rs tokio github"),
            vec![Kind::Repository, Kind::Site]
        );
        assert_eq!(
            kinds_for("attention is all you need arxiv"),
            vec![Kind::Paper, Kind::Site]
        );
    }
}
