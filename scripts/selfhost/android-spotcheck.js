// Android spot-check: OPDS native browse + download on the emulator.
// Opens the acquisition feed directly with credentials embedded as URL
// userinfo (applies to the first load — enough for browse + download).
// Usage: node android-spotcheck.js <ws-http-port=9222> <host-url-no-scheme> <code> <title>
(async () => {
  const [, , port = '9222', host, code, title] = process.argv;
  if (!host || !code || !title) {
    console.error('usage: node android-spotcheck.js <port> <host:port> <code> <title>');
    process.exit(1);
  }
  const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const page = list.find((t) => t.type === 'page');
  let msgId = 0;
  const pending = new Map();
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  const send = (m, p = {}) =>
    new Promise((res, rej) => {
      const id = ++msgId;
      pending.set(id, { res, rej });
      ws.send(JSON.stringify({ id, method: m, params: p }));
    });
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) {
      const p = pending.get(m.id);
      pending.delete(m.id);
      m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result);
    }
  };
  await new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = () => rej(new Error('ws'));
  });
  const ev = async (expr) => {
    const r = await send('Runtime.evaluate', {
      expression: expr,
      returnByValue: true,
      awaitPromise: true,
    });
    if (r.exceptionDetails)
      return 'EXC:' + (r.exceptionDetails.exception?.description || '').slice(0, 120);
    return r.result.value;
  };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const results = {};
  const report = (code2, ok, note = '') => {
    results[code2] = ok;
    console.log(`${code2}=${ok ? 'PASS' : 'FAIL'} ${note}`);
  };
  await send('Runtime.enable');

  // Books feed as the entry page, credentials in userinfo.
  const feedUrl =
    'http://' + encodeURIComponent('smoke@readest.test') + ':' + code + '@' + host + '/opds/books';
  await ev(`location.href = '/opds?url=' + encodeURIComponent(${JSON.stringify(feedUrl)}); 'nav'`);
  await sleep(6000);

  let browse = '';
  for (let i = 0; i < 20; i++) {
    browse = (await ev('document.body.innerText', false)) || '';
    if (browse.includes(title)) break;
    await sleep(1000);
  }
  report(
    'ANDROID-OPDS-BROWSE',
    browse.includes(title),
    browse.includes(title)
      ? 'feed rendered natively'
      : browse.slice(0, 120).split('\n').join(' | '),
  );

  // Title → detail → download.
  const invoke = (pattern) => `(() => {
    const t = (e) => (e.textContent || '').trim();
    const hits = [...document.querySelectorAll('body *')].filter((e) => ${pattern}.test(t(e)));
    for (const el of hits.reverse()) {
      let node = el;
      for (let i = 0; i < 10 && node; i++) {
        const key = Object.keys(node).find((k) => k.startsWith('__reactProps'));
        if (key && node[key] && node[key].onClick) {
          node[key].onClick(new MouseEvent('click', { bubbles: true }));
          return 'invoked';
        }
        node = node.parentElement;
      }
    }
    return 'no-handler';
  })()`;

  let detail = '';
  for (let i = 0; i < 12; i++) {
    detail =
      (await ev(invoke(new RegExp('^' + title.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '$')))) ||
      '';
    const has =
      (await ev(
        `[...document.querySelectorAll('button, a')].some((e) => /下载|download|epub/i.test((e.textContent || '') + (e.getAttribute('aria-label') || ''))) ? 'OK' : 'NO'`,
        false,
      )) || '';
    if (has === 'OK') break;
    await sleep(1000);
  }
  let clicked = '';
  for (let i = 0; i < 10; i++) {
    clicked =
      (await ev(invoke(/下载( [A-Za-z0-9]+)?|download( [a-z0-9]+)?( again)?|^(epub|pdf)$/i))) || '';
    if (clicked.startsWith('invoked')) break;
    await sleep(800);
  }
  let outcome = '';
  for (let i = 0; i < 60; i++) {
    await sleep(1000);
    const text = (await ev('document.body.innerText', false)) || '';
    if (/下载完成|Download completed/i.test(text)) {
      outcome = 'completed';
      break;
    }
    if (/下载失败|Download failed/i.test(text)) {
      outcome = 'failed';
      break;
    }
  }
  report('ANDROID-OPDS-DOWNLOAD', outcome === 'completed', outcome);

  const allPass = Object.values(results).every(Boolean);
  console.log(allPass ? 'ANDROID-SPOTCHECK=PASS' : 'ANDROID-SPOTCHECK=FAIL');
  process.exit(allPass ? 0 : 1);
})().catch((e) => {
  console.error('ANDROID-SPOTCHECK=FAIL:', e.message);
  process.exit(1);
});
