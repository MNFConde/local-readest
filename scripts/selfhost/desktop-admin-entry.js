// Desktop (Tauri/WebView2) CDP gate for ROADMAP ⑤: admin console menu entry.
// Launch readest.exe first with:
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223
// Usage: node desktop-admin-entry.js <http-port=9223> <server-url> <email> <password>
const [, , httpPort = '9223', serverUrl, email, password] = process.argv;
if (!serverUrl || !email || !password) {
  console.error('usage: node desktop-admin-entry.js <port> <server-url> <email> <password>');
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

const evalJs = async (expression, awaitPromise = false) => {
  const r = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise,
  });
  if (r.exceptionDetails) throw new Error('page threw: ' + JSON.stringify(r.exceptionDetails));
  return r.result.value;
};

let failed = 0;
const report = (code, ok, note = '') => {
  if (!ok) failed++;
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

const waitFor = async (condJs, timeoutMs, what) => {
  const deadline = Date.now() + timeoutMs;
  let note = '';
  while (Date.now() < deadline) {
    note = (await evalJs(condJs)) || '';
    if (String(note).startsWith('OK')) return String(note).slice(3).trim();
    await sleep(700);
  }
  return 'TIMEOUT waiting ' + what + ' last=' + String(note).slice(0, 120);
};

// Set a React-controlled input via the native value setter + input events.
const fillInput = (placeholderRe, value) => `(() => {
  const re = ${placeholderRe};
  const el = [...document.querySelectorAll('input')].find((i) => re.test(i.placeholder || '') || re.test(i.getAttribute('aria-label') || ''));
  if (!el) return 'missing';
  const desc = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value');
  desc.set.call(el, ${JSON.stringify(value)});
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return 'filled';
})()`;

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

// Native click on a real button/link: bubbles through the React root, and
// form-submit buttons (login) work without any React onClick handle.
const nativeClickText = (pattern) => `(() => {
  const hits = [...document.querySelectorAll('button, [role="button"], [role="menuitem"], a, summary')]
    .filter((e) => ${pattern}.test((e.textContent || '').trim()));
  if (!hits.length) return 'none';
  const el = hits[hits.length - 1];
  el.click();
  return 'clicked:' + el.tagName;
})()`;

(async () => {
  await connect();
  await sleep(2500);

  // Fresh auth state: clear persisted token/server URL, then open /auth.
  await evalJs('localStorage.clear(); location.href = "/auth"; "cleared"');
  await sleep(2500);
  report(
    'AUTH_PAGE',
    (await evalJs('location.pathname')) === '/auth',
    'pathname=' + (await evalJs('location.pathname')),
  );

  await evalJs(fillInput(/sync\.your-server|同步服务器地址/, serverUrl));
  console.log('  fill server:', 'done');
  console.log('  fill email:', await evalJs(fillInput(/电子邮件|email/i, email)));
  console.log('  fill pass :', await evalJs(fillInput(/密码|password/i, password)));
  let click = await evalJs(nativeClickText(/登录|^Sign in$/));
  if (click === 'none') click = await evalJs(invokeText(/登录|^Sign in$/));
  console.log('  login click:', click);
  const where = await waitFor(
    `location.pathname === '/library' ? 'OK library' : 'at ' + location.pathname`,
    20000,
    'redirect to /library',
  );
  report('LOGIN', where === 'library', where);

  // Open the settings menu (icon-only trigger: aria-label carries the name)
  // and look for the admin entry.
  const trigger = await evalJs(`(() => {
    const el = [...document.querySelectorAll('button')].find((e) =>
      (e.getAttribute('aria-label') || '').includes('设置') || /设置菜单/.test(e.textContent || ''));
    if (!el) return 'none';
    el.click();
    return 'clicked';
  })()`);
  console.log('  settings trigger:', trigger);
  const entryVisible = await waitFor(
    `[...document.querySelectorAll('body *')].some((e) => (e.textContent || '').trim() === '管理控制台') ? 'OK visible' : 'not yet'`,
    8000,
    'admin entry in menu',
  );
  report('ADMIN_ENTRY_VISIBLE', entryVisible === 'visible', entryVisible);

  // Click through: the entry must land on /admin with the console rendered.
  await evalJs(invokeText(/^管理控制台$/));
  const adminWhere = await waitFor(
    `location.pathname === '/admin' && document.body.innerText.includes('管理控制台') ? 'OK admin' : 'at ' + location.pathname`,
    15000,
    'navigate to /admin',
  );
  report('ADMIN_NAV', adminWhere === 'admin', adminWhere);

  console.log(failed === 0 ? 'GATE=PASS' : 'GATE=FAIL');
  process.exit(failed === 0 ? 0 : 1);
})().catch((e) => {
  console.error('FATAL', e.message);
  console.log('GATE=FAIL');
  process.exit(1);
});
