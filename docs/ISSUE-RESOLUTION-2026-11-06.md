# Integration status on 2026-11-06

PR #113 integrates #103, #106, #107, #108, #109 and #110 against v0.29.0.
Its source commit `45788f6` passed 14 CI checks, including all three renderer platforms,
Rust 1.88, the audit gate, containers and ARM64. Local checks passed 1106 Rust tests,
63 Python tests and 16 Firefox/Node renderer tests.

## Retrieval gate

The integration binary was evaluated on the same 100 questions and existing indexes.
Each run started with a fresh process and response cache. Exa exhausted its credit limit
late in the first API run. An authenticated diagnostic request confirmed HTTP 402 and
the provider tag `NO_MORE_CREDITS`.

| Condition | Hit@1 | Hit@5 | MRR@10 | Median ms |
| --- | ---: | ---: | ---: | ---: |
| API, source lost late in run | 69 | 84 | 0.757 | 841.6 |
| Standard | 54 | 68 | 0.606 | 581.5 |
| API, exhausted account | 50 | 64 | 0.566 | 788.0 |

The accepted API outcomes and suspension state are preserved per run. The fresh Japanese
evaluation also encountered the exhausted account. These observations describe mixed
availability and fallback conditions. The earlier API-active 72/100 and 87–88/100 runs
remain in the November 5 report.

Issue #104 remains open for verification with available credits or another reviewed source.
The integration stays in Draft while that requested gate remains outstanding.
The other fixes and their tests are ready for review. [Observations and protocol](../bench/results/issue-resolution-2026-11-06/README.md)

## Scope decisions

Draft #37 was closed after reviewing its outstanding change request. Its current page-fetch
path bypasses the guarded PageReader path, and canonical chains can lose source attribution.
Issue #16 was closed with its remaining default-enablement requirement retired under the
provider policy. Pinned Bing markets and independently configured variants remain supported.
