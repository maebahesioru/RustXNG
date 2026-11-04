// SPDX-License-Identifier: AGPL-3.0-or-later
const net = require('node:net');
const readline = require('node:readline');

async function main() {
  const { firefox } = require('playwright');
  const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  const messages = input[Symbol.asyncIterator]();
  const first = await messages.next();
  if (first.done) throw new Error('renderer input closed');
  const {
    url, html, content_type: contentType = 'text/html', max_bytes: maxBytes,
  } = JSON.parse(first.value);
  if (typeof html !== 'string' || !Number.isInteger(maxBytes) ||
      maxBytes < 1 || maxBytes > 4 * 1024 * 1024 || Buffer.byteLength(html) > maxBytes) {
    throw new Error('invalid renderer input size');
  }
  if (!['text/html', 'application/xhtml+xml'].includes(contentType)) throw new Error('invalid page content type');
  const initialUrl = new URL(url);
  initialUrl.hash = '';
  if (!['http:', 'https:'].includes(initialUrl.protocol)) throw new Error('invalid page URL');

  const pending = new Map();
  let browser;
  let nextId = 0;
  let fail;
  const failure = new Promise((_, reject) => { fail = reject; });
  failure.catch(() => {});
  const stop = () => {
    fail(new Error('renderer input closed'));
    for (const resolve of pending.values()) resolve({ error: 'renderer input closed' });
    pending.clear();
    if (browser) void browser.close().catch(() => {});
  };
  input.once('close', stop);
  process.once('SIGTERM', stop);
  process.once('SIGINT', stop);
  const replies = (async () => {
    for await (const line of messages) {
      const reply = JSON.parse(line);
      const resolve = pending.get(reply.id);
      if (!resolve) throw new Error('unexpected renderer reply');
      pending.delete(reply.id);
      resolve(reply);
    }
  })().catch(fail);

  // Unintercepted traffic must never leave this process.
  const denyProxy = net.createServer(socket => socket.destroy());
  try {
    await new Promise((resolve, reject) => {
      denyProxy.once('error', reject);
      denyProxy.listen(0, '127.0.0.1', resolve);
    });
    browser = await firefox.launch({
      headless: true,
      timeout: 10000,
      proxy: { server: `http://127.0.0.1:${denyProxy.address().port}` },
      firefoxUserPrefs: {
        // Firefox interception requires service workers enabled.
        'dom.serviceWorkers.enabled': true,
        'media.peerconnection.enabled': false,
        'network.webtransport.enabled': false,
        'network.proxy.allow_hijacking_localhost': true,
        'network.proxy.no_proxies_on': '',
        'network.proxy.failover_direct': false,
        'network.proxy.allow_bypass': false,
        'network.dns.disablePrefetch': true,
        'network.prefetch-next': false,
        'network.http.speculative-parallel-limit': 0,
        'network.predictor.enabled': false,
      },
    });
    const context = await browser.newContext({ serviceWorkers: 'block', acceptDownloads: false });
    await context.routeWebSocket('**/*', socket => socket.close());
    let initial = true;
    await context.route('**/*', async route => {
      try {
        const request = route.request();
        if (initial && request.isNavigationRequest() && request.url() === initialUrl.href) {
          initial = false;
          await route.fulfill({ status: 200, contentType: `${contentType}; charset=utf-8`, body: html });
          return;
        }
        if (!['GET', 'HEAD'].includes(request.method())) throw new Error('unsupported resource method');
        if (++nextId > 64) throw new Error('too many page resources');
        const reply = new Promise(resolve => pending.set(nextId, resolve));
        process.stdout.write(JSON.stringify({
          type: 'fetch', id: nextId, url: request.url(), method: request.method(),
        }) + '\n');
        const response = await reply;
        if (response.error) throw new Error(response.error);
        if (response.status >= 300 && response.status < 400) throw new Error('resource redirects are unsupported');
        const body = Buffer.from(response.body, 'base64');
        if (body.length > maxBytes) throw new Error('resource body too large');
        await route.fulfill({ status: response.status, headers: response.headers, body });
      } catch (error) {
        fail(error);
        await route.abort().catch(() => {});
      }
    });
    const page = await context.newPage();
    const rendered = (async () => {
      await page.goto(url, { waitUntil: 'networkidle', timeout: 10000 });
      const output = await page.evaluate(limit => {
        const value = document.documentElement.outerHTML;
        return typeof value === 'string' && value.length <= limit ? value : null;
      }, maxBytes);
      if (output === null || Buffer.byteLength(output) > maxBytes) throw new Error('rendered HTML too large');
      return Buffer.from(output).toString('base64');
    })();
    const body = await Promise.race([rendered, failure]);
    process.stdout.write(JSON.stringify({ type: 'html', body }) + '\n');
  } finally {
    if (browser) await browser.close();
    await new Promise(resolve => denyProxy.close(resolve));
    input.removeListener('close', stop);
    input.close();
    process.stdin.pause();
    await replies;
  }
}

main().catch(error => {
  const message = String(error.message ?? error).slice(0, 1024).toWellFormed();
  process.stdout.write(JSON.stringify({ type: 'error', message }) + '\n');
  process.stdin.pause();
  process.exitCode = 1;
});
