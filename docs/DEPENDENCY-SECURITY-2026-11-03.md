# Dependency security follow-up — 2026-11-03

The locked MySQL client is updated from 26.0.1 to 28.0.4, using the upstream
28.0.3-or-later dependency requirement. This removes `lru 0.12.5`; the remaining
version is 0.18.5, past both RUSTSEC-2026-0002 and RUSTSEC-2026-0253.
The upstream dependency requirement is recorded in
[mysql's Cargo.toml](https://github.com/blackbeam/rust-mysql-simple/blob/v28.0.3/Cargo.toml).

The MongoDB dependency chain's `serde_with` is updated from 3.17.0 to 3.21.0,
the patched version for
[GHSA-7gcf-g7xr-8hxj](https://github.com/jonasbb/serde_with/security/advisories/GHSA-7gcf-g7xr-8hxj).
This is a dependency remediation, not a finding that RustXNG invokes the affected
`KeyValueMap` serializer with attacker-controlled empty entries.

The [new RustSec snapshot](audit/2026-11-03/dependencies-mysql28.json) has no
vulnerabilities or unsoundness warnings. At that snapshot, three maintenance
warnings remained: `basic-toml`, `fxhash` and `unsafe-libyaml`. Issue #79 removes
them by disabling Askama's unused config feature, upgrading scraper, and moving
YAML parsing to the maintained `yaml_serde` compatibility release. Earlier
audit files are historical snapshots.

Validation includes Rust 1.88, default/slim builds, and an isolated MariaDB fixture
that runs both MySQL and MariaDB engine implementations through real authentication,
prepared parameter binding, pagination and an injection-shaped query value.
CI runs the database fixture in a disposable, digest-pinned MariaDB service;
ordinary offline test runs skip this explicitly marked test.

To run the database test locally, create an isolated database named `rustxng_test`
with fixture user `rustxng` and password `fixture-password` on loopback, then run:

```bash
MYSQL_TEST_PORT=3306 cargo test --locked --test mysql -- --ignored
```

The test creates and drops a table. Use a disposable database, not an existing application database.
