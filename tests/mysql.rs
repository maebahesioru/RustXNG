//! Run only against an isolated test database; CI provides a disposable service.
#![cfg(feature = "db-engines")]

use mysql::prelude::Queryable;
use rustxng::engine::{Engine, EngineItem, EngineRegistry};
use rustxng::engines::{mariadb_server::MariadbServer, mysql_server::MysqlServer};
use rustxng::query::Query;
use rustxng::settings::{EngineConfig, Settings};

#[test]
#[ignore = "requires an isolated MySQL-compatible server and MYSQL_TEST_PORT"]
fn mysql_and_mariadb_engines_bind_parameters_and_page_real_rows() {
    let port: u16 = std::env::var("MYSQL_TEST_PORT")
        .expect("MYSQL_TEST_PORT must identify an isolated fixture database")
        .parse()
        .unwrap();
    let opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some("127.0.0.1"))
        .tcp_port(port)
        .db_name(Some("rustxng_test"))
        .user(Some("rustxng"))
        .pass(Some("fixture-password"));
    let mut setup = mysql::Conn::new(opts).unwrap();
    let table = format!("fixture_docs_{}", std::process::id());
    setup.query_drop(format!("CREATE TABLE {table} (id INT PRIMARY KEY, title VARCHAR(64), body VARCHAR(255), optional_value VARCHAR(64))")).unwrap();
    setup.query_drop(format!("INSERT INTO {table} VALUES (1, 'First', 'rust', NULL), (2, 'Second', 'rust', 'present'), (3, 'Other', 'unrelated', NULL)")).unwrap();
    let registry = EngineRegistry::from_settings(&Settings::default()).unwrap();
    for engine_id in ["mysql_server", "mariadb_server"] {
        let mut config = EngineConfig::new("fixture", engine_id, "", &["general"]);
        for (key, value) in [
            ("host", "127.0.0.1".to_string()),
            ("database", "rustxng_test".into()),
            ("username", "rustxng".into()),
            ("password", "fixture-password".into()),
            (
                "query_str",
                format!(
                    "SELECT title, body, optional_value FROM {table} WHERE body = ? ORDER BY id"
                ),
            ),
        ] {
            config.extra.insert(key.into(), toml::Value::String(value));
        }
        config
            .extra
            .insert("port".into(), toml::Value::Integer(port.into()));
        config.extra.insert("limit".into(), toml::Value::Integer(1));
        let engine: std::sync::Arc<dyn Engine> = if engine_id == "mysql_server" {
            MysqlServer::from_config(&config).unwrap()
        } else {
            MariadbServer::from_config(&config).unwrap()
        };
        let query = Query::parse("rust", &registry, &[]);
        for (page, title) in [(1, "First"), (2, "Second")] {
            let items = engine.search_offline(&query, page).unwrap();
            assert_eq!(items.len(), 1);
            match &items[0] {
                EngineItem::Result(result) => assert_eq!(result.title, title),
                _ => panic!("expected a database row"),
            }
        }
        assert!(engine.search_offline(&query, 3).unwrap().is_empty());
        let injection = Query::parse("' OR 1=1 --", &registry, &[]);
        assert!(
            engine.search_offline(&injection, 1).unwrap().is_empty(),
            "query text must be bound as data"
        );
    }
    setup.query_drop(format!("DROP TABLE {table}")).unwrap();
}
