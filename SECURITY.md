# Security

## Private reporting

Please report vulnerabilities through [GitHub private vulnerability reporting](https://github.com/sahenjp/RustXNG/security/advisories/new),
not a public Issue or Discussion. This feature is enabled for the repository.
Include the affected version, configuration, minimal reproduction and expected impact, without real credentials or private data.
The owner coordinates investigation and disclosure; no response-time guarantee is currently offered.

## Current limitations

RustXNG retrieves untrusted upstream information. Local hosting and a successful health check do not establish safety.
The [2026-11-02 audit](docs/AUDIT-2026-11-02.md) records findings against commit fef998b, including DNS/IPv6/redirect
SSRF gaps, unsafe result URL schemes, response-size limits, cache identity and research API resource limits.
Read that report and the current fixes before exposing the service or enabling untrusted page fetching.

Bind local-agent services to loopback; publish container ports with an explicit host IP.
The engine timeout and suspension mechanisms are availability controls, not complete SSRF or memory-exhaustion defenses.
An authenticated proxy is not automatically a safe resolver for untrusted destinations.

## Untrusted destination checks

Page reads (MCP `read_page`, search raw content and research), images and favicons
use a direct-only client with URL checks on every redirect. DNS is resolved once
per connection; an empty answer or any non-global address rejects the entire answer.
The connector uses those checked addresses. Private configured engine endpoints
do not grant exceptions to page or media fetching. IPv4, IPv6, mapped addresses,
NAT64 and 6to4 destinations are checked. Proxies, including environment proxies,
disable these reads because remote DNS cannot be validated by this client.
Administrators should also enforce network egress restrictions for their deployment;
application checks cannot control local routing or custom address translation.

`server.secret_key` defaults to empty. At startup RustXNG replaces an empty key
or the historical `ultrasecretkey` example with 32 cryptographically random bytes.
The generated key lasts for that Searcher instance; signed media links expire on
restart. For multiple workers or stable links, set your own shared random secret
through the configuration or `SEARXNG_SECRET`. Signatures do not replace destination checks.

Offline regression tests exercise image, favicon, raw-content and research routes
against owned local servers and injected DNS answers. They verify no TCP connections
reach the private destination, including after redirects; no external DNS service or LLM is used.

## Dependency checks

```bash
cargo install cargo-audit --locked
cargo audit
```

The recorded audit found `ctutils 0.4.2` / RUSTSEC-2026-0385 and unsoundness advisories for `lru 0.12.5`,
plus maintenance warnings. The default `db-engines` feature enables the PostgreSQL and MySQL dependency chains;
it is incorrect to claim these dependencies are absent from the default build.
See the [recorded advisory output](docs/audit/2026-11-02/dependencies.json).
This is a dated snapshot, not a claim about the latest advisory database or exploitability of every dependency path.

The [2026-11-03 audit comparison](docs/audit/2026-11-03/dependencies-summary.json) records an update to
ctutils 0.4.4 for RUSTSEC-2026-0385. The vulnerability section is then empty, but two `lru 0.12.5`
unsoundness advisories through MySQL and three maintenance warnings remain. They are tracked in
[issue #64](https://github.com/sahenjp/RustXNG/issues/64); this is not an advisory-free dependency tree.

The [subsequent dependency update](docs/DEPENDENCY-SECURITY-2026-11-03.md) removes
the affected `lru` version and updates `serde_with` for GHSA-7gcf-g7xr-8hxj.
Its recorded RustSec scan has no vulnerabilities or unsoundness findings; three
maintenance warnings remain under [issue #79](https://github.com/sahenjp/RustXNG/issues/79).

Updates require review of actual usage, a locked dependency check, tests and platform/MSRV verification.
Do not silently suppress a security advisory or present a clean normal CI run as a security audit.

## Maintainer and workflow access

Workflow tokens are read-only by default and may not approve PRs. Release jobs request explicit permissions where needed.
Version tags are owner-controlled. New collaborators must follow the review and access procedure in [MAINTAINERS.md](MAINTAINERS.md).
No application, PR, social-media reply, or AI recommendation automatically grants write or administrative access.
## Continuous advisory checks

The CI and daily security workflow record the complete RustSec report. Vulnerabilities always fail.
Known maintenance findings are listed in `.github/security-baseline.json`, tied to issue #79,
and remain visible in logs and artifacts. New findings, changed affected versions, invalid reports and an expired
baseline fail the check. The current baseline expires on 2026-12-03; it is not a statement that the dependencies are safe.
