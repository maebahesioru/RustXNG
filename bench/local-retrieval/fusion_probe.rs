use rustxng::engine::EngineItem;
use rustxng::results::{ResultContainer, SearchResult};

fn result(url: &str, title: &str) -> EngineItem {
    EngineItem::Result(SearchResult::new(url, title))
}

fn rows(container: &ResultContainer) -> serde_json::Value {
    serde_json::Value::Array(
        container
            .ordered()
            .iter()
            .map(|result| {
                serde_json::json!({
                    "url": result.url,
                    "score": result.score,
                    "engines": result.engines,
                    "positions": result.positions,
                })
            })
            .collect(),
    )
}

fn main() {
    let mut routed = ResultContainer::new();
    routed.extend(
        "ordinary",
        1.0,
        vec![result("https://example.test/ordinary", "ordinary")],
    );
    routed.extend(
        "routed",
        2.0,
        vec![result("https://example.test/routed", "routed")],
    );
    routed.close(|_| 1.0, &[], 0.0, "", 0.0);

    let mut repeated = ResultContainer::new();
    repeated.extend(
        "one_source",
        1.0,
        vec![
            result("https://example.test/repeated", "first spelling"),
            result("http://example.test/repeated#heading", "second spelling"),
            result(
                "https://example.test/repeated?utm_source=tracking",
                "third spelling",
            ),
        ],
    );
    repeated.extend(
        "independent_a",
        1.0,
        vec![result("https://example.test/corroborated", "corroborated")],
    );
    repeated.extend(
        "independent_b",
        1.0,
        vec![result("https://example.test/corroborated", "corroborated")],
    );
    repeated.close(|_| 1.0, &[], 0.0, "", 0.0);

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "routed_weight": rows(&routed),
            "repeated_source": rows(&repeated),
        }))
        .unwrap()
    );
}
