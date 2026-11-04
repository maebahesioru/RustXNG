# Contributing to RustXNG

Bug reports, documentation, benchmarks, reviews, and code are welcome. 日本語・Englishどちらでも参加できます。
You do not need repository access to contribute: fork the repository and open a PR.
For sustained maintenance, see [GOVERNANCE.md](GOVERNANCE.md) and [MAINTAINERS.md](MAINTAINERS.md).

## Choose the right place

- Reproducible bug or scoped feature: [Issues](https://github.com/sahenjp/RustXNG/issues).
- Usage questions, exploratory ideas, or sharing an integration: [Discussions](https://github.com/sahenjp/RustXNG/discussions).
- Security vulnerability: [private reporting](https://github.com/sahenjp/RustXNG/security/advisories/new), following [SECURITY.md](SECURITY.md).
- Maintainer participation: the dedicated Issue form. An application does not grant access automatically.

Read existing Issues/PRs before starting. Large API, ranking, networking, and architecture changes should have an agreed
Issue first. Say which task you intend to work on so duplicate implementations can be avoided.
Small documentation fixes can go straight to a PR. Follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Development setup

Use stable Rust (minimum 1.88 for the current lockfile) and a C build toolchain.
PDF tests also require [Poppler and its Japanese CMap data](README.md#pdf-page-text).

```bash
git clone https://github.com/sahenjp/RustXNG.git
cd RustXNG
cargo test --all-targets
```

Use a topic branch, with one coherent purpose per PR. Keep unrelated dependency updates, formatting changes,
generated datasets, and architecture changes out of a small fix.

## Required evidence

| Change | Checks |
| --- | --- |
| Documentation only | Markdown lint and local links; execute command examples when feasible |
| Rust logic | `cargo fmt --all --check`, `cargo test --all-targets`, `cargo clippy --all-targets -- -D warnings` |
| Engine addition/change | Above plus offline fixtures for URL construction, parsing, empty/error cases, paging and supported filters |
| API/configuration | Above plus backwards compatibility and invalid-input tests |
| Performance/ranking | Before/after comparison with the same inputs and environment; quality, errors, p50/p95, output size and cache state |
| Platform/distribution | Target build, binary startup, checksum and install-path verification on the actual architecture |

```bash
npx --yes markdownlint-cli2 "*.md" "docs/*.md"
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
python3 tools/smoke_agent_setup.py --binary target/debug/rustxng
```

Build the server with `cargo build --locked --bin rustxng` before running the setup smoke tool.
It uses local fake responses, not live upstream search. CI additionally checks Linux/macOS/Windows,
`--no-default-features`, Rust 1.88, and the container.
Do not claim a check passed if it was not run. State the command, toolchain and limitation clearly.
An empty GitHub check list is not a green CI result; first-time fork workflows may need owner approval.

JavaScript rendering has optional [Firefox setup requirements](tools/page-renderer/README.md).
After installing them, run its real-browser checks explicitly:

```bash
npm test --prefix tools/page-renderer
cargo test --locked --test rendered_pages -- --include-ignored
```

## Adding an engine

1. Read `src/engine/mod.rs`, a similar existing adapter, and the provider's authoritative API documentation.
2. Add `src/engines/<name>.rs`, `// engine-id: <id>`, the engine factory entry, and its settings entry.
3. Implement `meta`, `build_request`, and `parse` using the shared helpers.
4. Add offline fixtures. Test Unicode queries, missing fields, pagination bounds, response errors and filter precision.
   Do not advertise day/week filtering if the provider only supports years.
5. Document authentication, quotas, categories, optional status and defaults. New traffic must be justified;
   a provider working once is not enough evidence for enabling it in every general search.
6. Avoid logging credentials. Preserve authors, dates and source URLs when the provider supplies them.

Declarative `xpath` / `json_engine` / `mediawiki` engines may use `tools/import_engines.py`.
Support is determined by working behavior, not by the count of files or configured entries.

## Benchmarks and claims

Use the benchmark Issue form for reproductions. Record the dataset revision, query transformations, repeats,
engine settings, cache/local-index state, language, time range, network environment and errors.

- Reference-page hit@k is not answer accuracy. A correct answer may have another valid source.
- Compression saves transfer bytes, not model tokens. Characters, bytes and tokenizer-specific tokens are different units.
- Rust affects implementation and resource behavior; it does not itself make the upstream information more accurate.
- Compare pinpoint troubleshooting and broad/current-information collection as distinct workloads.
- Preserve failed and missing results; do not report only favorable examples. Large raw page dumps do not belong in git.

The [SealQA report](docs/SEALQA.md) is an example of a retrieval-only protocol.
Search terms and upstream quotas still matter in local hosting; parallel agents do not remove upstream rate limits.

## Reviews and AI-assisted contributions

Explain the concrete trigger and before/after behavior, not only a list of files.
AI-assisted work is welcome. The submitter must review the diff, understand it, verify the claims,
and note materially AI-generated areas or checks they could not perform. An AI's approval is not a substitute for responsibility.
Reviewers should distinguish a reproduced bug, a code-based concern, and a hypothesis.

Required checks must pass on the current PR head and base. Resolve conversations and use squash merge.
See [repository settings](.github/repository-settings/README.md) for the approval policy and owner recovery exception.

## Releases

Release tags matching `v*` are owner-controlled. Area maintainers propose release changes through a PR;
they do not receive tag or admin access automatically.

1. Finish the release PR: version and CHANGELOG agree, CI is green, breaking changes and install paths are documented.
2. The owner creates an annotated `vX.Y.Z` tag after reviewing artifacts and release scope.
3. Verify checksums, version output and startup on each advertised target; attach LICENSE and NOTICE.
4. Do not move published tags or overwrite published binary bytes to repair a release. Prefer a new version.

## License and attribution

Contributions are accepted under the project's AGPL-3.0-or-later license.
Preserve source attribution and license notices for upstream-derived code/data and update [NOTICE](NOTICE) when needed.
Do not introduce material whose redistribution terms you cannot establish.
SPDX headers are encouraged for new source files.
