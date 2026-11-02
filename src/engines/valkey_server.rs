//! Valkey / Redis engine, ported from searx/engines/valkey_server.py.
//!
//! Talks the RESP protocol directly, so no client dependency is needed.
//! Upstream is an offline engine: it looks keys up in a Valkey/Redis instance.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

// engine-id: valkey_server

/// Upper bound on the SCAN/HSCAN pages one search follows. Upstream stops when
/// the server returns cursor 0; a server that never does must not hold the
/// request open. Hitting the budget fails the search, so a truncated key set
/// never passes for a complete one.
const SCAN_PAGE_BUDGET: usize = 32;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// A Valkey / Redis key value store.
pub struct ValkeyServer {
    meta: EngineMeta,
    host: String,
    port: u16,
    password: String,
    db: i64,
    exact_match_only: bool,
}

impl ValkeyServer {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let host = meta.opt_str("host").unwrap_or("127.0.0.1").to_string();
        let port = meta
            .opt_i64("port")
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(6379);
        let password = meta.opt_str("password").unwrap_or("").to_string();
        let db = meta.opt_i64("db").unwrap_or(0);
        let exact_match_only = meta.opt_bool("exact_match_only").unwrap_or(true);
        Ok(Arc::new(Self {
            meta,
            host,
            port,
            password,
            db,
            exact_match_only,
        }))
    }

    fn connect(&self) -> Result<Connection, EngineError> {
        let timeout = self.meta.timeout.max(Duration::from_millis(100));
        let stream = TcpStream::connect((self.host.as_str(), self.port)).map_err(|error| {
            EngineError::Message(format!(
                "valkey: cannot connect to {}:{}: {error}",
                self.host, self.port
            ))
        })?;
        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_write_timeout(Some(timeout)).ok();
        let mut connection = Connection {
            reader: BufReader::new(
                stream
                    .try_clone()
                    .map_err(|error| EngineError::Message(format!("valkey: {error}")))?,
            ),
            writer: stream,
            deadline: Instant::now() + timeout,
        };
        if !self.password.is_empty() {
            connection.command(&["AUTH", &self.password])?;
        }
        if self.db != 0 {
            let db = self.db.to_string();
            connection.command(&["SELECT", &db])?;
        }
        Ok(connection)
    }

    /// Every key/value row, like upstream search_keys().
    fn search_keys(
        &self,
        connection: &mut Connection,
        query: &str,
    ) -> Result<Vec<BTreeMap<String, String>>, EngineError> {
        let pattern = format!("*{query}*");
        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = "0".to_string();
        let mut complete = false;
        for _ in 0..SCAN_PAGE_BUDGET {
            let reply =
                connection.command(&["SCAN", &cursor, "MATCH", &pattern, "COUNT", "100"])?;
            let Some((next, keys)) = cursor_page(reply) else {
                break;
            };
            for key in keys {
                let Reply::Bulk(Some(key)) = key else {
                    continue;
                };
                if !seen.insert(key.clone()) {
                    continue;
                }
                let kind = match connection.command(&["TYPE", &key])? {
                    Reply::Simple(value) | Reply::Bulk(Some(value)) => value,
                    _ => continue,
                };
                let mut map = BTreeMap::new();
                match kind.as_str() {
                    "hash" => {
                        if let Reply::Array(pairs) = connection.command(&["HGETALL", &key])? {
                            for chunk in pairs.chunks(2) {
                                if let [Reply::Bulk(Some(name)), Reply::Bulk(Some(value))] = chunk {
                                    map.insert(name.clone(), value.clone());
                                }
                            }
                        }
                    }
                    "list" => {
                        if let Reply::Array(items) =
                            connection.command(&["LRANGE", &key, "0", "-1"])?
                        {
                            for (index, item) in items.iter().enumerate() {
                                if let Reply::Bulk(Some(value)) = item {
                                    map.insert(index.to_string(), value.clone());
                                }
                            }
                        }
                    }
                    _ => continue,
                }
                if !map.is_empty() {
                    map.insert("valkey_key".to_string(), key.clone());
                    rows.push(map);
                }
            }
            if next == "0" {
                complete = true;
                break;
            }
            cursor = next;
        }
        if !complete {
            return Err(EngineError::Message(format!(
                "valkey: SCAN stopped after {SCAN_PAGE_BUDGET} pages before cursor 0"
            )));
        }
        Ok(rows)
    }
}

impl Engine for ValkeyServer {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Err(EngineError::Message(
            "valkey_server is an offline engine".into(),
        ))
    }

    fn parse(
        &self,
        _response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        Ok(Vec::new())
    }

    fn is_offline(&self) -> bool {
        true
    }

    fn search_offline(&self, query: &Query, _pageno: u32) -> Result<Vec<EngineItem>, EngineError> {
        let text = query.query_text();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let mut connection = self.connect()?;
        if !self.exact_match_only {
            return Ok(rows_to_items(self.search_keys(&mut connection, &text)?));
        }
        let mut rows = Vec::new();
        if let Reply::Array(pairs) = connection.command(&["HGETALL", &text])? {
            let mut map = BTreeMap::new();
            for chunk in pairs.chunks(2) {
                if let [Reply::Bulk(Some(name)), Reply::Bulk(Some(value))] = chunk {
                    map.insert(name.clone(), value.clone());
                }
            }
            if !map.is_empty() {
                rows.push(map);
            }
        }
        if rows.is_empty() {
            if let Some((set, rest)) = text.split_once(' ') {
                let pattern = format!("*{rest}*");
                let mut seen = HashSet::new();
                let mut cursor = "0".to_string();
                let mut complete = false;
                for _ in 0..SCAN_PAGE_BUDGET {
                    let reply = connection.command(&["HSCAN", set, &cursor, "MATCH", &pattern])?;
                    let Some((next, pairs)) = cursor_page(reply) else {
                        break;
                    };
                    for chunk in pairs.chunks(2) {
                        if let [Reply::Bulk(Some(name)), Reply::Bulk(Some(value))] = chunk {
                            if !seen.insert(name.clone()) {
                                continue;
                            }
                            let mut map = BTreeMap::new();
                            map.insert(name.clone(), value.clone());
                            rows.push(map);
                        }
                    }
                    if next == "0" {
                        complete = true;
                        break;
                    }
                    cursor = next;
                }
                if !complete {
                    return Err(EngineError::Message(format!(
                        "valkey: HSCAN stopped after {SCAN_PAGE_BUDGET} pages before cursor 0"
                    )));
                }
            }
        }
        Ok(rows_to_items(rows))
    }
}

fn rows_to_items(rows: Vec<BTreeMap<String, String>>) -> Vec<EngineItem> {
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let title = row
                .get("valkey_key")
                .cloned()
                .or_else(|| row.keys().next().cloned())
                .unwrap_or_else(|| format!("row {}", index + 1));
            let mut builder =
                crate::engine::util::ResultBuilder::new(format!("keyvalue:{index}"), title)
                    .template(Template::KeyValue);
            for (key, value) in &row {
                builder = builder.metadata(key, value);
            }
            builder.build()
        })
        .collect()
}

/// Split one SCAN/HSCAN reply into its next cursor and the returned elements.
fn cursor_page(reply: Reply) -> Option<(String, Vec<Reply>)> {
    let Reply::Array(mut items) = reply else {
        return None;
    };
    if items.len() != 2 {
        return None;
    }
    let Reply::Array(elements) = items.pop()? else {
        return None;
    };
    let cursor = match items.pop()? {
        Reply::Bulk(Some(cursor)) | Reply::Simple(cursor) => cursor,
        _ => return None,
    };
    Some((cursor, elements))
}

/// One RESP reply.
#[derive(Debug, Clone)]
enum Reply {
    Simple(String),
    Bulk(Option<String>),
    #[allow(dead_code)]
    Integer(i64),
    Array(Vec<Reply>),
    Nil,
}

struct Connection {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    /// The whole search shares this deadline; the per-read socket timeout
    /// cannot bound a sequence of commands on its own.
    deadline: Instant,
}

impl Connection {
    fn command(&mut self, args: &[&str]) -> Result<Reply, EngineError> {
        if Instant::now() >= self.deadline {
            return Err(EngineError::Message(
                "valkey: search deadline exceeded".into(),
            ));
        }
        let mut request = format!("*{}\r\n", args.len());
        for argument in args {
            request.push_str(&format!("${}\r\n{}\r\n", argument.len(), argument));
        }
        self.writer
            .write_all(request.as_bytes())
            .map_err(|error| EngineError::Message(format!("valkey: {error}")))?;
        self.writer
            .flush()
            .map_err(|error| EngineError::Message(format!("valkey: {error}")))?;
        self.read_reply()
    }

    fn read_reply(&mut self) -> Result<Reply, EngineError> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .map_err(|error| EngineError::Message(format!("valkey: {error}")))?;
        if line.is_empty() {
            return Err(EngineError::Message("valkey: connection closed".into()));
        }
        let marker = line.chars().next().unwrap_or(' ');
        let rest = line[1..].trim_end().to_string();
        Ok(match marker {
            '+' => Reply::Simple(rest),
            '-' => return Err(EngineError::Message(format!("valkey: {rest}"))),
            ':' => Reply::Integer(rest.parse().unwrap_or(0)),
            '$' => {
                let length: i64 = rest.parse().unwrap_or(-1);
                if length < 0 {
                    Reply::Nil
                } else {
                    let mut buffer = vec![0u8; length as usize + 2];
                    self.reader
                        .read_exact(&mut buffer)
                        .map_err(|error| EngineError::Message(format!("valkey: {error}")))?;
                    buffer.truncate(length as usize);
                    Reply::Bulk(Some(String::from_utf8_lossy(&buffer).to_string()))
                }
            }
            '*' => {
                let count: i64 = rest.parse().unwrap_or(0);
                let mut items = Vec::new();
                for _ in 0..count.max(0) {
                    items.push(self.read_reply()?);
                }
                Reply::Array(items)
            }
            _ => Reply::Nil,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::thread;

    /// Start a stub server that answers the engine in script order and records
    /// the raw request bytes. Commands are parsed leniently so malformed bytes
    /// still get an answer; the recorded raw request is what the tests assert.
    fn stub(replies: Vec<&'static str>) -> (u16, Arc<Mutex<Vec<String>>>) {
        stub_delayed(replies, Duration::ZERO)
    }

    fn stub_delayed(replies: Vec<&'static str>, delay: Duration) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut writer = stream;
            for reply in replies {
                let Some(request) = read_request(&mut reader) else {
                    break;
                };
                recorded.lock().expect("lock").push(request);
                thread::sleep(delay);
                if writer.write_all(reply.as_bytes()).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        });
        (port, requests)
    }

    fn read_request(reader: &mut impl BufRead) -> Option<String> {
        let mut request = String::new();
        let count = resp_line(reader, &mut request)?;
        let count: usize = count.trim_start_matches('*').parse().ok()?;
        for _ in 0..count {
            let header = resp_line(reader, &mut request)?;
            let digits: String = header
                .trim_start_matches('$')
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            let length: usize = digits.parse().ok()?;
            let mut payload = vec![0u8; length + 2];
            reader.read_exact(&mut payload).ok()?;
            request.push_str(&String::from_utf8_lossy(&payload));
        }
        Some(request)
    }

    fn resp_line(reader: &mut impl BufRead, request: &mut String) -> Option<String> {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        request.push_str(&line);
        Some(line.trim_end().to_string())
    }

    fn engine_at(port: u16, password: &str, exact_match_only: bool) -> Arc<ValkeyServer> {
        engine_at_with_timeout(port, password, exact_match_only, DEFAULTS.timeout)
    }

    fn engine_at_with_timeout(
        port: u16,
        password: &str,
        exact_match_only: bool,
        timeout: f64,
    ) -> Arc<ValkeyServer> {
        let mut config = EngineConfig::new("valkey", "valkey_server", "", &["general"]);
        config.timeout = Some(timeout);
        config
            .extra
            .insert("host".into(), toml::Value::String("127.0.0.1".into()));
        config
            .extra
            .insert("port".into(), toml::Value::Integer(i64::from(port)));
        config
            .extra
            .insert("password".into(), toml::Value::String(password.into()));
        config.extra.insert(
            "exact_match_only".into(),
            toml::Value::Boolean(exact_match_only),
        );
        ValkeyServer::from_config(&config).expect("engine")
    }

    fn parsed(query: &str) -> Query {
        Query::parse(query, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn auth_and_hgetall_send_valid_resp_bulk_headers() {
        let (port, requests) = stub(vec!["+OK\r\n", "*2\r\n$4\r\nname\r\n$3\r\nAda\r\n"]);
        let engine = engine_at(port, "hunter2", true);
        let items = engine.search_offline(&parsed("user1"), 1).expect("search");
        assert_eq!(items.len(), 1);
        assert_eq!(
            *requests.lock().expect("lock"),
            vec![
                "*2\r\n$4\r\nAUTH\r\n$7\r\nhunter2\r\n".to_string(),
                "*2\r\n$7\r\nHGETALL\r\n$5\r\nuser1\r\n".to_string(),
            ]
        );
    }

    #[test]
    fn scan_sends_valid_resp_bulk_headers() {
        let (port, requests) = stub(vec!["*2\r\n$1\r\n0\r\n*0\r\n"]);
        let engine = engine_at(port, "", false);
        let items = engine.search_offline(&parsed("user1"), 1).expect("search");
        assert!(items.is_empty());
        assert_eq!(
            *requests.lock().expect("lock"),
            vec![
                "*6\r\n$4\r\nSCAN\r\n$1\r\n0\r\n$5\r\nMATCH\r\n$7\r\n*user1*\r\n$5\r\nCOUNT\r\n$3\r\n100\r\n"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn scan_follows_the_cursor_to_zero() {
        let (port, requests) = stub(vec![
            "*2\r\n$1\r\n7\r\n*1\r\n$6\r\nuser:1\r\n",
            "+hash\r\n",
            "*4\r\n$4\r\nname\r\n$3\r\nAda\r\n$4\r\ncity\r\n$6\r\nLondon\r\n",
            "*2\r\n$1\r\n0\r\n*1\r\n$6\r\nuser:2\r\n",
            "+string\r\n",
        ]);
        let engine = engine_at(port, "", false);
        let items = engine.search_offline(&parsed("user1"), 1).expect("search");
        assert_eq!(items.len(), 1);
        let requests = requests.lock().expect("lock").clone();
        assert_eq!(requests.len(), 5, "one SCAN per page then TYPE per key");
        assert!(requests[0].starts_with("*6\r\n$4\r\nSCAN\r\n$1\r\n0\r\n"));
        assert!(requests[3].starts_with("*6\r\n$4\r\nSCAN\r\n$1\r\n7\r\n"));
    }

    #[test]
    fn hscan_follows_the_cursor_to_zero() {
        let (port, requests) = stub(vec![
            "*0\r\n",
            "*2\r\n$1\r\n3\r\n*2\r\n$4\r\nname\r\n$3\r\nAda\r\n",
            "*2\r\n$1\r\n0\r\n*2\r\n$4\r\ncity\r\n$6\r\nLondon\r\n",
        ]);
        let engine = engine_at(port, "", true);
        let items = engine
            .search_offline(&parsed("people ada"), 1)
            .expect("search");
        assert_eq!(items.len(), 2);
        let requests = requests.lock().expect("lock").clone();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].starts_with("*5\r\n$5\r\nHSCAN\r\n$6\r\npeople\r\n"));
        assert!(requests[1].contains("$1\r\n0\r\n$5\r\nMATCH"));
        assert!(requests[2].contains("$1\r\n3\r\n$5\r\nMATCH"));
    }

    #[test]
    fn scan_stops_after_the_page_budget() {
        let mut replies = vec!["*2\r\n$1\r\n1\r\n*1\r\n$6\r\nuser:1\r\n", "+string\r\n"];
        replies.extend(std::iter::repeat_n(
            "*2\r\n$1\r\n1\r\n*1\r\n$6\r\nuser:1\r\n",
            SCAN_PAGE_BUDGET - 1,
        ));
        let (port, requests) = stub(replies);
        let engine = engine_at(port, "", false);
        let error = engine
            .search_offline(&parsed("user1"), 1)
            .expect_err("the budget must fail the search");
        assert!(error.to_string().contains("SCAN stopped after"), "{error}");
        let requests = requests.lock().expect("lock").clone();
        let scans = requests
            .iter()
            .filter(|request| request.contains("SCAN"))
            .count();
        assert_eq!(scans, SCAN_PAGE_BUDGET);
    }

    #[test]
    fn hscan_stops_after_the_page_budget() {
        let mut replies = vec!["*0\r\n"];
        replies.extend(std::iter::repeat_n(
            "*2\r\n$1\r\n1\r\n*2\r\n$4\r\nname\r\n$3\r\nAda\r\n",
            SCAN_PAGE_BUDGET,
        ));
        let (port, requests) = stub(replies);
        let engine = engine_at(port, "", true);
        let error = engine
            .search_offline(&parsed("people ada"), 1)
            .expect_err("the budget must fail the search");
        assert!(error.to_string().contains("HSCAN stopped after"), "{error}");
        let requests = requests.lock().expect("lock").clone();
        let scans = requests
            .iter()
            .filter(|request| request.contains("HSCAN"))
            .count();
        assert_eq!(scans, SCAN_PAGE_BUDGET);
    }

    #[test]
    fn scan_returns_a_key_once_across_pages() {
        let (port, requests) = stub(vec![
            "*2\r\n$1\r\n1\r\n*2\r\n$6\r\nuser:1\r\n$6\r\nuser:1\r\n",
            "+hash\r\n",
            "*4\r\n$4\r\nname\r\n$3\r\nAda\r\n$4\r\ncity\r\n$6\r\nLondon\r\n",
            "*2\r\n$1\r\n0\r\n*0\r\n",
        ]);
        let engine = engine_at(port, "", false);
        let items = engine.search_offline(&parsed("user1"), 1).expect("search");
        assert_eq!(items.len(), 1);
        let requests = requests.lock().expect("lock").clone();
        assert_eq!(requests.len(), 4, "the duplicate key is not typed again");
    }

    #[test]
    fn hscan_returns_a_field_once_across_pages() {
        let (port, _) = stub(vec![
            "*0\r\n",
            "*2\r\n$1\r\n1\r\n*2\r\n$4\r\nname\r\n$3\r\nAda\r\n",
            "*2\r\n$1\r\n0\r\n*2\r\n$4\r\nname\r\n$3\r\nAda\r\n",
        ]);
        let engine = engine_at(port, "", true);
        let items = engine
            .search_offline(&parsed("people ada"), 1)
            .expect("search");
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn search_stops_at_the_connection_deadline() {
        let mut replies = Vec::new();
        for _ in 0..SCAN_PAGE_BUDGET {
            replies.push("*2\r\n$1\r\n1\r\n*1\r\n$6\r\nuser:1\r\n");
            replies.push("+string\r\n");
        }
        let (port, requests) = stub_delayed(replies, Duration::from_millis(120));
        let engine = engine_at_with_timeout(port, "", false, 0.2);
        let error = engine
            .search_offline(&parsed("user1"), 1)
            .expect_err("the deadline must stop the search");
        assert!(error.to_string().contains("deadline"), "{error}");
        let scans = requests
            .lock()
            .expect("lock")
            .iter()
            .filter(|request| request.contains("SCAN"))
            .count();
        assert!(scans < SCAN_PAGE_BUDGET, "scans: {scans}");
    }

    fn engine() -> Arc<ValkeyServer> {
        let mut config = EngineConfig::new("valkey", "valkey_server", "", &["general"]);
        config
            .extra
            .insert("host".into(), toml::Value::String("127.0.0.1".into()));
        ValkeyServer::from_config(&config).expect("engine")
    }

    #[test]
    fn reads_the_connection_options() {
        let engine = engine();
        assert_eq!(engine.host, "127.0.0.1");
        assert_eq!(engine.port, 6379);
        assert!(engine.exact_match_only);
        assert!(engine.is_offline());
    }

    #[test]
    fn rows_become_keyvalue_results() {
        let mut row = BTreeMap::new();
        row.insert("valkey_key".to_string(), "user:1".to_string());
        row.insert("name".to_string(), "Ada".to_string());
        let items = rows_to_items(vec![row]);
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "user:1");
                assert_eq!(result.template, Template::KeyValue);
                assert_eq!(result.metadata.get("name").map(String::as_str), Some("Ada"));
            }
            _ => panic!("expected a result"),
        }
    }
}
