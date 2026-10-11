// CDP-driven self-host login smoke on the emulator's Readest WebView.
// Usage: node cdp-login.js <ws-url> <server-url> <login-code>
const [, , wsUrl, serverUrl, code] = process.argv;
if (!wsUrl || !serverUrl || !code) {
  console.error('usage: node cdp-login.js <ws-url> <server-url> <code>');
  process.exit(1);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let msgId = 0;
const pending = new Map();
const ws = new WebSocket(wsUrl);

const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++msgId;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });

ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? reject(new Error(JSON.stringify(msg.error))) : resolve(msg.result);
  }
};

const evalJs = async (expression, awaitPromise = true) => {
  const r = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise,
  });
  if (r.exceptionDetails) throw new Error('page threw: ' + JSON.stringify(r.exceptionDetails));
  return r.result.value;
};

ws.onopen = async () => {
  try {
    await send('Runtime.enable');

    // 1) Click the "Sign in to sync your library" entry on the empty library.
    const clicked = await evalJs(`(async () => {
      const deadline = Date.now() + 8000;
      while (Date.now() < deadline) {
        const els = [...document.querySelectorAll('a, button')];
        const el = els.find((e) => /sign in to sync/i.test(e.textContent || ''));
        if (el) { el.click(); return location.pathname; }
        await new Promise((r) => setTimeout(r, 400));
      }
      return null;
    })()`);
    console.log('sign-in link clicked, route =', clicked);
    if (!clicked) throw new Error('sign-in link not found');

    await sleep(1500);

    // 2) Fill the auth form (self-host mode: server url + login code).
    const filled = await evalJs(`(async () => {
      const deadline = Date.now() + 8000;
      while (Date.now() < deadline) {
        const s = document.querySelector('#server-url');
        const p = document.querySelector('#password');
        if (s && p) {
          const set = (el, v) => {
            const proto = Object.getPrototypeOf(el);
            const desc = Object.getOwnPropertyDescriptor(proto, 'value');
            desc.set.call(el, v);
            el.dispatchEvent(new Event('input', { bubbles: true }));
            el.dispatchEvent(new Event('change', { bubbles: true }));
          };
          set(s, ${JSON.stringify(serverUrl)});
          set(p, ${JSON.stringify(code)});
          return location.pathname;
        }
        await new Promise((r) => setTimeout(r, 400));
      }
      return null;
    })()`);
    console.log('form filled on route =', filled);
    if (!filled) {
      console.log('BODY:', await evalJs('document.body.innerText.slice(0, 600)', false));
      throw new Error('#server-url / #password not found');
    }

    // 3) Submit.
    await evalJs(`(async () => {
      const btn = [...document.querySelectorAll('button')].find((b) =>
        /^sign in$/i.test((b.textContent || '').trim()));
      if (!btn) throw new Error('submit button not found');
      btn.click();
    })()`);

    // 4) Wait for the JWT to land in localStorage (silent re-login token).
    const tokenKey = await evalJs(`(async () => {
      const deadline = Date.now() + 15000;
      while (Date.now() < deadline) {
        const hit = Object.keys(localStorage).find((k) =>
          /token|auth|jwt/i.test(k) && localStorage.getItem(k) && localStorage.getItem(k).length > 40);
        if (hit) return hit + ' = ' + localStorage.getItem(hit).slice(0, 24) + '…';
        await new Promise((r) => setTimeout(r, 500));
      }
      return null;
    })()`);
    console.log('token:', tokenKey || 'NOT FOUND');
    console.log(tokenKey ? 'LOGIN-SMOKE=PASS' : 'LOGIN-SMOKE=FAIL');
    process.exit(tokenKey ? 0 : 1);
  } catch (e) {
    console.error('LOGIN-SMOKE=FAIL:', e.message);
    process.exit(1);
  }
};

ws.onerror = (e) => {
  console.error('WS error', e.message || e);
  process.exit(1);
};
