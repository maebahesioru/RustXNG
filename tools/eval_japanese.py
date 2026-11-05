#!/usr/bin/env python3
"""Evaluate Japanese diagnostic retrieval and broad/fresh discovery on RustXNG.

The evaluator stores the raw server JSON and a separate compact extraction.
Token counts are deliberately left unmeasured because RustXNG does not specify
the tokenizer used by any downstream model.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import unquote, urlsplit

HERE = Path(__file__).resolve().parents[1]
DIAGNOSTICS = HERE / "bench/japanese-search/diagnostics.json"
BROAD_FRESH = HERE / "bench/japanese-search/broad_fresh.json"


def canonical_page(url: str) -> tuple[str, str, str]:
    try:
        parts = urlsplit(url)
        host = parts.hostname
        port = parts.port
    except ValueError:
        return "", "", ""
    if parts.scheme.lower() not in ("http", "https") or not host:
        return "", "", ""
    host = host.lower().removeprefix("www.")
    if port and port not in (80, 443):
        host += f":{port}"
    return host, unquote(parts.path).rstrip("/") or "/", parts.query


def rank_of(urls: list[str], references: list[str]) -> int | None:
    gold = {canonical_page(u) for u in references} - {("", "", "")}
    return next((i for i, url in enumerate(urls, 1) if canonical_page(url) in gold), None)


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[max(0, min(len(ordered) - 1, int((len(ordered) - 1) * fraction + 0.5)))]


def make_query(case: dict, variant: str) -> str:
    error = case["exact_error"]
    if variant == "raw":
        return error
    if variant == "quoted":
        return f'"{error}"'
    if variant == "keywords":
        return " ".join(case["keyword_terms"])
    raise ValueError(f"unknown query variant: {variant}")


def build_search_url(base: str, query: str, language: str, qiita: bool,
                     time_range: str | None, include_raw_content: bool,
                     locale: str | None = None,
                     baseline_engines: list[str] | None = None) -> str:
    params: dict[str, str | int] = {
        "q": query, "format": "json", "max_results": 10, "full": 1,
    }
    if language != "default":
        params["language"] = language
    if locale:
        params["locale"] = locale
    selected_engines = list(baseline_engines or [])
    if qiita and "qiita" not in selected_engines:
        selected_engines.append("qiita")
    if selected_engines:
        params["engines"] = ",".join(selected_engines)
    if time_range:
        params["time_range"] = time_range
    if include_raw_content:
        params["include_raw_content"] = "1"
    return base.rstrip("/") + "/search?" + urllib.parse.urlencode(params)


def search(base: str, query: str, language: str, qiita: bool,
           time_range: str | None, include_raw_content: bool, timeout: float,
           locale: str | None = None,
           baseline_engines: list[str] | None = None) -> dict:
    request_url = build_search_url(base, query, language, qiita, time_range, include_raw_content, locale, baseline_engines)
    request = urllib.request.Request(request_url, headers={"User-Agent": "rustxng-japanese-search-eval/1.0"})
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read()
            status = response.status
        payload = json.loads(body)
        results = payload.get("results", [])
        extracted = []
        for item in results[:10]:
            raw_content = item.get("raw_content")
            snippet = item.get("content")
            extracted.append({
                key: item.get(key) for key in
                ("url", "title", "content", "published_date", "engine", "engines", "score")
            } | {
                "published_date": item.get("published_date") or item.get("publishedDate"),
                "raw_content": raw_content,
                "raw_content_bytes": len(raw_content.encode("utf-8")) if isinstance(raw_content, str) else None,
                "raw_content_characters": len(raw_content) if isinstance(raw_content, str) else None,
                "raw_content_tokens": None,
                "content_bytes": len(snippet.encode("utf-8")) if isinstance(snippet, str) else None,
                "content_characters": len(snippet) if isinstance(snippet, str) else None,
                "content_tokens": None,
            })
        return {
            "http_status": status, "query": payload.get("query"),
            "search_query": payload.get("search_query"),
            "results": extracted,
            "unresponsive_engines": payload.get("unresponsive_engines", []),
            "engines": payload.get("engines", []), "error": payload.get("error"),
            "response_bytes": len(body), "elapsed_ms": round((time.perf_counter() - started) * 1000, 2),
            "raw_json": payload,
        }
    except (urllib.error.URLError, TimeoutError, ValueError, KeyError) as error:
        return {"http_status": None, "results": [], "error": f"{type(error).__name__}: {error}",
                "response_bytes": None, "elapsed_ms": round((time.perf_counter() - started) * 1000, 2)}


def pinpoint_summary(rows: list[dict]) -> dict:
    ranks = [row["rank"] for row in rows]
    total = len(rows)
    times = [row["elapsed_ms"] for row in rows]
    return {
        "observations": total,
        "reference_hit@1": sum(r == 1 for r in ranks),
        "reference_hit@5": sum(r is not None and r <= 5 for r in ranks),
        "reference_hit@10": sum(r is not None and r <= 10 for r in ranks),
        "mrr": round(sum(1 / r for r in ranks if r is not None) / total, 4) if total else 0,
        "failures": sum(r is None for r in ranks),
        "request_errors": sum(bool(row.get("error")) for row in rows),
        "p50_ms": round(statistics.median(times), 1) if times else 0,
        "p95_ms": round(percentile(times, .95), 1),
    }


def broad_metrics(row: dict, window_days: int) -> dict:
    results = row.get("results", [])
    urls = [r.get("url", "") for r in results if r.get("url")]
    domains = [canonical_page(url)[0] for url in urls]
    valid_domains = [d for d in domains if d]
    dated, fresh, buckets = [], [], set()
    missing_dates = invalid_dates = 0
    cutoff = datetime.now(timezone.utc) - timedelta(days=window_days)
    for result in results:
        raw = result.get("published_date")
        if not raw:
            missing_dates += 1
            continue
        try:
            date = datetime.fromisoformat(raw.replace("Z", "+00:00"))
        except (ValueError, AttributeError):
            invalid_dates += 1
            continue
        if date.tzinfo is None:
            date = date.replace(tzinfo=timezone.utc)
        dated.append(raw)
        if cutoff <= date <= datetime.now(timezone.utc):
            fresh.append(raw)
            buckets.add(date.strftime("%Y-%m"))
    normalized = [canonical_page(u) for u in urls]
    return {
        "result_count": len(urls),
        "unique_domains": len(set(valid_domains)),
        "domain_independence_ratio": round(len(set(valid_domains)) / len(valid_domains), 4) if valid_domains else 0,
        "duplicate_pages": len(normalized) - len(set(normalized)),
        "publication_date_count": len(dated),
        "publication_date_missing_count": missing_dates,
        "publication_date_invalid_count": invalid_dates,
        "publication_date_coverage": round(len(dated) / len(results), 4) if results else 0,
        "within_time_window_count": len(fresh),
        "within_time_window_coverage": round(len(fresh) / len(results), 4) if results else 0,
        "covered_publication_months": sorted(buckets),
        "target_source_domains_seen": sorted(set(valid_domains) & set(row.get("target_source_domains", []))),
        "target_source_domain_coverage": len(set(valid_domains) & set(row.get("target_source_domains", []))) / max(1, len(row.get("target_source_domains", []))),
    }


def load_context(path: Path | None) -> dict:
    if path is None:
        return {"operator_supplied_context": None,
                "missing_context": ["cache state", "engine list", "local index state", "region", "server runtime settings"]}
    context = json.loads(path.read_text())
    return {"operator_supplied_context": context,
            "missing_context": [key for key in ("cache_state", "engines", "local_index", "region", "server_settings") if key not in context]}


def server_snapshot(base: str, path: str, timeout: float) -> dict:
    """Save a server diagnostic endpoint verbatim when it is available."""
    try:
        request = urllib.request.Request(base.rstrip("/") + path, headers={"User-Agent": "rustxng-japanese-search-eval/1.0"})
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read()
            return {"status": response.status, "sha256": hashlib.sha256(body).hexdigest(),
                    "body_utf8": body.decode("utf-8", errors="replace")}
    except (urllib.error.URLError, TimeoutError, ValueError) as error:
        return {"error": f"{type(error).__name__}: {error}"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="http://127.0.0.1:18991")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--groups", default="pinpoint,broad_fresh")
    parser.add_argument("--variants", default="raw,quoted,keywords")
    parser.add_argument("--languages", default="default,ja")
    parser.add_argument("--qiita", default="off,on")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--delay", type=float, default=.2)
    parser.add_argument("--timeout", type=float, default=20)
    parser.add_argument("--include-raw-content", action="store_true")
    parser.add_argument("--locale", help="Optional locale parameter, recorded separately from language")
    parser.add_argument("--engines", help="Comma-separated explicit baseline engine list; Qiita is added in the on condition")
    parser.add_argument("--run-context", type=Path, help="JSON describing cache, engines, local index, region and server settings")
    parser.add_argument("--broad-fresh-json", type=Path, default=BROAD_FRESH)
    parser.add_argument("--diagnostics-json", type=Path, default=DIAGNOSTICS, help="Pinned diagnostic corpus, including independently captured compiler cases")
    args = parser.parse_args()
    if args.repeats < 2:
        parser.error("--repeats must be at least 2")
    args.output.mkdir(parents=True, exist_ok=True)
    groups = set(args.groups.split(","))
    variants = args.variants.split(",")
    languages = args.languages.split(",")
    qiita_values = [value == "on" for value in args.qiita.split(",")]
    baseline_engines = [value for value in (args.engines or "").split(",") if value]
    diagnostics = json.loads(args.diagnostics_json.read_text())["cases"]
    broad = json.loads(args.broad_fresh_json.read_text()).get("cases", []) if "broad_fresh" in groups else []
    started = datetime.now(timezone.utc).isoformat()
    settings_path = HERE / "settings.toml"
    metadata = {
        "started_at": started, "base": args.base,
        "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=HERE, text=True).strip(),
        "settings_sha256": hashlib.sha256(settings_path.read_bytes()).hexdigest(),
        "corpus_sha256": hashlib.sha256(args.diagnostics_json.read_bytes()).hexdigest(),
        "server_context": load_context(args.run_context),
        "server_snapshots": {"before_stats": server_snapshot(args.base, "/stats", args.timeout),
                             "before_engine_errors": server_snapshot(args.base, "/stats/errors", args.timeout)},
        "request_defaults": {"format": "json", "max_results": 10, "full": 1, "category": "server default", "engine": "server default", "region": None, "locale": args.locale, "cache_bypass": False, "raw_content_requested": args.include_raw_content},
        "conditions": {"variants": variants, "languages": languages, "qiita_engine": qiita_values,
                       "baseline_engines": baseline_engines or "server default (unspecified)"},
        "time_range": {"pinpoint": None, "broad_fresh": "month"},
        "repeats": args.repeats,
        "source_license": "TypeScript diagnostic localization: Apache-2.0; reference documentation terms remain with TypeScript website",
        "tokenizer": None,
        "token_count_status": "unmeasured; no tokenizer implementation/version is specified",
        "limitations": ["No live benchmark is run by this script's tests.", "The Japanese messages are official localization templates with explicit placeholder instantiations, not captured compiler transcripts.", "Cache, engine and local-index state require an operator-supplied --run-context to be fully attributable."],
    }
    (args.output / "metadata.json").write_text(json.dumps(metadata, ensure_ascii=False, indent=2) + "\n")
    observations = []
    total = len(diagnostics) * args.repeats * len(variants) * len(languages) * len(qiita_values) if "pinpoint" in groups else 0
    total += len(broad) * args.repeats * len(languages) * len(qiita_values) if "broad_fresh" in groups else 0
    with (args.output / "observations.jsonl").open("w") as out:
        index = 0
        work = []
        if "pinpoint" in groups:
            for case in diagnostics:
                for variant in variants:
                    for language in languages:
                        for qiita in qiita_values:
                            for repeat in range(1, args.repeats + 1):
                                work.append(("pinpoint", case, variant, language, qiita, repeat, None))
        if "broad_fresh" in groups:
            for case in broad:
                for language in languages:
                    for qiita in qiita_values:
                        for repeat in range(1, args.repeats + 1):
                            work.append(("broad_fresh", case, "raw", language, qiita, repeat, case.get("time_range", "month")))
        for group, case, variant, language, qiita, repeat, time_range in work:
            index += 1
            query = make_query(case, variant) if group == "pinpoint" else case["query"]
            row = search(args.base, query, language, qiita, time_range, args.include_raw_content, args.timeout, args.locale, baseline_engines)
            row.update({"group": group, "case_id": case["id"], "query_variant": variant,
                        "transformation": "verbatim localized message" if variant == "raw" else ("whole message wrapped in ASCII double quotes" if variant == "quoted" else "curated diagnostic keyword terms joined by one ASCII space") if group == "pinpoint" else "fixed broad/fresh query",
                        "query_sent": query, "language_condition": language, "qiita_condition": qiita,
                        "baseline_engines": baseline_engines or "server default (unspecified)",
                        "time_range": time_range, "repeat": repeat, "reference_urls": case.get("expected_reference_urls", []),
                        "model_extraction": None,
                        "rank": rank_of([r.get("url", "") for r in row.get("results", [])], case.get("expected_reference_urls", [])) if group == "pinpoint" else None})
            if group == "broad_fresh":
                row["target_source_domains"] = case.get("target_source_domains", [])
                row["broad_fresh_metrics"] = broad_metrics(row, case.get("window_days", 30))
            observations.append(row)
            out.write(json.dumps(row, ensure_ascii=False) + "\n")
            out.flush()
            print(f"{index:4}/{total} {group}/{case['id']} {variant} lang={language} qiita={qiita} rep={repeat} {row['elapsed_ms']:.1f}ms", flush=True)
            if args.delay:
                time.sleep(args.delay)
    summaries = {}
    groups_conditions: dict[str, list[dict]] = {}
    for row in observations:
        key = "/".join((row["group"], row["query_variant"], row["language_condition"], "qiita=" + str(row["qiita_condition"])))
        groups_conditions.setdefault(key, []).append(row)
    for key, rows in groups_conditions.items():
        if rows[0]["group"] == "pinpoint":
            summaries[key] = pinpoint_summary(rows)
        else:
            metrics = [r["broad_fresh_metrics"] for r in rows]
            summaries[key] = {"observations": len(rows), "mean_unique_domains": round(statistics.mean(m["unique_domains"] for m in metrics), 3) if metrics else 0,
                              "mean_domain_independence_ratio": round(statistics.mean(m["domain_independence_ratio"] for m in metrics), 4) if metrics else 0,
                              "duplicate_pages": sum(m["duplicate_pages"] for m in metrics),
                              "missing_publication_dates": sum(m["publication_date_missing_count"] for m in metrics),
                              "invalid_publication_dates": sum(m["publication_date_invalid_count"] for m in metrics),
                              "publication_date_coverage": round(sum(m["publication_date_count"] for m in metrics) / max(1, sum(m["result_count"] for m in metrics)), 4),
                              "within_time_window_coverage": round(sum(m["within_time_window_count"] for m in metrics) / max(1, sum(m["result_count"] for m in metrics)), 4),
                              "mean_target_source_domain_coverage": round(statistics.mean(m["target_source_domain_coverage"] for m in metrics), 4) if metrics else 0,
                              "p50_ms": round(statistics.median(r["elapsed_ms"] for r in rows), 1) if rows else 0,
                              "p95_ms": round(percentile([r["elapsed_ms"] for r in rows], .95), 1)}
    report = {"metadata": metadata, "completed_at": datetime.now(timezone.utc).isoformat(), "summaries": summaries}
    report["metadata"]["server_snapshots"]["after_stats"] = server_snapshot(args.base, "/stats", args.timeout)
    report["metadata"]["server_snapshots"]["after_engine_errors"] = server_snapshot(args.base, "/stats/errors", args.timeout)
    (args.output / "metadata.json").write_text(json.dumps(report["metadata"], ensure_ascii=False, indent=2) + "\n")
    (args.output / "summary.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(summaries, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
