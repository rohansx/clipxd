// clipxd-capture (Playwright) — drives a page and emits a clipxd browser trace.
// Usage: node capture.mjs <out-dir>
import { chromium } from 'playwright';
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const OUT = process.argv[2] || path.join(__dirname, 'clip');
fs.mkdirSync(path.join(OUT, 'frames'), { recursive: true });
const html = fs.readFileSync(path.join(__dirname, 'checkout.html'), 'utf8');

// tiny origin: GET / -> the page; POST /api/checkout -> 500
const server = http.createServer((req, res) => {
  if (req.method === 'POST' && req.url === '/api/checkout') {
    res.writeHead(500, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: 'payment_processor_unavailable' }));
  } else {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(html);
  }
});
await new Promise((r) => server.listen(0, r));
const base = `http://localhost:${server.address().port}`;

const started = Date.now();
const events = [];
const push = (e) => events.push({ ...e, t_ms: e.t_ms ?? Date.now() });
let shot = 0;
async function screenshot(page, reason) {
  shot += 1;
  const rel = `frames/${String(shot).padStart(6, '0')}.png`;
  await page.screenshot({ path: path.join(OUT, rel) });
  push({ type: 'screenshot', path: rel, reason, redacted: true });
}

const browser = await chromium.launch({ args: ['--no-sandbox'] });
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });

// in-page recorder (clicks + DOM mutations + alert a11y), page-side timestamps
await page.exposeBinding('__clipxdEmit', (_src, ev) => push(ev));
await page.addInitScript(() => {
  const sel = (el) => (!el || el === document.body ? 'body'
    : el.id ? el.tagName.toLowerCase() + '#' + el.id
    : (el.tagName || 'node').toLowerCase());
  document.addEventListener('click', (e) => {
    const t = e.target.closest('button,a,[role=button],input') || e.target;
    window.__clipxdEmit({ type: 'click', t_ms: Date.now(), click_kind: 'click', target: sel(t),
      label: ((t.getAttribute && t.getAttribute('aria-label')) || t.textContent || '').trim() || null,
      x: e.clientX, y: e.clientY });
  }, true);
  const observe = () => {
    const root = document.documentElement || document.body;
    if (!root) { document.addEventListener('DOMContentLoaded', observe, { once: true }); return; }
    new MutationObserver((muts) => {
      for (const m of muts) for (const n of m.addedNodes) {
        if (n.nodeType !== 1) continue;
        const role = n.getAttribute && n.getAttribute('role');
        const name = (n.textContent || '').trim().slice(0, 200);
        window.__clipxdEmit({ type: 'dom_mutation', t_ms: Date.now(), target: '#' + (m.target.id || sel(m.target)),
          op: 'insert', added: 1, removed: 0, text_delta: (n.textContent || '').length, role, name });
        if (role === 'alert' || role === 'status') {
          window.__clipxdEmit({ type: 'a11y_text', t_ms: Date.now(), selector: sel(n), role, text: name });
        }
      }
    }).observe(root, { childList: true, subtree: true });
  };
  observe();
});

page.on('console', (m) => push({ type: 'console',
  level: m.type() === 'error' ? 'error' : m.type() === 'warning' ? 'warn' : 'log',
  text: m.text(), source: 'javascript', uncaught: false }));
page.on('pageerror', (e) => push({ type: 'console', level: 'error', text: e.message, uncaught: true }));
page.on('response', (r) => push({ type: 'network', method: r.request().method(), url: r.url(),
  status: r.status(), status_text: r.statusText(), resource_type: r.request().resourceType(), initiator: 'script' }));
page.on('framenavigated', (f) => { if (f === page.mainFrame()) push({ type: 'navigate', url: f.url(), nav_kind: 'load', title: null }); });

// drive the checkout-500 scenario
await page.goto(base + '/');
await page.waitForLoadState('load');
push({ type: 'dom_snapshot', url: base + '/', node_count: 0, text: await page.evaluate(() => document.body.innerText) });
for (const [s, role] of [['h1', 'heading'], ['#place-order', 'button']]) {
  const el = await page.$(s);
  if (el) push({ type: 'a11y_text', selector: s, role, text: (await el.innerText()).trim() });
}
// fill in the document title on the navigate event
const title = await page.title();
for (const e of events) if (e.type === 'navigate') e.title = title;
await screenshot(page, 'navigation');

await page.click('#place-order');
await page.waitForSelector('.toast', { timeout: 5000 });
await page.waitForTimeout(250);
await screenshot(page, 'error');
await page.waitForTimeout(250);
await screenshot(page, 'state_settle');

await browser.close();
server.close();

events.sort((a, b) => a.t_ms - b.t_ms);
const trace = {
  clipxd_trace_version: '1', session_id: 'capture-' + started,
  captured_by: 'clipxd-capture-playwright/0.1', started_at_ms: started,
  viewport: { w: 1280, h: 800 }, url: base + '/', events,
};
const outFile = path.join(OUT, 'session.trace.json');
fs.writeFileSync(outFile, JSON.stringify(trace, null, 2));
console.log('wrote', outFile, '—', events.length, 'events,', shot, 'screenshots');
