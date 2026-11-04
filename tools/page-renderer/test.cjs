// SPDX-License-Identifier: AGPL-3.0-or-later
const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const http = require('node:http');
const path = require('node:path');
const readline = require('node:readline');
const test = require('node:test');

function run(html, fetch, options = {}) {
  const child = spawn(process.execPath, [path.join(__dirname, 'render.cjs')], {
    stdio: ['pipe', 'pipe', 'pipe'],
    env: { ...process.env, ...options.env },
  });
  const requests = [];
  let result;
  let diagnostics = '';
  const lines = readline.createInterface({ input: child.stdout });
  child.stderr.on('data', value => { diagnostics += value; });
  const done = new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error('renderer timed out: ' + diagnostics));
    }, 15000);
    child.on('error', reject);
    lines.on('line', line => {
      try {
        const message = JSON.parse(line);
        if (message.type === 'fetch') {
          requests.push(message);
          const reply = fetch(message);
          child.stdin.write(JSON.stringify({ id: message.id, ...reply }) + '\n');
        } else result = message;
      } catch (error) {
        child.kill('SIGKILL');
        reject(error);
      }
    });
    child.on('close', code => {
      clearTimeout(timer);
      if (!result) reject(new Error(`renderer exited ${code}: ${diagnostics}`));
      else resolve({ ...result, code, requests });
    });
  });
  child.stdin.on('error', () => {});
  child.stdin.write(JSON.stringify({
    url: options.url || 'https://fixture.invalid/article',
    html,
    max_bytes: options.maxBytes || 4 * 1024 * 1024,
  }) + '\n');
  return { child, done };
}

function response(body, contentType = 'application/javascript') {
  return { status: 200, headers: { 'content-type': contentType }, body: Buffer.from(body).toString('base64') };
}

test('renders external scripts, fetches, and worker content beyond 8000 characters', async () => {
  const text = '日本語の動的本文'.repeat(1500);
  const resources = {
    '/app.js': response(`
      fetch('/article.json').then(r => r.json()).then(v => document.querySelector('main').textContent = v.text);
      const worker = new Worker('/worker.js');
      worker.onmessage = e => document.querySelector('aside').textContent = e.data;
    `),
    '/article.json': response(JSON.stringify({ text }), 'application/json'),
    '/worker.js': response(`fetch('/worker.json').then(r => r.json()).then(v => postMessage(v.text));`),
    '/worker.json': response(JSON.stringify({ text: 'Worker text' }), 'application/json'),
  };
  const { done } = run('<main></main><aside></aside><script src="/app.js"></script>', request => {
    assert.equal(request.method, 'GET');
    return resources[new URL(request.url).pathname] || { error: 'unexpected resource' };
  });
  const result = await done;
  assert.equal(result.type, 'html');
  assert.equal(result.code, 0);
  const rendered = Buffer.from(result.body, 'base64').toString();
  assert.ok(rendered.includes(`<main>${text}</main>`));
  assert.ok(rendered.includes('<aside>Worker text</aside>'));
  assert.equal(result.requests.length, 4);
});

test('broker failures and redirected resources fail the whole render', async () => {
  for (const reply of [{ error: 'blocked destination' }, { status: 302, headers: { location: 'http://127.0.0.1/' }, body: '' }]) {
    const { done } = run('<main>placeholder</main><script src="/app.js"></script>', () => reply);
    const result = await done;
    assert.equal(result.type, 'error');
    assert.equal(result.code, 1);
    assert.match(result.message, /blocked destination|redirects are unsupported/);
  }
});

test('POST resources fail before contacting the broker', async () => {
  const { done } = run('<script>fetch("/change", {method:"POST",body:"x"})</script>', () => {
    assert.fail('POST reached broker');
  });
  const result = await done;
  assert.equal(result.type, 'error');
  assert.match(result.message, /unsupported resource method/);
  assert.equal(result.requests.length, 0);
});

test('generated UTF-8 output stays within the byte limit', async () => {
  const { done } = run('<body><script>document.body.textContent="日".repeat(400)</script>', () => {
    assert.fail('unexpected resource');
  }, { maxBytes: 1024 });
  const result = await done;
  assert.equal(result.type, 'error');
  assert.match(result.message, /rendered HTML too large/);
});

test('WebRTC, WebTransport, and unhandled worker traffic cannot contact loopback', async () => {
  let hits = 0;
  const canary = http.createServer((_request, res) => { hits++; res.end('private'); });
  canary.on('upgrade', (_request, socket) => { hits++; socket.destroy(); });
  await new Promise(resolve => canary.listen(0, '127.0.0.1', resolve));
  try {
    const url = `http://127.0.0.1:${canary.address().port}`;
    const { done } = run(`<main></main><script>
      const results = [typeof RTCPeerConnection, typeof WebTransport];
      const registration = ServiceWorkerContainer.prototype.register.call(navigator.serviceWorker, '/sw.js')
        .then(() => 'unexpected registration', () => 'service worker blocked');
      const worker = new Worker(URL.createObjectURL(new Blob([
        'const ws = new WebSocket(${JSON.stringify(url.replace('http:', 'ws:') + '/socket')});' +
        'ws.onerror = () => postMessage("worker socket blocked");'
      ], {type:'application/javascript'})));
      const socketResult = new Promise(resolve => { worker.onmessage = e => resolve(e.data); });
      Promise.all([registration, socketResult]).then(values => {
        document.querySelector('main').textContent = [...results, ...values].join('|');
      });
    </script>`, () => ({ error: 'unexpected broker request' }), { url });
    const result = await done;
    assert.equal(result.type, 'html');
    const rendered = Buffer.from(result.body, 'base64').toString();
    assert.ok(rendered.includes('<main>undefined|undefined|'));
    assert.ok(rendered.includes('service worker blocked'));
    assert.ok(rendered.includes('worker socket blocked'));
    assert.equal(hits, 0);
  } finally {
    await new Promise(resolve => canary.close(resolve));
  }
});

test('closing broker input terminates an unfinished render', async () => {
  const { child, done } = run('<script src="/waiting.js"></script>', () => {
    child.stdin.end();
    return { error: 'input closed' };
  });
  const result = await done;
  assert.equal(result.type, 'error');
  assert.equal(result.code, 1);
});

test('a missing browser reports a protocol error', async () => {
  const { done } = run('<main>text</main>', () => assert.fail('unexpected resource'), {
    env: { PLAYWRIGHT_BROWSERS_PATH: path.join(__dirname, 'missing-browser') },
  });
  const result = await done;
  assert.equal(result.type, 'error');
  assert.equal(result.code, 1);
  assert.match(result.message, /Executable doesn't exist/);
});

test('page exceptions produce bounded diagnostics', async () => {
  const { done } = run(`<script>
    Object.defineProperty(document.documentElement, 'outerHTML', {
      get() { throw new Error('x'.repeat(20000)); }
    });
  </script>`, () => assert.fail('unexpected resource'));
  const result = await done;
  assert.equal(result.type, 'error');
  assert.ok(result.message.includes('xxxxx'));
  assert.ok([...result.message].length <= 1024);
});
