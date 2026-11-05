use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use rustxng::localindex::{LocalIndex, Page, SCHEMA_VERSION};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Fixture {
    name: String,
    scope: String,
    documents: Vec<Document>,
    queries: Vec<Query>,
}

#[derive(Deserialize)]
struct Document {
    url: String,
    title: String,
    body: String,
    lang: String,
}

#[derive(Deserialize)]
struct Query {
    id: String,
    query: String,
    gold_urls: Vec<String>,
}

struct Options {
    fixture: PathBuf,
    repeats: usize,
    warmup: usize,
    loose: bool,
    label: String,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut options = Self {
            fixture: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("bench/local-retrieval/development.json"),
            repeats: 200,
            warmup: 5,
            loose: false,
            label: String::new(),
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let value = args.next().ok_or_else(|| {
                "usage: local_index_eval [--fixture PATH] [--repeats N] [--warmup N] [--mode strict|loose] [--label TEXT]".to_string()
            })?;
            match arg.as_str() {
                "--fixture" => options.fixture = value.into(),
                "--label" => options.label = value,
                "--repeats" => options.repeats = value.parse().map_err(|_| "invalid repeats")?,
                "--warmup" => options.warmup = value.parse().map_err(|_| "invalid warmup")?,
                "--mode" => match value.as_str() {
                    "strict" => options.loose = false,
                    "loose" => options.loose = true,
                    _ => return Err("mode must be strict or loose".into()),
                },
                _ => return Err(format!("unknown option: {arg}")),
            }
        }
        if options.repeats == 0 {
            return Err("repeats must be positive".into());
        }
        Ok(options)
    }
}

impl Fixture {
    fn validate(&self) -> Result<(), String> {
        if self.documents.is_empty() || self.queries.is_empty() {
            return Err("fixture needs documents and queries".into());
        }
        let mut urls = HashSet::new();
        for document in &self.documents {
            if document.url.is_empty() || !urls.insert(document.url.as_str()) {
                return Err(format!("empty or duplicate document URL: {}", document.url));
            }
        }
        let mut ids = HashSet::new();
        for query in &self.queries {
            if query.id.is_empty()
                || !ids.insert(query.id.as_str())
                || query.query.trim().is_empty()
            {
                return Err(format!("empty or duplicate query ID/text: {}", query.id));
            }
            if query.gold_urls.is_empty()
                || query
                    .gold_urls
                    .iter()
                    .any(|url| !urls.contains(url.as_str()))
            {
                return Err(format!("missing gold document for query: {}", query.id));
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct Measurement {
    latency_ms: Vec<f64>,
    first_results: Option<Value>,
    first_rank: Option<usize>,
    first_error: Option<String>,
    result_variations: usize,
    errors: usize,
    response_bytes: usize,
}

fn percentile(values: &[f64], percent: usize) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(sorted.len() * percent).div_ceil(100).saturating_sub(1)]
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn quality(ranks: &[Option<usize>]) -> Value {
    let total = ranks.len() as f64;
    let hit_at = |k| {
        ranks
            .iter()
            .filter(|rank| rank.is_some_and(|r| r <= k))
            .count()
    };
    json!({
        "hit_at_1": hit_at(1), "hit_at_5": hit_at(5), "hit_at_10": hit_at(10),
        "mrr_at_10": ranks.iter().flatten().map(|rank| 1.0 / *rank as f64).sum::<f64>() / total,
        "queries": ranks.len(),
    })
}

fn run(options: Options) -> Result<(), String> {
    let fixture_bytes = std::fs::read(&options.fixture).map_err(|error| error.to_string())?;
    let fixture: Fixture =
        serde_json::from_slice(&fixture_bytes).map_err(|error| error.to_string())?;
    fixture.validate()?;
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let index_start = Instant::now();
    let index = LocalIndex::open_for_write(directory.path())?;
    for document in &fixture.documents {
        index.add(&Page {
            url: &document.url,
            title: &document.title,
            body: &document.body,
            lang: &document.lang,
            snippet: "",
        })?;
    }
    index.commit()?;
    let indexing_ms = index_start.elapsed().as_secs_f64() * 1000.0;
    let search = |query: &str| {
        if options.loose {
            index.search_loose(query, 10)
        } else {
            index.search(query, 10)
        }
    };
    for _ in 0..options.warmup {
        for query in &fixture.queries {
            let _ = search(&query.query);
        }
    }
    let mut measurements: Vec<Measurement> = (0..fixture.queries.len())
        .map(|_| Measurement::default())
        .collect();
    for iteration in 0..options.repeats {
        for offset in 0..fixture.queries.len() {
            let position = (offset + iteration) % fixture.queries.len();
            let query = &fixture.queries[position];
            let measurement = &mut measurements[position];
            let start = Instant::now();
            let result = search(&query.query);
            measurement
                .latency_ms
                .push(start.elapsed().as_secs_f64() * 1000.0);
            let (results, rank, error) = match result {
                Ok(hits) => {
                    let rank = hits
                        .iter()
                        .position(|hit| query.gold_urls.contains(&hit.url))
                        .map(|i| i + 1);
                    let results = json!(hits.iter().map(|hit| json!({
                        "url": hit.url, "title": hit.title, "snippet": hit.snippet, "score": hit.score,
                    })).collect::<Vec<_>>());
                    (results, rank, None)
                }
                Err(error) => {
                    measurement.errors += 1;
                    (json!([]), None, Some(error))
                }
            };
            if iteration == 0 {
                measurement.response_bytes = serde_json::to_vec(&results)
                    .map_err(|error| error.to_string())?
                    .len();
                measurement.first_results = Some(results);
                measurement.first_rank = rank;
                measurement.first_error = error;
            } else if measurement.first_results.as_ref() != Some(&results)
                || measurement.first_error != error
            {
                measurement.result_variations += 1;
            }
        }
    }
    let all_latencies: Vec<_> = measurements
        .iter()
        .flat_map(|row| row.latency_ms.iter().copied())
        .collect();
    let ranks: Vec<_> = measurements.iter().map(|row| row.first_rank).collect();
    let rows: Vec<_> = fixture.queries.iter().zip(&measurements).map(|(query, row)| json!({
        "id": query.id, "query": query.query, "gold_urls": query.gold_urls,
        "rank_at_10": row.first_rank, "results": row.first_results,
        "first_error": row.first_error, "errors": row.errors,
        "result_variations": row.result_variations, "result_bytes": row.response_bytes,
        "p50_ms": percentile(&row.latency_ms, 50), "p95_ms": percentile(&row.latency_ms, 95),
        "latency_ms": row.latency_ms,
    })).collect();
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let binary_bytes = std::fs::read(executable).map_err(|error| error.to_string())?;
    let report = json!({
        "format_version": 1,
        "metadata": {
            "label": options.label, "fixture": fixture.name, "scope": fixture.scope,
            "fixture_sha256": sha256(&fixture_bytes),
            "binary_sha256": sha256(&binary_bytes),
            "package_version": env!("CARGO_PKG_VERSION"), "schema_version": SCHEMA_VERSION,
            "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "parallelism": std::thread::available_parallelism().map(|value| value.get()).ok(),
            "mode": if options.loose { "loose" } else { "strict" }, "limit": 10,
            "repeats": options.repeats, "warmup": options.warmup,
            "cache": "one fresh temporary index; warmup queries before measurement; serial rotating query order",
            "timing": "LocalIndex search only, includes failures, excludes serialization and indexing; nearest-rank percentiles",
            "quality": "exact fixture URL match in first measured result; errors count as misses; inspect result_variations",
            "live_provider_comparison": false,
        },
        "summary": {
            "quality": quality(&ranks), "documents": index.document_count(), "indexing_ms": indexing_ms,
            "measurements": all_latencies.len(),
            "p50_ms": percentile(&all_latencies, 50), "p95_ms": percentile(&all_latencies, 95),
            "errors": measurements.iter().map(|row| row.errors).sum::<usize>(),
            "result_variations": measurements.iter().map(|row| row.result_variations).sum::<usize>(),
        },
        "rows": rows,
    });
    serde_json::to_writer_pretty(std::io::stdout().lock(), &report)
        .map_err(|error| error.to_string())?;
    println!();
    Ok(())
}

fn main() {
    if let Err(error) = Options::parse().and_then(run) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn misses_remain_in_quality_denominator() {
        let result = quality(&[Some(1), None, Some(5), None]);
        assert_eq!(result["hit_at_1"], 1);
        assert_eq!(result["hit_at_5"], 2);
        assert_eq!(result["mrr_at_10"], 0.3);
        assert_eq!(result["queries"], 4);
    }

    #[test]
    fn nearest_rank_percentiles_include_tail() {
        let values = [3.0, 1.0, 4.0, 100.0, 2.0];
        assert_eq!(percentile(&values, 50), 3.0);
        assert_eq!(percentile(&values, 95), 100.0);
        assert_eq!(percentile(&[0.5], 95), 0.5);
    }
}
