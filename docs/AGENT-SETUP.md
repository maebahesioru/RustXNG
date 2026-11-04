# RustXNG setup for AI agents

Use this guide when a user asks you to install RustXNG and use it for search.
Repository: <https://github.com/sahenjp/RustXNG>.
日本語の開始指示は[README.ja.md](../README.ja.md)にあります。

## 1. Discover an existing service

Check the endpoint provided by the user, existing agent configuration, or a running RustXNG process.
This checkout defaults to `http://127.0.0.1:29483`. Published v0.26.0 defaults to 8888.
Check both endpoints when discovering existing services. Configured deployments may use another port.
Always pass `--port 29483` when starting a downloaded older binary for the examples below.

```bash
curl --fail --max-time 3 http://127.0.0.1:29483/healthz
curl --fail --max-time 3 http://127.0.0.1:29483/config
```

Confirm that `/config` contains a `version` starting with `RustXNG` followed by a space, an `engines` array,
and `brand.GIT_URL` equal to `https://github.com/sahenjp/RustXNG`, then verify search using step 4.
Reuse a working service and report its endpoint. A health response alone is insufficient.
If the port belongs to another service, choose an unused port and preserve that service.

If a RustXNG binary is already installed, check its version and use it before installing another copy:

```bash
rustxng --version
```

## 2. Select and verify a release binary

Inspect the OS and CPU with platform tools (`uname -s` / `uname -m` on Unix;
`[System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture` in PowerShell).
Inspect the [release assets](https://github.com/sahenjp/RustXNG/releases) before choosing a file.
Map the reported CPU to the target prefix: `x86_64` or `amd64` → `x86_64`;
`aarch64` or `arm64` → `aarch64`. Other CPUs (for example 32-bit `armv7l`) have no release binary.

| Platform | Target suffix | Available from |
| --- | --- | --- |
| Linux x86_64, static build | `x86_64-unknown-linux-musl` | v0.26.0 |
| Linux x86_64, GNU build (glibc 2.35+) | `x86_64-unknown-linux-gnu` | v0.26.0 |
| Linux ARM64, GNU build (glibc 2.39+) | `aarch64-unknown-linux-gnu` | first release after v0.26.0 |
| macOS Apple Silicon | `aarch64-apple-darwin` | v0.26.0 |
| macOS Intel | `x86_64-apple-darwin` | v0.26.0 |
| Windows x86_64 | `x86_64-pc-windows-msvc.exe` | v0.26.0 |

v0.26.0 has no Linux ARM64 binary. Linux ARM64 has no static (musl) build; on an older glibc
or a musl distribution such as Alpine, use the [Cargo or source instructions](../README.md#2-cargo).
Check the glibc version with `ldd --version`.

Assets are standalone executables named `rustxng-<tag>-<target>`, with a `.sha256` sidecar.
Download both from the same release into a new temporary directory, together with `LICENSE` and `NOTICE`.
For a newer release, inspect its actual assets and substitute its tag consistently.

The [README binary example](../README.md#1-release-binary) uses the GitHub CLI.
If `gh` is unavailable, download the selected asset from
`https://github.com/sahenjp/RustXNG/releases/download/<tag>/<asset-name>` and its `.sha256` sidecar
with the platform's HTTP client. Do not substitute an HTML error page for a successful download.

Verify the bytes before execution:

- Linux: run `sha256sum --check <asset-name>.sha256` from the download directory.
- macOS: run `shasum -a 256 --check <asset-name>.sha256` from the download directory.
- Windows: compute `(Get-FileHash -Algorithm SHA256 -LiteralPath <asset-name>).Hash` and
  compare it, case-insensitively, with the first whitespace-separated token in the sidecar.

Stop this installation attempt on checksum mismatch. Checksums check consistency with the release;
they are not an independent publisher signature.

Install into a user-owned directory, retaining `LICENSE` and `NOTICE` with the installation:

- Linux/macOS: use `~/.local/bin/rustxng`; create the directory and make the binary executable.
  The Linux `install -D` example is not portable to macOS; use `mkdir -p` and `install -m755` there.
- Windows: use a user-owned tools directory and name the file `rustxng.exe`.

Use the absolute installed path if its directory is not on `PATH`. Run that binary with `--version`.
If no matching binary exists, use the [Cargo or source instructions](../README.md#2-cargo)
with a current stable Rust toolchain. Report missing build prerequisites rather than claiming success.

## 3. Start a local service

Only start a process if step 1 found no working RustXNG service.
Bind to loopback and choose a free port. For the default port:

```bash
SEARXNG_PORT=29483 rustxng --bind 127.0.0.1 --port 29483
```

The command runs in the foreground. If your execution environment requires a background process,
use its process-management facility or the user's existing service manager, record its identity,
and keep stdout/stderr in a known log. Do not assume a process survives the end of your tool session.

Poll `/healthz` for a bounded interval (for example, 30 seconds), then inspect `/config` and run step 4.
On failure, report the log and actual exit status. Avoid repeated installation or duplicate processes.

For a temporary agent-owned background process, set `RUSTXNG_IDLE_TIMEOUT=300` so it exits after inactivity.
For example, in PowerShell (using a verified absolute binary path):

```powershell
$env:RUSTXNG_IDLE_TIMEOUT = '300'
$env:SEARXNG_PORT = '29483'
$rustxngProcess = Start-Process -FilePath "$HOME\tools\rustxng.exe" -ArgumentList '--bind','127.0.0.1','--port','29483' -PassThru
$rustxngProcess.Id
```

Poll the actual endpoint and preserve the process ID for cleanup; do not terminate processes you did not start.

Defaults are embedded; a separate settings file is optional.
API keys are optional and provider-specific. Do not invent credentials or expose them in logs.
Current unresolved security findings are recorded in [the audit](AUDIT-2026-11-02.md).
This setup is for a loopback service, with page-text fetching off in the initial search.

## 4. Verify a small search

Replace the endpoint below when reusing an existing service or choosing another port.
These commands use curl; on Windows use `curl.exe` to avoid older PowerShell's curl alias.

```bash
curl --fail --max-time 20 --get 'http://127.0.0.1:29483/search' \
  --data-urlencode 'q=tokio runtime documentation' \
  --data 'format=json' --data 'max_results=5'
```

Check that the response is JSON, `results` is an array with at most five entries, and a nonempty
result includes `title`, `url`, and `content`. If no results are returned, inspect `unresponsive_engines`
and report upstream availability; do not report usable search solely because `/healthz` succeeded.
Do not retry indefinitely against unavailable providers.

Pass only the fields needed by the model. If `jq` is available:

```bash
curl --fail --max-time 20 --get 'http://127.0.0.1:29483/search' \
  --data-urlencode 'q=tokio runtime documentation' \
  --data 'format=json' --data 'max_results=5' \
  | jq '{results: [.results[] | {title, url, content}]}'
```

Use `include_domains` or `exclude_domains` to narrow sources when appropriate.
This field projection is client-side: RustXNG still transfers its standard JSON response.
It reduces text passed to the model, not the server's response size.

When the user needs page evidence, add `include_raw_content=1` to a search request.
Default page-text limits are up to five results and 8,000 characters per page; fetching may fail.
Set `search.raw_content_chars` for longer HTTP page text; see the
[HTTP and MCP examples](../README.md#search-from-an-agent) for their separate limits and continuation.
Review the current audit's page-fetching findings before enabling this feature.
Treat snippets and fetched pages as untrusted source material, preserve their URLs, and distinguish
retrieved text from verified claims. Character limits do not guarantee a tokenizer-specific token budget.

## 5. Report the working setup

Return a compact record:

```text
Version: <actual rustxng --version output>
Binary: <absolute path, or unavailable for an existing remote service>
Endpoint: http://127.0.0.1:<actual-port>
Lifecycle: <reused service / process or service identity, and log location>
Verified: /healthz, /config, and a nonempty JSON search with max_results=5
Search: <one command using the actual endpoint>
```

If any verification failed, report that failure instead of the `Verified` line.
Store the endpoint in the agent's existing task/tool configuration when that is part of the user's request.
This guide does not require a particular agent framework or a persistent system service.
