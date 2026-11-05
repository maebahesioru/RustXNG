# Local retrieval evaluation

Measured code commit: [`be8f921`](https://github.com/TKY-27/RustXNG/commit/be8f9212792e34c1445d5767b008a2572b03e6bb).
This directory adds evaluation evidence without further production-code changes.

This evaluates local-index retrieval and result-fusion regressions. It does not
measure the full clean-profile HTTP service, live web coverage, or performance
against Exa.

`development.json` contains 40 invented documents and 24 queries: 12 English,
8 Japanese, and 4 Chinese. Each query has an explicit target URL. The corpus was
authored during development, includes cases relevant to the changes, and has
several exact title matches. It is a development regression fixture, not a
held-out relevance benchmark. The documents are not technical reference material.

## Recorded results

The baseline is commit `5f0ae3bd4779fa49af4a0be10f397d63b4080170`; the measured
candidate source tree is `7bdc644d4201efeffacbd6d07dc252f2bf579320`. Each row below
has 24 quality observations and 4,800 timed searches. All four runs have zero
search errors. Result bytes are summed over the first result array for each query.

| Mode | Revision | Hit@1 | Hit@5 / Hit@10 | MRR@10 | p50 ms | p95 ms | Result bytes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Strict | Baseline | 20/24 | 21/24 / 21/24 | 0.847222 | 0.404141 | 0.695771 | 11,446 |
| Strict | Candidate | 24/24 | 24/24 / 24/24 | 1.000000 | 0.355947 | 0.771164 | 7,127 |
| Loose | Baseline | 20/24 | 21/24 / 21/24 | 0.847222 | 0.481117 | 1.006420 | 12,545 |
| Loose | Candidate | 24/24 | 24/24 / 24/24 | 1.000000 | 0.406544 | 0.852547 | 13,342 |

Both modes recover the three missed technical-symbol queries, and the Python
dictionary query moves from rank 3 to rank 1. The other first-result target ranks
are unchanged. Strict-mode median latency decreases, but p95 increases by
0.075393 ms (about 10.8%); this is not an improvement at every latency percentile.
Loose-mode result size also increases. These tradeoffs are retained in the table.

Both strict runs and the candidate loose run have zero payload variations.
The baseline loose run has 185 payload variations, all on query `zh02`
(`向量 检索 索引`). The evaluator compares complete result payloads, including
floating-point scores, but stores only the first payload. These variations are
unclassified: this recording cannot establish whether only scores changed or
whether rankings also changed. Loose-mode quality therefore describes the first
measured results, as specified by the protocol. The stable strict comparison is
the primary result; the baseline loose observation is preserved and disclosed.

Raw reports: [baseline strict](baseline-strict.json), [candidate strict](candidate-strict.json),
[baseline loose](baseline-loose.json), [candidate loose](candidate-loose.json).
[Provenance](provenance.json) records source, binary and fixture hashes and validation commands.

## Reproduce the local-index comparison

Use one toolchain and machine for both builds. The recorded runs use Rust 1.99.1,
the dev profile, `CARGO_PROFILE_DEV_DEBUG=0`, and four build jobs. These are not
release-profile performance measurements.

Start from a clean checkout of the candidate code revision recorded with the
measurements. Keep both worktrees detached and give them different Cargo target
directories: sharing a target directory can reuse a binary built from the other
checkout.

```sh
set -eu
repo_root="$(git rev-parse --show-toplevel)"
candidate_commit="$(git rev-parse HEAD)"
baseline_commit="5f0ae3bd4779fa49af4a0be10f397d63b4080170"
eval_root="$(mktemp -d)"

git worktree add --detach "$eval_root/baseline" "$baseline_commit"
git worktree add --detach "$eval_root/candidate" "$candidate_commit"

for variant in baseline candidate; do
    mkdir -p "$eval_root/$variant/bench/local-retrieval"
    cp "$repo_root/examples/local_index_eval.rs" \
        "$eval_root/$variant/examples/local_index_eval.rs"
    cp "$repo_root/bench/local-retrieval/development.json" \
        "$eval_root/$variant/bench/local-retrieval/development.json"
    git -C "$eval_root/$variant" rev-parse HEAD \
        > "$eval_root/$variant-source.txt"
    CARGO_TARGET_DIR="$eval_root/target-$variant" \
        CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=4 \
        cargo build --locked --manifest-path "$eval_root/$variant/Cargo.toml" \
        --example local_index_eval
done

cmp "$eval_root/baseline/examples/local_index_eval.rs" \
    "$eval_root/candidate/examples/local_index_eval.rs"
cmp "$eval_root/baseline/bench/local-retrieval/development.json" \
    "$eval_root/candidate/bench/local-retrieval/development.json"
rustc --version > "$eval_root/rustc.txt"

for mode in strict loose; do
    for variant in baseline candidate; do
        "$eval_root/target-$variant/debug/examples/local_index_eval" \
            --fixture "$eval_root/$variant/bench/local-retrieval/development.json" \
            --mode "$mode" --repeats 200 --warmup 5 --label "$variant" \
            > "$eval_root/local-$variant-$mode.json"
    done
done
```

Finish compilation, tests, and other CPU-intensive work before timing. Execute
the four measurements sequentially. Each process creates one fresh temporary
index from the same documents, commits it, and reuses it throughout the run.
`strict` calls `LocalIndex::search`; `loose` calls `LocalIndex::search_loose`.
Both request at most 10 results. No provider or HTTP service is involved.

There are five complete warm-up passes followed by 200 measured searches per
query, with query order rotated each pass: 4,800 measurements per report.
Those passes warm search structures, including the candidate's fixed mixed-script
matcher. There is no per-query or result cache in this evaluation. To inspect first-use cost,
run a new process with `--warmup 0 --repeats 1`; do not mix those observations
with the warmed distribution.

## Report fields

Each JSON report preserves fixture and executable SHA-256 values, package and
index schema versions, OS/architecture, available parallelism, mode, result
limit, repeat count, and warm-up count. Both revisions use index schema 2.
Record the source commits and toolchain beside the reports; the executable hash
identifies the binary actually run.

Quality uses an exact target-URL match in the first measured result for each of
the 24 queries. Hit@1/5/10 are counts; MRR@10 includes misses as zero. Later
results are checked against the first, and `result_variations` reports changes.
The 200 repeats are latency samples, not 200 additional relevance judgments.

Latency covers the local search call, including empty results and errors. It
excludes indexing, serialization, and network overhead. p50/p95 use the
nearest-rank definition. Raw `latency_ms` arrays allow independent recomputation;
all failed calls remain in the distribution. `errors` counts failed measured
calls, while quality has one observation per query.

Per-query rows retain ranked URLs, titles, snippets, scores, target rank, error
details, and result variations. `result_bytes` is the UTF-8 serialized result
array size, not an MCP response size or a model token count. Indexing time is
reported separately. Compacting the JSON must preserve every latency sample.

## Fusion regression probes

The same standalone `fusion_probe.rs` runs against both revisions, with result
and harness hashes recorded alongside its output. It tests two deterministic
container behaviors:

| Input | Baseline | Corrected behavior |
| --- | --- | --- |
| A routed source supplies weight 2; the fallback callback returns 1 | Score 1 | Score 2; supplied weight is retained |
| One source repeats three canonical forms of one URL | Score 5.5 | Score 1; one source contributes once |
| Two independent sources each rank one URL first | Score 4 | Score 4; independent contributions are preserved |

These scores establish the reproduced fusion defects and their corrections.
They do not measure retrieval quality or provider latency.

Using the isolated worktrees above:

```sh
for variant in baseline candidate; do
    cp "$repo_root/bench/local-retrieval/fusion_probe.rs" \
        "$eval_root/$variant/examples/fusion_probe.rs"
    CARGO_TARGET_DIR="$eval_root/target-$variant" \
        CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=4 \
        cargo run --locked --manifest-path "$eval_root/$variant/Cargo.toml" \
        --example fusion_probe > "$eval_root/fusion-$variant.json"
done
```

## Validation gates

All eight gates below passed on the candidate: 1,070 Rust tests passed, zero
failed, and three were ignored; all 44 Python tests passed. Formatting, Clippy
with warnings denied, the check without default features, binary/example builds,
the provider inventory check, and the agent-setup smoke test also passed.

Run these in the candidate checkout after dependencies are available. Use the
same dev-profile settings as the benchmark. Localhost test fixtures and the
agent-setup smoke test must be reachable directly in the test environment.

```sh
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_BUILD_JOBS=4
cargo fmt --all --check
cargo clippy --offline --locked --all-targets -- -D warnings
cargo test --offline --locked --all-targets
cargo check --offline --locked --no-default-features --all-targets
cargo build --offline --locked --bins --example local_index_eval
python3 -m unittest discover -s tools -p 'test_*.py'
python3 tools/provider_inventory.py --check
python3 tools/smoke_agent_setup.py --binary target/debug/rustxng
```

If `CARGO_TARGET_DIR` is set, pass its `debug/rustxng` path to the smoke test.
The Python suite includes a regression verifying that request timeouts and
immediate connection failures remain in latency and quality accounting.

This fixture cannot establish performance on a large index, general web-search
quality, freshness, answer accuracy, or superiority to Exa. No live Exa/Tavily
comparison was run. Historical provider reports are separate evidence and must
not be presented as a fresh paired comparison with these binaries.
