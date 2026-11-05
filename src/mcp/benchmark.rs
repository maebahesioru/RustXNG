//! Offline comparison using retained live responses; never sends a request.
use super::*;
use crate::aggregator::SearchResponse;
use crate::results::{SearchResult, UnresponsiveEngine};
use serde_json::{Value, json};
use std::time::Instant;

/// Run explicitly with RUSTXNG_BUDGET_INPUT and RUSTXNG_BUDGET_OUTPUT.
/// Input is an uncompressed eval_quality --retain-payload report.
#[test]
#[ignore = "offline artifact generation; requires retained evaluation responses"]
fn replay_payload_budgets() {
    let input = std::env::var("RUSTXNG_BUDGET_INPUT").expect("input report path");
    let output = std::env::var("RUSTXNG_BUDGET_OUTPUT").expect("output artifact path");
    let report: Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let encodings = ["cl100k_base", "o200k_base"];
    // Exclude one-time embedded-vocabulary loading from steady-state packing.
    for encoding in encodings {
        let _ = tiktoken::get_encoding(encoding).unwrap().count("warm up");
    }
    let mut rows = Vec::new();
    for row in report["rows"].as_array().unwrap() {
        let payload = &row["payload"];
        let query = Query::parse(
            row["query"].as_str().unwrap(),
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        let mut response = SearchResponse::new(&query);
        // The HTTP compatibility serializer flattens metadata and templates.
        // Restore the typed source results, then use the production projection.
        response.results = payload["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                let mut item = item.clone();
                item.as_object_mut().unwrap().remove("template");
                if item["priority"].as_str() == Some("") {
                    item.as_object_mut().unwrap().remove("priority");
                }
                let metadata = item.as_object_mut().unwrap().remove("metadata");
                let mut result: SearchResult = serde_json::from_value(item).unwrap();
                if let Some(Value::String(metadata)) = metadata {
                    for pair in metadata.split(" | ") {
                        if let Some((key, value)) = pair.split_once(": ") {
                            result.metadata.insert(key.into(), value.into());
                        }
                    }
                }
                result
            })
            .collect();
        response.paging = payload["paging"].as_bool().unwrap_or(false);
        response.unresponsive_engines = payload["unresponsive_engines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| UnresponsiveEngine {
                engine: entry[0].as_str().unwrap_or_default().into(),
                error_type: entry[1].as_str().unwrap_or_default().into(),
                suspended: false,
            })
            .collect();
        let raw = payload.to_string();
        rows.push(json!({"id":row["id"], "condition":"raw_http_canonical_json",
            "urls":row["urls"], "gold":row["gold"], "bytes":row["bytes"],
            "retrieval_ms":row["millis"], "canonical_bytes":raw.len(),
            "tokens":encodings.map(|e| (e, tiktoken::get_encoding(e).unwrap().count(&raw))).into_iter().collect::<std::collections::BTreeMap<_,_>>() }));
        for name in ["compact", "cl100k_base", "o200k_base"] {
            let budget = if name == "compact" {
                TokenBudget::default()
            } else {
                TokenBudget::parse(name, 4096).unwrap()
            };
            let mut times = Vec::new();
            let mut last = None;
            for _ in 0..10 {
                let started = Instant::now();
                let mut packed = compact_search(&response, 0, 10, budget);
                fit_search_payload(
                    &mut packed,
                    default_payload_chars(),
                    default_payload_bytes(),
                    budget,
                )
                .unwrap();
                let canonical = serde_json::to_value(&packed).unwrap().to_string();
                times.push(started.elapsed().as_secs_f64() * 1000.0);
                last = Some((packed, canonical));
            }
            let (packed, canonical) = last.unwrap();
            let leading_evidence = packed
                .results
                .iter()
                .zip(&response.results)
                .filter(|(packed, source)| {
                    let prefix: String = source.content.chars().take(80).collect();
                    packed.snippet.starts_with(&prefix)
                })
                .count();
            rows.push(json!({"id":row["id"], "condition":name,
                "urls":packed.results.iter().map(|item| &item.url).collect::<Vec<_>>(),
                "gold":row["gold"], "bytes":canonical.len(), "packing_ms":times,
                "source_count":response.results.len(), "leading_80_chars_preserved":leading_evidence,
                "truncated":packed.truncated, "next_offset":packed.next_offset,
                "tokens":encodings.map(|e| (e, tiktoken::get_encoding(e).unwrap().count(&canonical))).into_iter().collect::<std::collections::BTreeMap<_,_>>() }));
        }
    }
    let artifact = json!({"tokenizer_implementation":"tiktoken 4.1.2 (Rust)",
        "count_scope":"Compact sorted-key structured JSON, ordinary text, excluding MCP envelopes, text-content copy, SDK formatting, caller prompt and model framing. Raw token counts use canonicalized retained HTTP JSON; raw bytes are measured wire bytes.",
        "protocol":"Frozen live responses; production compact projection and budget fitter; 10 steady-state packing samples per case; unchanged URLs/gold; no upstream calls, no answer-generation model. Leading 80 snippet characters are an evidence-retention proxy, not answer correctness.",
        "rows":rows});
    std::fs::write(output, serde_json::to_vec_pretty(&artifact).unwrap()).unwrap();
}
