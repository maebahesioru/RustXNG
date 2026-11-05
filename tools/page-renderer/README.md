# Optional JavaScript page renderer

This helper lets HTTP `include_raw_content=1` and MCP `read_page` extract text
after page scripts run. It is disabled by default. RustXNG continues to fetch
pages directly unless `search.page_renderer_command` is configured.

## Install

Use Node.js 20 or later and the Firefox build matched to the pinned Playwright
package. From the repository root:

```bash
cd tools/page-renderer
npm ci
npm run install-browser
```

Linux installations may also need `npx playwright install-deps firefox`.
Run the helper as an unprivileged user with Firefox's sandbox enabled.
The standard RustXNG container does not include Node.js or Firefox; install the
helper and browser in your deployment before enabling this setting.

Use absolute paths when RustXNG runs from another directory:

```toml
[search]
page_renderer_command = ["node", "/opt/RustXNG/tools/page-renderer/render.cjs"]
raw_content_chars = 20000
```

The command is an executable and its arguments, not a shell expression. This
setting enables rendering for HTML page reads in both HTTP and MCP. MCP retains
its own `max_chars`, payload budgets, and pagination.

## Supported pages and limits

The helper supports scripts, GET/HEAD requests, and ordinary Web Workers. It
waits for worker startup and all intercepted resources to finish, followed by
500 ms without further resource activity, before extracting the DOM. A resource
that starts during extraction causes the snapshot to be retried. This wait and
extraction share a 10-second deadline. Workers need not terminate or send a
message. Content scheduled after the quiet period may not be included. Rendering is
limited to 15 seconds including browser startup, after the initial page
download. Process and profile cleanup can take up to 8 additional seconds after
cancellation or the deadline. A fresh browser is closed after each page, and no
more than four renderers run concurrently.

RustXNG fetches each intercepted resource through its public-address resolver
and bounded HTTP client. Resources are limited to 64 requests, 4 MiB per
resource, and 16 MiB in total. Initial and rendered HTML are limited to 4 MiB.
Transfer and output limits do not bound the browser's JavaScript heap or total
memory use.

An owned local proxy closes every connection that bypasses interception.
Firefox is configured to proxy localhost as well, disable direct proxy
failover, and disable WebRTC, WebTransport, DNS prefetching, and speculative
connections. Service-worker registration and WebSockets are blocked; traffic
not covered by those API hooks still encounters the rejecting proxy.

POST requests and redirected subresources fail the render. The initial page's
HTTP redirects remain supported. Login sessions, interactive authentication,
WebSockets, and service-worker applications are unsupported. An explicit
`accept_language` page preference is applied by RustXNG to all guarded GET/HEAD
resources, including other origins. Browser cookies and arbitrary request headers
are not forwarded; `navigator.language` is unchanged. Unsupported methods or
resources rejected by RustXNG cause a retrieval error.

## Verify

```bash
npm test
```

The tests launch real Firefox against synthetic responses and owned loopback
canaries. They cover external scripts, JSON fetches, workers, Japanese content
beyond 8,000 characters, rejected requests, UTF-8 output bounds, and shutdown.
No external page is requested by the tests.

The helper communicates with RustXNG using JSON lines on standard input/output.
Its first input is `{url, html, content_type, max_bytes}`. `content_type` is
`text/html` (the default) or `application/xhtml+xml`, preserving the page's parser.
It sends resource requests as
`{type: "fetch", id, url, method}` and receives `{id, status, headers, body}`,
where `body` is base64, or `{id, error}`. Its result is
`{type: "html", body}` with base64 HTML, or `{type: "error", message}`.
