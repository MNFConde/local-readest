// Desktop (Tauri/WebView2) CDP gate: M1 zh-CN assertion + selfhost login +
// OPDS native browse + download. Launch readest.exe first with:
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223
// Usage: node desktop-cdp.js <http-port=9223> <server-url> <login-code> <ignored> <expected-title>
const [, , httpPort = '9223', serverUrl, loginCode, , expectedTitle] = process.argv;
if (!serverUrl || !loginCode || !expectedTitle) {
  console.error('usage: node desktop-cdp.js <port> <server-url> <code> <opds-url> <title>');
  process.exit(1);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let msgId = 0;
const pending = new Map();
let ws;

const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++msgId;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });

const evalJs = async (expression, awaitPromise = true) => {
  const r = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise,
  });
  if (r.exceptionDetails) throw new Error('page threw: ' + JSON.stringify(r.exceptionDetails));
  return r.result.value;
};

const results = {};
const report = (code, ok, note = '') => {
  results[code] = ok;
  console.log(`${code}=${ok ? 'PASS' : 'FAIL'} ${note}`);
};

async function connect() {
  const list = await (await fetch(`http://127.0.0.1:${httpPort}/json/list`)).json();
  const page = list.find((t) => t.type === 'page');
  if (!page) throw new Error('no page target');
  ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = () => rej(new Error('WS error'));
  });
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) {
      const p = pending.get(m.id);
      pending.delete(m.id);
      m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result);
    }
  };
  await send('Runtime.enable');
}

const clickText = (pattern) => `(async () => {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    const hits = [...document.querySelectorAll('a, button, [role], h3, h4')]
      .filter((e) => ${pattern}.test((e.textContent || '').trim()));
    if (hits.length) { hits[hits.length - 1].click(); return true; }
    await new Promise((r) => setTimeout(r, 400));
  }
  return false;
})()`;

// Invoke the nearest React onClick for the innermost element matching pattern.
const invokeText = (pattern) => `(() => {
  const t = (e) => (e.textContent || '').trim();
  const hits = [...document.querySelectorAll('body *')].filter((e) => ${pattern}.test(t(e)));
  for (const el of hits.reverse()) {
    let node = el;
    for (let i = 0; i < 10 && node; i++) {
      const key = Object.keys(node).find((k) => k.startsWith('__reactProps'));
      if (key && node[key] && node[key].onClick) {
        node[key].onClick(new MouseEvent('click', { bubbles: true }));
        return 'invoked:' + node.tagName;
      }
      node = node.parentElement;
    }
  }
  return 'no-handler';
})()`;

const waitFor = async (condJs, timeoutMs, what) => {
  const deadline = Date.now() + timeoutMs;
  let note = '';
  while (Date.now() < deadline) {
    note = (await evalJs(condJs, false)) || '';
    if (note.startsWith('OK')) return note.slice(3).trim();
    await sleep(700);
  }
  return 'TIMEOUT waiting ' + what + ' last=' + note.slice(0, 100);
};

// Retry [invoke pattern → wait cond] until cond passes or timeout.
const invokeUntil = async (pattern, condJs, timeoutMs, what) => {
  const deadline = Date.now() + timeoutMs;
  let last = '';
  let n = 0;
  while (Date.now() < deadline) {
    n++;
    last = (await evalJs(invokeText(pattern), false)) || '';
    const loc = await evalJs('location.pathname', false);
    const cond = (await evalJs(condJs, false)) || '';
    console.log(`  [${what} #${n}] invoke=${last} loc=${loc} cond=${cond.slice(0, 50)}`);
    if (cond.startsWith('OK')) return cond.slice(3).trim();
    if (last.startsWith('invoked')) {
      const dl = Date.now() + 5000;
      while (Date.now() < dl) {
        const c = (await evalJs(condJs, false)) || '';
        if (c.startsWith('OK')) return c.slice(3).trim();
        await sleep(500);
      }
    }
    await sleep(700);
  }
  return 'TIMEOUT waiting ' + what + ' last=' + last;
};

const fillCatalogForm = async () => {
  const r = await evalJs(`(() => {
    const setVal = (el, v) => {
      const desc = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value');
      desc.set.call(el, v);
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
    };
    const byP = (re) => [...document.querySelectorAll('input')].find((i) => re.test(i.placeholder || ''));
    const byUrl = () => [...document.querySelectorAll('input')].find((i) => (i.placeholder || '').includes('example.com/opds'));
    const name = byP(/calibre 图书馆|calibre library/i);
    const url = byUrl();
    const user = byP(/用户名|username/i);
    const pass = byP(/密码|password/i);
    if (!name || !url || !user || !pass) return 'missing';
    setVal(name, 'Local Library');
    setVal(url, ${JSON.stringify(serverUrl)} + '/opds');
    setVal(user, 'smoke@readest.test');
    setVal(pass, ${JSON.stringify(loginCode)});
    return 'filled';
  })()`);
  if (r !== 'filled') throw new Error('catalog form fill failed: ' + r);
  // Submit ignores synthetic el.click() — invoke React onClick.
  await evalJs(`(() => {
    const hits = [...document.querySelectorAll('button')].filter(e =>
      /^(添加目录|保存更改|保存|add catalog|save changes)$/i.test((e.textContent||'').trim()));
    for (const el of hits.reverse()) {
      let node = el;
      for (let i = 0; i < 8 && node; i++) {
        const key = Object.keys(node).find(k => k.startsWith('__reactProps'));
        if (key && node[key] && node[key].onClick) {
          node[key].onClick(new MouseEvent('click', { bubbles: true }));
          return 'invoked';
        }
        node = node.parentElement;
      }
    }
    return 'none';
  })()`);
  // Validation probes the catalog URL over the network — allow up to 9s.
  await sleep(9000);
};

// (Re)open the online-library manager from the library page.
const openManager = async () => {
  await evalJs(`location.href = '/'; setTimeout(() => location.reload(), 200); 'nav'`, false);
  await sleep(5000);
  // Open the "+" import dropdown (aria-label 导入书籍/Import) — mobile
  // layouts have two visible matches; the LAST one is the header control.
  await evalJs(`(() => {
    const b = [...document.querySelectorAll('button')].filter(x => /导入书籍|import/i.test(x.getAttribute('aria-label') || ''));
    const btn = b[b.length - 1];
    if (btn) { btn.focus(); btn.click(); return true; }
    return false;
  })()`);
  await sleep(800);
  await evalJs(
    `(() => { const h = [...document.querySelectorAll('a,button,[role],li')].filter(e => /OPDS 目录|在线书库|OPDS Catalogs|Online Library/i.test((e.textContent||'').trim())); if (h.length) { h[h.length-1].click(); return 1; } return 0; })()`,
  );
  await sleep(2000);
};

// Fiber-read the saved catalog and jump to its /opds page.
const fiberNav = () =>
  evalJs(`(() => {
    const el = [...document.querySelectorAll('body *')]
      .find((e) => (e.textContent || '').trim() === 'Local Library');
    if (!el) return 'no-card';
    const fk = Object.keys(el).find((k) => k.startsWith('__reactFiber'));
    let fiber = fk ? el[fk] : null;
    for (let d = 0; fiber && d < 25; d++) {
      const cat = fiber.memoizedProps && fiber.memoizedProps.catalog;
      if (cat && cat.id && cat.url) {
        location.href = '/opds?url=' + encodeURIComponent(cat.url) + '&id=' + encodeURIComponent(cat.id);
        return 'nav:' + cat.id;
      }
      fiber = fiber.return;
    }
    return 'no-fiber';
  })()`);

const onFeedWithTitle = (title) =>
  `location.pathname.startsWith('/opds') && document.body.innerText.includes(${JSON.stringify(title)}) ? 'OK in feed' : 'NO'`;
const onOpds = `location.pathname.startsWith('/opds') ? 'OK on /opds' : 'NO'`;

async function main() {
  await connect();

  // ── A: M1 zh-CN login UI assertion ──
  await evalJs(`localStorage.setItem('i18nextLng', 'zh-CN'); 'ok'`, false);
  await evalJs(`location.href = '/auth'; 'nav'`, false);
  await sleep(5000);
  const zh = await evalJs(`document.body.innerText.includes('同步服务器地址')`, false);
  report(
    'M1-ZH-LOGIN',
    zh,
    zh ? 'found 同步服务器地址' : await evalJs(`document.body.innerText.slice(0, 150)`, false),
  );

  // ── B: selfhost login ──
  const filled = await evalJs(`(async () => {
    const deadline = Date.now() + 8000;
    while (Date.now() < deadline) {
      const s = document.querySelector('#server-url');
      const p = document.querySelector('#password');
      if (s && p) {
        const set = (el, v) => {
          const desc = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value');
          desc.set.call(el, v);
          el.dispatchEvent(new Event('input', { bubbles: true }));
          el.dispatchEvent(new Event('change', { bubbles: true }));
        };
        set(s, ${JSON.stringify(serverUrl)});
        set(p, ${JSON.stringify(loginCode)});
        return true;
      }
      await new Promise((r) => setTimeout(r, 400));
    }
    return false;
  })()`);
  if (!filled) throw new Error('#server-url/#password not found');
  await evalJs(clickText('/^sign in$|^登录$/i'));
  let token = null;
  for (let i = 0; i < 30 && !token; i++) {
    await sleep(1000);
    token = await evalJs(
      `(() => { const t = localStorage.getItem('token'); return t && t.length > 40 ? 'yes' : null; })()`,
      false,
    );
  }
  report('DESKTOP-LOGIN', !!token);

  // ── C: OPDS add-source → browse (with nav retries — the /opds page can
  // bounce back to /library; each attempt re-opens the manager) ──
  let browsed = '';
  for (let attempt = 1; attempt <= 3 && !browsed.includes('in feed'); attempt++) {
    console.log(`── browse attempt ${attempt} ──`);
    await openManager();
    const exists = await evalJs(
      `[...document.querySelectorAll('div, li')].some((d) =>
        (d.textContent || '').includes('Local Library') &&
        (d.textContent || '').includes('/opds')) ? 'yes' : 'no'`,
      false,
    );
    console.log('catalog exists:', exists);
    if (exists === 'no') {
      await evalJs(
        `(() => { const h = [...document.querySelectorAll('button')].filter(e => /^(添加目录|添加 OPDS 目录|添加第一个目录|add catalog|add opds catalog|add your first catalog)$/i.test((e.textContent||'').trim())); if (h.length) { h[0].click(); return 1; } return 0; })()`,
      );
      await sleep(1200);
      try {
        await fillCatalogForm();
      } catch (e) {
        console.log('fill failed this attempt:', e.message);
        continue;
      }
    }
    const nav = await fiberNav();
    const navWait = await waitFor(onOpds, 8000, '/opds');
    console.log('nav:', nav, '→', navWait);
    if (!navWait.includes('/opds')) continue;
    browsed = await invokeUntil(
      /^(all books|全部书目)$/i,
      onFeedWithTitle(expectedTitle),
      25000,
      'title in feed',
    );
  }
  report('OPDS-BROWSE', browsed.includes('in feed'), browsed);

  // ── D: download via the native path ──
  const done = await invokeUntil(
    new RegExp('^' + expectedTitle.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '$'),
    `[...document.querySelectorAll('button, a')].some((e) =>
      /^(epub|pdf|mobi|azw3|fb2|cbz|(重新)?下载( [A-Za-z0-9]+)?|download( [a-z0-9]+)?( again)?)$/gi.test((e.textContent || '').trim())) ? 'OK detail' : 'NO'`,
    20000,
    'detail view',
  );
  if (!done.includes('detail')) {
    report('OPDS-DOWNLOAD', false, done);
  } else {
    await invokeUntil(
      /^(epub|pdf|mobi|azw3|fb2|cbz|(重新)?下载( [A-Za-z0-9]+)?|download( [a-z0-9]+)?( again)?)$/gi,
      onOpds,
      10000,
      'download click',
    );
    let outcome = '';
    for (let i = 0; i < 60; i++) {
      await sleep(1000);
      const text = await evalJs('document.body.innerText', false);
      if (/下载完成|Download completed/i.test(text)) {
        outcome = 'completed';
        break;
      }
      if (/下载失败|Download failed/i.test(text)) {
        outcome = 'failed';
        break;
      }
    }
    report('OPDS-DOWNLOAD', outcome === 'completed', outcome);
  }

  // Screenshot for the archive.
  const shot = await send('Page.captureScreenshot', { format: 'png' });
  const { writeFileSync, mkdirSync } = await import('node:fs');
  mkdirSync('tmp/spotcheck', { recursive: true });
  writeFileSync('tmp/spotcheck/desktop-m3.png', Buffer.from(shot.data, 'base64'));
  console.log('screenshot: tmp/spotcheck/desktop-m3.png');

  const allPass = Object.values(results).every(Boolean);
  console.log(allPass ? 'DESKTOP-GATE=PASS' : 'DESKTOP-GATE=FAIL');
  process.exit(allPass ? 0 : 1);
}

main().catch((e) => {
  console.error('DESKTOP-GATE=FAIL:', e.message);
  process.exit(1);
});
