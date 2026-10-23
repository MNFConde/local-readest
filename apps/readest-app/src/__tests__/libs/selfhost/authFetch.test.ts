import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// ROADMAP ⑨ — silent 401 re-login wrapper (libs/selfhost/authFetch).
//
// The module under test branches on the build-time SELFHOSTED constant, so
// every case stubs NEXT_PUBLIC_SELFHOSTED and re-imports the module fresh
// (which also resets the single-flight / rate-limit state).

const b64url = (s: string) => Buffer.from(s, 'utf-8').toString('base64');
const mkJwt = (payload: object) =>
  b64url('{"alg":"HS256","typ":"JWT"}') + '.' + b64url(JSON.stringify(payload)) + '.sig';

const staleJwt = mkJwt({ sub: 'owner', plan: 'pro', exp: 1000 });
const freshJwt = mkJwt({ sub: 'owner', plan: 'pro', exp: 9999999999 });
const otherAccountJwt = mkJwt({ sub: 'account-b', plan: 'pro', exp: 9999999999 });

const API = 'http://127.0.0.1:8080/api';
const seedSession = (token = staleJwt, credentials = { email: 'a@b.c', password: 'pw' }) => {
  localStorage.setItem('token', token);
  // selfhostedLogin refuses to run without a configured server URL ('no-url')
  localStorage.setItem('selfhosted_server_url', 'http://127.0.0.1:8080');
  if (credentials) {
    localStorage.setItem('selfhosted_credentials', JSON.stringify(credentials));
  }
};

const deferred = () => {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => (resolve = r));
  return { promise, resolve };
};

interface ServerOptions {
  /** What POST /api/auth responds with: 'ok' (loginToken), 'reject-401', or 'network-error'. */
  login?: 'ok' | 'reject-401' | 'network-error';
  /** Token the login endpoint issues (defaults to freshJwt / account A). */
  loginToken?: string;
  /** Held open until released — used to keep a re-login in flight (single-flight test). */
  loginGate?: Promise<void>;
  /** Async hook fired when a sync request arrives, before it is answered. */
  onSync?: () => void;
}

/** Fake sync server: /sync 401s anything not bearing `freshJwt`. */
const installServer = (opts: ServerOptions = {}) => {
  const state = { logins: 0, syncCalls: [] as (string | undefined)[] };
  const mock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    const auth = new Headers(init?.headers).get('Authorization') ?? undefined;
    if (url.includes('/api/auth')) {
      state.logins++;
      if (opts.loginGate) await opts.loginGate;
      if (opts.login === 'reject-401') {
        return new Response(JSON.stringify({ error: 'invalid credentials' }), { status: 401 });
      }
      if (opts.login === 'network-error') throw new TypeError('fetch failed');
      return Response.json({ access_token: opts.loginToken ?? freshJwt, token_type: 'bearer' });
    }
    if (url.includes('/sync')) {
      state.syncCalls.push(auth);
      await opts.onSync?.();
      if (auth === `Bearer ${freshJwt}`) return Response.json({ ok: true });
      return new Response(JSON.stringify({ error: 'invalid token', code: 'AUTH' }), {
        status: 401,
      });
    }
    return Response.json({ ok: true });
  });
  vi.stubGlobal('fetch', mock);
  return { state, mock };
};

/** Capture window.location.href assignments without jsdom navigation errors. */
const captureLocation = (pathname = '/library', search = '') => {
  const original = window.location;
  const fake = { href: '', pathname, search };
  Object.defineProperty(window, 'location', { value: fake, configurable: true });
  return () => Object.defineProperty(window, 'location', { value: original, configurable: true });
};

const importWrapper = async () => {
  vi.stubEnv('NEXT_PUBLIC_SELFHOSTED', '1');
  vi.resetModules();
  return import('@/libs/selfhost/authFetch');
};

describe('selfhostedAuthFetch (silent 401 re-login)', () => {
  let restoreLocation: (() => void) | null = null;

  beforeEach(() => {
    seedSession();
  });
  afterEach(() => {
    localStorage.clear();
    restoreLocation?.();
    restoreLocation = null;
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    vi.resetModules();
  });

  it('re-logs in with saved credentials and replays the request once', async () => {
    const server = installServer();
    const { selfhostedAuthFetch } = await importWrapper();

    const res = await selfhostedAuthFetch(`${API}/sync`, { method: 'POST', body: '{}' });

    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
    // exactly one re-login, and the replay carried the fresh token
    expect(server.state.logins).toBe(1);
    expect(server.state.syncCalls).toEqual([`Bearer ${staleJwt}`, `Bearer ${freshJwt}`]);
    // the renewed session is persisted the way AuthContext.login does
    expect(localStorage.getItem('token')).toBe(freshJwt);
    expect(JSON.parse(localStorage.getItem('user') ?? '{}').id).toBe('owner');
  });

  it('collapses concurrent 401s into a single re-login (single-flight)', async () => {
    const gate = deferred();
    const server = installServer({ loginGate: gate.promise });
    const { selfhostedAuthFetch } = await importWrapper();

    const p1 = selfhostedAuthFetch(`${API}/sync`, { method: 'POST', body: '{}' });
    const p2 = selfhostedAuthFetch(`${API}/sync`, { method: 'GET' });
    // let both requests 401 and both callers reach the re-login guard…
    await new Promise((r) => setTimeout(r, 0));
    expect(server.state.logins).toBe(1); // …but only one POST /api/auth went out
    gate.resolve();
    const [r1, r2] = await Promise.all([p1, p2]);

    expect(r1.status).toBe(200);
    expect(r2.status).toBe(200);
    expect(server.state.logins).toBe(1);
  });

  it('rate-limits re-login attempts to one per minute on transient failures', async () => {
    let fakeNow = 1_000_000;
    const nowSpy = vi.spyOn(Date, 'now').mockImplementation(() => fakeNow);
    const server = installServer({ login: 'network-error' }); // offline server
    const { selfhostedAuthFetch } = await importWrapper();

    const first = await selfhostedAuthFetch(`${API}/sync`);
    expect(first.status).toBe(401); // unhealed, original response
    expect(server.state.logins).toBe(1);

    fakeNow += 30_000; // inside the rate-limit window: no new attempt
    const second = await selfhostedAuthFetch(`${API}/sync`);
    expect(second.status).toBe(401);
    expect(server.state.logins).toBe(1);

    fakeNow += 31_000; // past the window: one more attempt allowed
    const third = await selfhostedAuthFetch(`${API}/sync`);
    expect(third.status).toBe(401);
    expect(server.state.logins).toBe(2);
    nowSpy.mockRestore();
  });

  it('falls back to the login page when the saved credentials are rejected', async () => {
    restoreLocation = captureLocation('/reader', '?ids=1');
    installServer({ login: 'reject-401' });
    const { selfhostedAuthFetch, SELFHOSTED_SESSION_INVALID_EVENT } = await importWrapper();
    let invalidEvent = 0;
    window.addEventListener(SELFHOSTED_SESSION_INVALID_EVENT, () => invalidEvent++);

    const res = await selfhostedAuthFetch(`${API}/sync`);

    expect(res.status).toBe(401); // surfaced untouched
    expect(invalidEvent).toBe(1);
    expect(localStorage.getItem('token')).toBeNull();
    expect(localStorage.getItem('user')).toBeNull();
    expect(localStorage.getItem('selfhosted_credentials')).toBeNull();
    expect(window.location.href).toBe(`/auth?redirect=${encodeURIComponent('/reader?ids=1')}`);
  });

  it('never re-logins for the login endpoint itself (401 = wrong credentials)', async () => {
    const { state, mock } = installServer({ login: 'reject-401' });
    const { selfhostedAuthFetch } = await importWrapper();

    const res = await selfhostedAuthFetch(`${API}/auth`, { method: 'POST', body: '{}' });

    expect(res.status).toBe(401);
    // exactly one round trip: the original request — no re-login follow-up
    expect(mock).toHaveBeenCalledTimes(1);
    expect(state.logins).toBe(1);
  });

  it('does not heal anonymous requests (no token attached)', async () => {
    localStorage.removeItem('token');
    const server = installServer();
    const { selfhostedAuthFetch } = await importWrapper();

    const res = await selfhostedAuthFetch(`${API}/sync`);

    expect(res.status).toBe(401);
    expect(server.state.logins).toBe(0);
  });

  it('never replays one account request under another account token', async () => {
    // The request goes out with account A's token; while it is in flight the
    // session storage is switched to account B (login-as-B racing an
    // expired-A 401). The re-login then succeeds as B — replaying A's
    // request under B's token would file A's rows under B, so the wrapper
    // must surface the original 401 instead.
    const server = installServer({
      loginToken: otherAccountJwt, // re-login issues B's token
      onSync: () => {
        localStorage.setItem('token', otherAccountJwt);
        localStorage.setItem(
          'selfhosted_credentials',
          JSON.stringify({ email: 'b@b.c', password: 'pw-b' }),
        );
      },
    });
    const { selfhostedAuthFetch } = await importWrapper();

    const res = await selfhostedAuthFetch(`${API}/sync`, { method: 'POST', body: '{}' });

    expect(res.status).toBe(401); // original response, no replay
    expect(server.state.syncCalls).toHaveLength(1); // exactly one /sync round trip
  });
});
