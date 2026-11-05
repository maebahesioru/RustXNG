//! Local BM25 retrieval over crawled pages and imported Wikipedia articles.
//! Word tokens support space-separated text; character bigrams support CJK.

mod mixed_script;

use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::indexer::UserOperation;
use tantivy::query::{
    BooleanQuery, BoostQuery, ConstScoreQuery, Occur, Query, QueryParser, RegexQuery, TermQuery,
};
use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Term, doc};

/// One page ready to be indexed.
pub struct Page<'a> {
    pub url: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub lang: &'a str,
    /// Short excerpt shown in result lists. Empty falls back to the start of
    /// the body.
    pub snippet: &'a str,
}

/// One search hit.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub score: f32,
}

/// Weight of a title match against a body match.
const TITLE_BOOST: f32 = 3.0;
/// Weight of a match in the words of the URL (host and path).
const URL_BOOST: f32 = 1.5;
/// Extra weight for pages that contain the whole query as a phrase.
const PHRASE_BOOST: f32 = 2.0;
/// Longest stored snippet, in characters.
const SNIPPET_CHARS: usize = 300;
pub(crate) const MAX_SEARCH_WINDOW: usize = 10_000;
/// Bumped whenever the schema changes; an older index must be rebuilt.
pub const SCHEMA_VERSION: u32 = 2;

/// A name for the CJK tokenizer, registered once per index.
const BIGRAM: &str = "bigram";

/// Fields of the schema, kept together so the searcher can address them.
struct Fields {
    url: Field,
    title: Field,
    body: Field,
    title_ja: Field,
    body_ja: Field,
    lang: Field,
    url_text: Field,
    snippet: Field,
}

/// An opened local index.
pub struct LocalIndex {
    index: Index,
    reader: IndexReader,
    fields: Fields,
    /// Present only for the process that writes the index.
    writer: Option<std::sync::Mutex<IndexWriter>>,
}

fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    // The plain tokenizer for space-separated languages.
    let plain = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("default")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let plain_url = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("default")
            .set_index_option(IndexRecordOption::WithFreqs),
    );
    // Bigrams for languages without spaces.
    let ngram = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(BIGRAM)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    builder.add_text_field("url", STRING | STORED);
    builder.add_text_field("title", plain.clone() | STORED);
    builder.add_text_field("body", plain);
    builder.add_text_field("title_ja", ngram.clone() | STORED);
    builder.add_text_field("body_ja", ngram);
    builder.add_text_field("lang", STRING | STORED);
    // The words of the host and path, so navigational queries ("tokio
    // tutorial") match pages whose URL says so.
    builder.add_text_field("url_text", plain_url);
    builder.add_text_field("snippet", STORED);
    builder.build()
}

/// The text for the bigram fields: only pages that contain Japanese or Chinese
/// get bigrams. Every letter pair of an English page would otherwise be
/// indexed for queries it can never answer, which doubles the index.
fn cjk_only(text: &str) -> String {
    if has_cjk(text) {
        text.to_string()
    } else {
        String::new()
    }
}

/// True when the text contains Japanese or Chinese characters.
pub fn has_cjk(text: &str) -> bool {
    text.chars().any(is_cjk)
}

/// The host and path of a URL as plain words ("docs.rs/tokio/latest" becomes
/// "docs rs tokio latest"); the default tokenizer splits on punctuation.
fn url_words(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => format!(
            "{} {}",
            parsed.host_str().unwrap_or(""),
            parsed.path().replace(['/', '-', '_', '.'], " ")
        ),
        Err(_) => url.to_string(),
    }
}

impl LocalIndex {
    /// Open the index for reading. Safe to call from any number of processes,
    /// including while a crawl is writing.
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_inner(path, false)
    }

    /// Open the index for writing. Only one process may hold this at a time.
    pub fn open_for_write(path: &Path) -> Result<Self, String> {
        Self::open_inner(path, true)
    }

    fn open_inner(path: &Path, writable: bool) -> Result<Self, String> {
        // Only the writer creates the directory: a reader that finds nothing
        // must not litter the working tree with an empty index.
        if writable {
            std::fs::create_dir_all(path).map_err(|error| format!("create {path:?}: {error}"))?;
        } else if !path.exists() {
            return Err(format!("no index at {path:?}"));
        }
        let directory =
            MmapDirectory::open(path).map_err(|error| format!("open {path:?}: {error}"))?;
        let exists = Index::exists(&directory)
            .map_err(|error| format!("inspect index at {path:?}: {error}"))?;
        let index = if writable && !exists {
            Index::create_in_dir(path, build_schema())
                .map_err(|error| format!("create index: {error}"))?
        } else {
            Index::open(directory).map_err(|error| format!("open index at {path:?}: {error}"))?
        };
        let tokenizer = TextAnalyzer::builder(
            NgramTokenizer::new(2, 2, false).map_err(|error| error.to_string())?,
        )
        .filter(LowerCaser)
        .build();
        index.tokenizers().register(BIGRAM, tokenizer);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|error| format!("reader: {error}"))?;
        let writer = if writable {
            Some(
                index
                    .writer(256_000_000)
                    .map_err(|error| format!("writer: {error}"))?,
            )
        } else {
            None
        };
        let schema = index.schema();
        if schema.get_field("snippet").is_err() || schema.get_field("url_text").is_err() {
            return Err(format!(
                "the index at {path:?} was built by an older version (schema < {SCHEMA_VERSION}); rebuild it with `rustxng-index crawl`"
            ));
        }
        let fields = Fields {
            url: schema.get_field("url").map_err(|error| error.to_string())?,
            title: schema
                .get_field("title")
                .map_err(|error| error.to_string())?,
            body: schema
                .get_field("body")
                .map_err(|error| error.to_string())?,
            title_ja: schema
                .get_field("title_ja")
                .map_err(|error| error.to_string())?,
            body_ja: schema
                .get_field("body_ja")
                .map_err(|error| error.to_string())?,
            lang: schema
                .get_field("lang")
                .map_err(|error| error.to_string())?,
            url_text: schema
                .get_field("url_text")
                .map_err(|error| error.to_string())?,
            snippet: schema
                .get_field("snippet")
                .map_err(|error| error.to_string())?,
        };
        Ok(Self {
            index,
            reader,
            fields,
            writer: writer.map(std::sync::Mutex::new),
        })
    }

    /// Number of indexed documents.
    pub fn document_count(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    /// Whether this URL is already indexed (committed documents only).
    /// Lets a later crawl continue where the previous one stopped instead of
    /// indexing the same pages twice.
    pub fn has_url(&self, url: &str) -> bool {
        let searcher = self.reader.searcher();
        let query = TermQuery::new(
            Term::from_field_text(self.fields.url, url),
            IndexRecordOption::Basic,
        );
        searcher
            .search(&query, &TopDocs::with_limit(1).order_by_score())
            .map(|hits| !hits.is_empty())
            .unwrap_or(false)
    }

    /// Replace this URL's page. Changes become visible after commit.
    pub fn add(&self, page: &Page<'_>) -> Result<(), String> {
        let writer = self.writer.as_ref().ok_or("index opened read-only")?;
        let writer = writer.lock().map_err(|_| "writer poisoned")?;
        let snippet = if page.snippet.is_empty() {
            crate::crawl::snippet(page.body, SNIPPET_CHARS)
        } else {
            page.snippet.to_string()
        };
        let document = doc!(
            self.fields.url_text => url_words(page.url),
            self.fields.snippet => snippet,
            self.fields.url => page.url.to_string(),
            self.fields.title => page.title.to_string(),
            self.fields.body => page.body.to_string(),
            self.fields.title_ja => cjk_only(page.title),
            self.fields.body_ja => cjk_only(page.body),
            self.fields.lang => page.lang.to_string(),
        );
        writer
            .run([
                UserOperation::Delete(Term::from_field_text(self.fields.url, page.url)),
                UserOperation::Add(document),
            ])
            .map_err(|error| format!("replace document: {error}"))?;
        Ok(())
    }

    /// Remove a URL, for example after a recrawl discovers `noindex`.
    pub fn remove(&self, url: &str) -> Result<(), String> {
        let writer = self.writer.as_ref().ok_or("index opened read-only")?;
        let writer = writer.lock().map_err(|_| "writer poisoned")?;
        writer.delete_term(Term::from_field_text(self.fields.url, url));
        Ok(())
    }

    /// Flush everything added so far and make it searchable.
    pub fn commit(&self) -> Result<(), String> {
        if let Some(writer) = self.writer.as_ref() {
            let mut writer = writer.lock().map_err(|_| "writer poisoned")?;
            writer
                .commit()
                .map_err(|error| format!("commit: {error}"))?;
        }
        self.reader
            .reload()
            .map_err(|error| format!("reload: {error}"))?;
        Ok(())
    }

    /// Rank the indexed pages for a query.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
        self.search_with(query, limit, false)
    }

    /// Like [`search`](Self::search), but a page needs only one of the content
    /// words, and the ranking decides. For a large general index (an
    /// encyclopedia), where almost every question has a page that holds only
    /// some of its words.
    pub fn search_loose(&self, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
        self.search_with(query, limit, true)
    }

    fn search_with(&self, query: &str, limit: usize, loose: bool) -> Result<Vec<Hit>, String> {
        self.search_filtered(query, limit, &[], &[], loose)
    }

    /// Apply host restrictions before selecting the best `limit` documents.
    pub fn search_filtered(
        &self,
        query: &str,
        limit: usize,
        include_domains: &[String],
        exclude_domains: &[String],
        loose: bool,
    ) -> Result<Vec<Hit>, String> {
        self.search_range(query, 0..limit, include_domains, exclude_domains, loose)
    }

    /// Search a result window without counting filtered-out pages.
    pub fn search_range(
        &self,
        query: &str,
        range: std::ops::Range<usize>,
        include_domains: &[String],
        exclude_domains: &[String],
        loose: bool,
    ) -> Result<Vec<Hit>, String> {
        let limit = range.end.saturating_sub(range.start);
        let trimmed = query.trim();
        if trimmed.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        if trimmed.len() > 16_384 {
            return Err("local search query exceeds 16384 bytes".to_string());
        }
        if range.end > MAX_SEARCH_WINDOW {
            return Err("local search window exceeds 10000 results".to_string());
        }
        if include_domains.len().saturating_add(exclude_domains.len()) > 64
            || include_domains
                .iter()
                .chain(exclude_domains)
                .any(|domain| domain.len() > 2048)
        {
            return Err("local search domain filters exceed their size limit".to_string());
        }
        let words = content_words(trimmed);
        if words.is_empty() {
            return Ok(Vec::new());
        }
        if words.len() > 128 || words.iter().any(|word| word.chars().count() > 256) {
            return Err("local search query has too many terms".to_string());
        }
        let needed = if loose {
            1
        } else {
            required_matches(words.len())
        };
        let cjk_query = has_cjk(trimmed);
        let terms = words
            .iter()
            .map(|word| self.word_query(word, cjk_query))
            .collect();
        let mut required: Vec<Box<dyn Query>> = vec![Box::new(
            BooleanQuery::union_with_minimum_required_clauses(terms, needed),
        )];
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        // URL terms have no positions; phrases only use title and body.
        if words.len() >= 2 {
            let mut parser =
                QueryParser::for_index(&self.index, vec![self.fields.title, self.fields.body]);
            parser.set_field_boost(self.fields.title, TITLE_BOOST);
            let literal = trimmed.replace('\\', "\\\\").replace('"', "\\\"");
            if let Ok(phrase) = parser.parse_query(&format!("\"{literal}\"")) {
                if has_cjk(trimmed) {
                    // Existing indexes keep mixed scripts in one plain token.
                    required.push(phrase);
                } else {
                    clauses.push((
                        Occur::Should,
                        Box::new(BoostQuery::new(phrase, PHRASE_BOOST)),
                    ));
                }
            }
        }
        clauses.push((Occur::Must, Box::new(BooleanQuery::union(required))));
        if !include_domains.is_empty() {
            clauses.push((
                Occur::Must,
                Box::new(ConstScoreQuery::new(
                    self.domain_query(include_domains)?,
                    0.0,
                )),
            ));
        }
        if !exclude_domains.is_empty() {
            clauses.push((Occur::MustNot, self.domain_query(exclude_domains)?));
        }
        let query = BooleanQuery::new(clauses);
        let searcher = self.reader.searcher();
        let top = searcher
            .search(
                &query,
                &TopDocs::with_limit(limit)
                    .and_offset(range.start)
                    .order_by_score(),
            )
            .map_err(|error| format!("search: {error}"))?;
        let mut hits = Vec::new();
        for (score, address) in top {
            let document: tantivy::TantivyDocument = searcher
                .doc(address)
                .map_err(|error| format!("doc: {error}"))?;
            let text = |field: Field| {
                document
                    .get_first(field)
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_default()
            };
            hits.push(Hit {
                url: text(self.fields.url),
                title: text(self.fields.title),
                snippet: text(self.fields.snippet),
                score,
            });
        }
        Ok(hits)
    }

    fn word_query(&self, word: &str, cjk_query: bool) -> Box<dyn Query> {
        let fields = [
            (self.fields.title, TITLE_BOOST),
            (self.fields.body, 1.0),
            (self.fields.url_text, URL_BOOST),
        ];
        if !has_cjk(word) || word.chars().count() < 2 {
            let plain = term_in_fields(word, &fields);
            if !cjk_query || has_cjk(word) {
                return plain;
            }
            // Older indexes retain words such as `Rustの所有権` as one token.
            let mixed = mixed_script::MixedScriptQuery::new(word, self.fields.title);
            let mut variants = vec![plain];
            for (field, boost) in &fields[..2] {
                let query = mixed.for_field(*field);
                variants.push(Box::new(BoostQuery::new(Box::new(query), *boost)));
            }
            return Box::new(BooleanQuery::union(variants));
        }
        let chars: Vec<char> = word.chars().collect();
        let mut grams = Vec::new();
        for pair in chars.windows(2) {
            let gram: String = pair.iter().collect();
            if !grams.contains(&gram) {
                grams.push(gram);
            }
        }
        let needed = required_matches(grams.len());
        let fields = [
            (self.fields.title_ja, TITLE_BOOST),
            (self.fields.body_ja, 1.0),
        ];
        let terms = grams
            .iter()
            .map(|gram| term_in_fields(gram, &fields))
            .collect();
        Box::new(BooleanQuery::union_with_minimum_required_clauses(
            terms, needed,
        ))
    }

    fn domain_query(&self, domains: &[String]) -> Result<Box<dyn Query>, String> {
        let mut queries: Vec<Box<dyn Query>> = Vec::new();
        for domain in domains {
            let domain = crate::results::normalize_domain(domain);
            if domain.is_empty() {
                continue;
            }
            let pattern = format!(
                "https?://([^/@:]+\\.)?{}(:[0-9]+)?([/?#].*)?",
                regex::escape(&domain),
            );
            let query = RegexQuery::from_pattern(&pattern, self.fields.url)
                .map_err(|error| format!("domain filter: {error}"))?;
            queries.push(Box::new(query));
        }
        Ok(Box::new(BooleanQuery::union(queries)))
    }
}

/// Words too common to say anything about the topic of a query.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "did", "do", "does", "for", "from", "has",
    "have", "how", "in", "is", "it", "its", "many", "most", "much", "of", "on", "or", "that",
    "the", "their", "there", "this", "to", "was", "were", "what", "when", "where", "which", "who",
    "whom", "whose", "why", "will", "with",
];

/// The distinct plain words of a query that carry meaning.
fn content_words(query: &str) -> Vec<String> {
    let mut separated = String::new();
    let mut previous_script = None;
    for ch in query.chars() {
        if ch.is_alphanumeric() {
            let cjk = is_cjk(ch);
            if previous_script.is_some_and(|previous| previous != cjk) {
                separated.push(' ');
            }
            separated.extend(ch.to_lowercase());
            previous_script = Some(cjk);
        } else {
            separated.push(' ');
            previous_script = None;
        }
    }
    let mut words = Vec::new();
    for word in separated.split_whitespace() {
        if !words.iter().any(|existing| existing == word) {
            words.push(word.to_string());
        }
    }
    if words.len() > 1
        && words
            .iter()
            .any(|word| !STOP_WORDS.contains(&word.as_str()))
    {
        words.retain(|word| !STOP_WORDS.contains(&word.as_str()));
    }
    words
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff)
}

fn term_in_fields(term: &str, fields: &[(Field, f32)]) -> Box<dyn Query> {
    let queries = fields
        .iter()
        .map(|&(field, boost)| {
            let query = TermQuery::new(
                Term::from_field_text(field, term),
                IndexRecordOption::WithFreqs,
            );
            Box::new(BoostQuery::new(Box::new(query), boost)) as Box<dyn Query>
        })
        .collect();
    Box::new(BooleanQuery::union(queries))
}

/// How many of `count` content words a page has to contain: all of one or two,
/// then about three out of five.
fn required_matches(count: usize) -> usize {
    match count {
        0 | 1 => count,
        2 => 2,
        _ => (count * 3).div_ceil(5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_index() -> (TempDir, LocalIndex) {
        let dir = TempDir::new().unwrap();
        let index = LocalIndex::open_for_write(dir.path()).unwrap();
        (dir, index)
    }

    #[test]
    fn a_reader_does_not_create_the_directory() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("index-data");
        assert!(
            LocalIndex::open(&missing).is_err(),
            "a missing index is an error"
        );
        assert!(!missing.exists(), "the reader must not create it");
        // The writer creates it on demand.
        let index = LocalIndex::open_for_write(&missing).unwrap();
        index.commit().unwrap();
        assert!(missing.exists());
    }

    #[test]
    fn indexes_and_finds_english_pages() {
        let (_dir, index) = temp_index();
        index
            .add(&Page {
                url: "https://tokio.rs/tokio/tutorial",
                title: "Tutorial",
                body: "async runtime for rust with tasks and futures",
                lang: "en",
                snippet: "",
            })
            .unwrap();
        index
            .add(&Page {
                url: "https://example.com/other",
                title: "Other",
                body: "nothing to do with the query",
                lang: "en",
                snippet: "",
            })
            .unwrap();
        index.commit().unwrap();
        assert_eq!(index.document_count(), 2);
        let hits = index.search("async runtime rust", 5).unwrap();
        assert_eq!(hits[0].url, "https://tokio.rs/tokio/tutorial");
    }

    #[test]
    fn a_long_question_does_not_match_pages_that_share_only_common_words() {
        let (_dir, index) = temp_index();
        for (url, title, body) in [
            (
                "https://docs.example/rust-book",
                "Ownership in Rust",
                "the rules of ownership and borrowing decide how memory is moved",
            ),
            (
                "https://docs.example/unrelated",
                "Unrelated page",
                "this is the page of the most common words that were in the year",
            ),
        ] {
            index
                .add(&Page {
                    url,
                    title,
                    body,
                    lang: "en",
                    snippet: "",
                })
                .unwrap();
        }
        index.commit().unwrap();
        let off_topic = index
            .search(
                "Who holds the all-time record for the most wins in the album of the year category?",
                10,
            )
            .unwrap();
        assert!(
            off_topic.is_empty(),
            "common words alone must not match: {off_topic:?}"
        );
        let on_topic = index
            .search("what are the rules of ownership and borrowing in rust", 10)
            .unwrap();
        assert_eq!(on_topic[0].url, "https://docs.example/rust-book");
        assert_eq!(required_matches(5), 3);
        assert_eq!(
            content_words("The Rust, the rust!"),
            vec!["rust".to_string()]
        );
    }

    #[test]
    fn loose_search_finds_a_page_that_holds_only_some_of_the_words() {
        let (_dir, index) = temp_index();
        index
            .add(&Page {
                url: "https://en.wikipedia.org/wiki/Grammy_Award_for_Album_of_the_Year",
                title: "Grammy Award for Album of the Year",
                body: "The award goes to the artists whose album won; Taylor Swift holds the record.",
                lang: "en",
                snippet: "",
            })
            .unwrap();
        index.commit().unwrap();
        let question = "Which artist has the record for most Grammy wins in album of the year, and which engineer";
        assert!(index.search(question, 5).unwrap().is_empty());
        let loose = index.search_loose(question, 5).unwrap();
        assert_eq!(loose.len(), 1);
        assert!(loose[0].url.ends_with("Album_of_the_Year"));
    }

    #[test]
    fn indexes_and_finds_japanese_pages() {
        let (_dir, index) = temp_index();
        index
            .add(&Page {
                url: "https://www.gsi.go.jp/",
                title: "国土地理院",
                body: "地図と測量の情報を提供しています",
                lang: "ja",
                snippet: "",
            })
            .unwrap();
        index.commit().unwrap();
        let hits = index.search("国土地理院 地図", 5).unwrap();
        assert_eq!(hits[0].url, "https://www.gsi.go.jp/");
    }

    fn page<'a>(url: &'a str, title: &'a str, body: &'a str) -> Page<'a> {
        Page {
            url,
            title,
            body,
            lang: "en",
            snippet: "",
        }
    }

    #[test]
    fn a_title_match_beats_a_body_mention() {
        let (_dir, index) = temp_index();
        index
            .add(&page(
                "https://a.example/body",
                "Unrelated notes",
                "we sometimes talk about tantivy indexing in passing",
            ))
            .unwrap();
        index
            .add(&page(
                "https://b.example/title",
                "Tantivy indexing guide",
                "a guide",
            ))
            .unwrap();
        index.commit().unwrap();
        let hits = index.search("tantivy indexing", 5).unwrap();
        assert_eq!(hits[0].url, "https://b.example/title");
    }

    #[test]
    fn the_words_of_the_url_count() {
        let (_dir, index) = temp_index();
        index
            .add(&page("https://example.com/misc", "Page", "generic text"))
            .unwrap();
        index
            .add(&page(
                "https://docs.rs/serde-json/latest",
                "Page",
                "generic text",
            ))
            .unwrap();
        index.commit().unwrap();
        let hits = index.search("serde json", 5).unwrap();
        assert_eq!(hits[0].url, "https://docs.rs/serde-json/latest");
    }

    #[test]
    fn the_whole_phrase_outranks_scattered_words() {
        let (_dir, index) = temp_index();
        index
            .add(&page(
                "https://a.example/scattered",
                "Notes",
                "dict is a type. the get of other things. method names vary",
            ))
            .unwrap();
        index
            .add(&page(
                "https://b.example/phrase",
                "Notes",
                "use the dict get method to read a key safely",
            ))
            .unwrap();
        index.commit().unwrap();
        let hits = index.search("dict get method", 5).unwrap();
        assert_eq!(hits[0].url, "https://b.example/phrase");
    }

    #[test]
    fn a_title_made_only_of_stop_words_remains_searchable() {
        let (_dir, index) = temp_index();
        index
            .add(&page("https://example.com/band", "The Who", "Band history"))
            .unwrap();
        index.commit().unwrap();
        let hits = index.search("The Who", 5).unwrap();
        assert_eq!(hits[0].url, "https://example.com/band");
    }

    #[test]
    fn hits_carry_a_snippet_from_the_body() {
        let (_dir, index) = temp_index();
        index
            .add(&page(
                "https://example.com/p",
                "Title",
                "The quick brown fox jumps over the lazy dog.",
            ))
            .unwrap();
        index.commit().unwrap();
        let hits = index.search("quick fox", 5).unwrap();
        assert!(hits[0].snippet.starts_with("The quick brown fox"));
    }

    #[test]
    fn an_index_from_an_older_schema_is_reported_clearly() {
        let dir = TempDir::new().unwrap();
        let mut builder = Schema::builder();
        builder.add_text_field("url", STRING | STORED);
        Index::create_in_dir(dir.path(), builder.build()).unwrap();
        let error = LocalIndex::open(dir.path()).err().expect("must fail");
        assert!(error.contains("older version"), "{error}");
    }

    #[test]
    fn opening_an_empty_or_corrupt_index_never_repairs_it_implicitly() {
        let dir = TempDir::new().unwrap();
        assert!(LocalIndex::open(dir.path()).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        let metadata = dir.path().join("meta.json");
        std::fs::write(&metadata, "broken metadata").unwrap();
        assert!(LocalIndex::open(dir.path()).is_err());
        assert!(LocalIndex::open_for_write(dir.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(metadata).unwrap(),
            "broken metadata"
        );
    }

    #[test]
    fn recrawls_replace_a_url_and_noindex_removes_it() {
        let (_dir, index) = temp_index();
        let url = "https://docs.example/page";
        index.add(&page(url, "Obsolete", "obsolete text")).unwrap();
        index.add(&page(url, "Current", "current text")).unwrap();
        index.commit().unwrap();
        assert_eq!(index.document_count(), 1);
        assert!(index.search("obsolete", 10).unwrap().is_empty());
        assert_eq!(index.search("current", 10).unwrap()[0].title, "Current");

        index.add(&page(url, "Updated", "updated text")).unwrap();
        index.commit().unwrap();
        assert_eq!(index.document_count(), 1);
        assert!(index.search("current", 10).unwrap().is_empty());
        assert!(index.has_url(url));
        index.remove(url).unwrap();
        index.commit().unwrap();
        assert!(!index.has_url(url));
        assert_eq!(index.document_count(), 0);
    }

    #[test]
    fn technical_punctuation_is_text_not_query_syntax() {
        let (_dir, index) = temp_index();
        index
            .add(&page(
                "https://docs.example/arc",
                "Shared ownership",
                "std::sync::Arc reference counting",
            ))
            .unwrap();
        index
            .add(&page(
                "https://docs.example/literal",
                "Syntax",
                "title:secret is a literal field selector",
            ))
            .unwrap();
        index
            .add(&page(
                "https://docs.example/other",
                "secret",
                "unrelated information",
            ))
            .unwrap();
        index.commit().unwrap();
        let hits = index.search("std::sync::Arc", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].url.ends_with("/arc"));
        let hits = index.search("title:secret", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].url.ends_with("/literal"));
        assert!(index.search("ownership", 0).unwrap().is_empty());
    }

    #[test]
    fn mixed_language_queries_require_both_topics() {
        let (_dir, index) = temp_index();
        for (url, title, body) in [
            (
                "https://docs.example/rust",
                "Rust のメモリ管理",
                "所有権と借用",
            ),
            (
                "https://docs.example/ram",
                "メモリ メモリ メモリ",
                "ハードウェアの仕様",
            ),
            (
                "https://docs.example/python",
                "Python のメモリ管理",
                "オブジェクトの寿命",
            ),
        ] {
            index.add(&page(url, title, body)).unwrap();
        }
        index.commit().unwrap();
        let hits = index.search("Rust メモリ管理", 10).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].url.ends_with("/rust"));
    }

    #[test]
    fn mixed_scripts_without_spaces_still_match_existing_indexes() {
        let (_dir, index) = temp_index();
        index
            .add(&page(
                "https://docs.example/rust",
                "Rustの所有権",
                "借用の規則",
            ))
            .unwrap();
        index.commit().unwrap();
        assert_eq!(index.search("Rustの所有権", 10).unwrap().len(), 1);
        assert_eq!(index.search("Rust 所有権", 10).unwrap().len(), 1);
        assert!(index.search("Python 所有権", 10).unwrap().is_empty());
        assert!(
            index
                .search_range("規則", 10_000..10_001, &[], &[], false)
                .is_err()
        );
        assert!(index.search(&"語".repeat(257), 10).is_err());
        assert!(
            index
                .search_filtered("Rust", 10, &vec!["example.org".into(); 65], &[], false)
                .is_err()
        );
    }

    #[test]
    fn cjk_matches_need_more_than_one_shared_bigram() {
        let (_dir, index) = temp_index();
        index
            .add(&page(
                "https://docs.example/forecast",
                "東京の気象予報",
                "降水確率の解説",
            ))
            .unwrap();
        index
            .add(&page(
                "https://docs.example/map",
                "東京 東京 東京",
                "鉄道路線と地図",
            ))
            .unwrap();
        index
            .add(&page(
                "https://docs.example/news",
                "気象情報",
                "大阪のニュース",
            ))
            .unwrap();
        index.commit().unwrap();
        let hits = index.search("東京 気象予報", 10).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].url.ends_with("/forecast"));
    }

    #[test]
    fn phrase_boost_uses_positions_without_url_field_errors() {
        let (_dir, index) = temp_index();
        index
            .add(&page(
                "https://a.example/scattered",
                "Notes",
                "alpha filler beta",
            ))
            .unwrap();
        index
            .add(&page(
                "https://b.example/phrase",
                "Notes",
                "alpha beta filler",
            ))
            .unwrap();
        index.commit().unwrap();
        let hits = index.search("alpha beta", 10).unwrap();
        assert!(hits[0].url.ends_with("/phrase"), "{hits:?}");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn host_filters_are_applied_before_top_k() {
        let (_dir, index) = temp_index();
        for n in 0..25 {
            index
                .add(&page(
                    &format!("https://noise.example/{n}"),
                    "Actor mailbox",
                    "actor mailbox",
                ))
                .unwrap();
        }
        let target = "https://docs.target.example/guide";
        index
            .add(&page(
                target,
                "Guide",
                "an actor uses a mailbox to receive messages",
            ))
            .unwrap();
        index
            .add(&page(
                "https://target.example.evil.test/guide",
                "Actor mailbox",
                "actor mailbox",
            ))
            .unwrap();
        index.commit().unwrap();
        assert!(
            !index
                .search("actor mailbox", 20)
                .unwrap()
                .iter()
                .any(|hit| hit.url == target)
        );
        let hits = index
            .search_filtered(
                "actor mailbox",
                1,
                &["https://www.target.example/".into()],
                &[],
                false,
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, target);
        assert!(
            index
                .search_filtered(
                    "actor mailbox",
                    10,
                    &["target.example".into()],
                    &["docs.target.example".into()],
                    false,
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            index
                .search_filtered("actor mailbox", 10, &["".into()], &[], false)
                .unwrap()
                .is_empty()
        );
        let first = index
            .search_filtered("actor mailbox", 5, &[], &[], false)
            .unwrap();
        let next = index
            .search_range("actor mailbox", 5..10, &[], &[], false)
            .unwrap();
        assert_eq!(next.len(), 5);
        assert!(
            next.iter()
                .all(|hit| !first.iter().any(|previous| previous.url == hit.url))
        );
    }
}
