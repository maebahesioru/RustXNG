# Japanese search evaluation protocol

This fixture set separates pinpoint retrieval of Japanese diagnostic messages from broad, time-filtered discovery. It does not reuse the 100-question or SealQA scores as a proxy for either task.

## Diagnostic cases

`diagnostics.json` contains 30 public TypeScript 5.9.3 Japanese localization entries from the official `lib/ja/diagnosticMessages.generated.json` at tag `v5.9.3`. The corpus stores each original localized template, its numeric diagnostic code, an explicit placeholder instantiation, the official source URL and SHA-256, Apache-2.0 licensing, and expected TypeScript handbook/TSConfig pages. Since placeholder values are supplied to make realistic query instances, each row is labeled as a template instantiation rather than a captured compiler transcript. The public TypeScript source is available at [the pinned Japanese diagnostic catalog](https://github.com/microsoft/TypeScript/blob/v5.9.3/lib/ja/diagnosticMessages.generated.json); the TypeScript repository is Apache-2.0 licensed. The references point to official TypeScript handbook/TSConfig pages.

`broad_fresh.json` is a separate set of six open-ended Japanese discovery questions. They use the server's `month` time range, with expected source domains treated as coverage targets rather than exhaustive answers.

## Run

Start the RustXNG server using the configuration you want to evaluate. Supply an operator context JSON to record information not exposed by the query API, such as cache state, enabled engines, local index state, region, and runtime settings. For example:

```json
{
  "cache_state": "fresh process; cache enabled; no explicit flush",
  "engines": ["list the enabled engines here"],
  "local_index": "disabled",
  "region": "Japan",
  "server_settings": "settings file/config revision and relevant overrides"
}
```

Inspect the request plan before running a provider-backed evaluation. This command
does not contact the server or create an output directory:

```sh
python3 tools/eval_japanese.py --dry-run --engines wikipedia --qiita off
```

The explicit engine list is a comparison condition, not evidence that the
operator's intended use is permitted. Review the selected providers' conditions,
quotas, and response-retention rights first. For Qiita, the [API documentation](https://qiita.com/api/v2/docs)
states 60 requests/hour per IP without authentication and 1,000/hour with
authentication. Its [application guidance](https://help.qiita.com/ja/articles/qiita-api)
permits applications within the API's features, prohibits advertising
monetization of those applications, and does not permit scraping. The plan counts
RustXNG search calls selecting Qiita, not actual upstream requests; shared IP
traffic, retries, routing and caches also affect quota use.

After choosing an appropriate configuration, run from the repository root:

```sh
python3 tools/eval_japanese.py \
  --base http://127.0.0.1:18991 \
  --output /tmp/rustxng-japanese-run \
  --run-context /path/to/run-context.json \
  --engines wikipedia --qiita off \
  --repeats 2
```

The default matrix independently crosses exact, quoted, and curated-keyword pinpoint queries with the server-default and `ja` language conditions, plus Qiita-engine-off/on conditions. The broad/fresh questions run as a separate query group with a one-month time range. With an explicit baseline, that full matrix is 768 serial requests (720 pinpoint and 48 broad/fresh), including 384 calls selecting Qiita. This exceeds Qiita's hourly anonymous allowance if issued in one hour. Use `--groups`, `--variants`, `--languages`, and `--qiita` to reduce or isolate conditions.

The `on` condition requires an explicit `--engines` baseline and appends `qiita`
to that same list. The baseline must exclude Qiita, so the `off` condition cannot
silently include it. Invalid or duplicate matrix values fail before any server
request. Historical runs made without an explicit baseline retain their original
meaning; this validation does not turn them into additive comparisons. `--locale`
adds a separately recorded locale parameter. The script requires at least two
repetitions.

Use a new output directory for each run. The evaluator refuses to overwrite
existing metadata, observations, or summaries so historical evidence is retained.

`--diagnostics-json` and `--broad-fresh-json` accept independently frozen corpora.
Both selected files are hashed in the plan and run metadata, and diagnostic case
origins and dataset licenses are recorded from the fixture. Supplying a different
file does not itself establish that its cases are held out or independently
captured. The default 30 cases remain template instances.

The output directory contains:

- `metadata.json`: hashes of both selected corpora, the request plan, repository settings hash and commit, parameters, supplied run context, and before-run server `/stats` and `/stats/errors` snapshots. The repository settings hash does not identify the running server's configuration; supply that through `--run-context`.
- `observations.jsonl`: one complete raw JSON payload and one compact extraction per request, including the actual query transformation, language/Qiita/time conditions, exact reference rank, returned publication dates, and snippet/raw-content byte and Unicode-character counts.
- `summary.json`: hit@1, hit@5, MRR, failures, p50/p95 latency, and broad/fresh source diversity, duplicate-page, publication-date, and time-window coverage measures.

Token counts are always `null`: no tokenizer or version is specified for the downstream model. Raw-content retrieval is disabled unless `--include-raw-content` is set. `--run-context` records cache/index/runtime facts that the server API does not expose; `missing_context` lists any omitted fields. Server snapshots are saved before and after the run. Search cache bypass is not claimed, so repeat runs are intentionally observable as configured-cache behavior.

Pinpoint scoring canonicalizes host case, `www`, scheme, fragments, percent-encoded paths, and trailing slashes; path case and query strings remain significant. A reference is a hit only when the same page appears, not when a URL merely shares a prefix. Broad/fresh coverage counts distinct domains, duplicate canonical pages, valid publication dates, results within the specified 30-day window, and the share of target source domains observed. The benchmark measures retrieval behavior for this fixed set only; it does not establish general commercial-search superiority.

## Offline tests

```sh
python3 -m unittest discover -s tools -p 'test_eval_japanese.py'
```

Tests use only local fixtures and do not contact the search server.
