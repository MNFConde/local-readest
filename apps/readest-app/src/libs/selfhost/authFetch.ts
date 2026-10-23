// SELFHOSTED (fork): silent 401 re-login for the sync-server API surface.
//
// The self-hosted JWT lives ~1h and the only silent re-login used to be the
// one-shot `refreshSelfhostedSession` at AuthContext mount. An app left open
// across an offline period — or a server-side JWT_SECRET rotation — lets the
// token expire mid-session, after which every sync request 401s until the
// app is restarted (ROADMAP ⑨).
//
// Sync-server requests therefore route through `selfhostedAuthFetch`, a
// drop-in fetch() replacement that on 401 re-logins with the persisted
// credentials and replays the original request once. Guard rails:
//   * single-flight — concurrent 401s share one re-login (no thundering herd);
//   * rate limit — at most one attempt per minute, so a server whose secret
//     rotated is never hammered with credential attempts;
//   * the login endpoint itself never triggers a re-login — its own
//     401/403 means the credentials are wrong, and looping them would never
//     terminate;
//   * a replay only happens when the renewed token belongs to the SAME
//     account as the request's original token, so an account switch racing
//     an in-flight request can never file the old account's rows under the
//     new one.
// Only a definitive credential rejection tears the session down and falls
// back to the login page; network/server/locked failures keep the stale
// token so the next sync window can try again.

import { getAccessToken } from '@/utils/access';
import {
  SelfhostedLoginError,
  clearCredentials,
  getSavedCredentials,
  jwtSub,
  jwtToUser,
  selfhostedLogin,
} from '@/services/selfhostedAuth';

/** Minimum spacing between two silent re-login attempts. */
const RELOGIN_MIN_INTERVAL_MS = 60_000;

/** Fired on `window` when saved credentials are rejected and the session is torn down. */
export const SELFHOSTED_SESSION_INVALID_EVENT = 'selfhosted:session-invalid';

let inflightRelogin: Promise<boolean> | null = null;
let lastReloginAttemptAt = 0;

const isLoginEndpoint = (input: RequestInfo | URL): boolean => {
  const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
  return url.includes('/api/auth');
};

// A streaming body can only be consumed once, so a request built on one
// cannot be replayed — hand the 401 back to the caller instead. (Every sync
// call site sends a JSON string or no body at all.)
const isReplayableBody = (body: unknown): boolean =>
  !(typeof ReadableStream !== 'undefined' && body instanceof ReadableStream);

const sameAccount = (a: string | null, b: string | null): boolean =>
  !!a && !!b && jwtSub(a) === jwtSub(b);

const buildAuthInit = async (
  init: RequestInit,
): Promise<{ init: RequestInit; token: string | null }> => {
  const token = await getAccessToken();
  const headers = new Headers(init.headers);
  if (token) headers.set('Authorization', `Bearer ${token}`);
  else headers.delete('Authorization');
  return { init: { ...init, headers }, token };
};

/**
 * Re-login with the persisted credentials, guarded to a single in-flight
 * attempt and one attempt per minute. Returns whether a fresh token is in
 * `localStorage` now. Callers whose transport cannot replay a request (the
 * file transfer adapters) use this to heal their NEXT attempt instead.
 */
export async function triggerSelfhostedRelogin(): Promise<boolean> {
  if (inflightRelogin) return inflightRelogin;
  if (Date.now() - lastReloginAttemptAt < RELOGIN_MIN_INTERVAL_MS) return false;
  lastReloginAttemptAt = Date.now();
  inflightRelogin = reloginWithSavedCredentials().finally(() => {
    inflightRelogin = null;
  });
  return inflightRelogin;
}

async function reloginWithSavedCredentials(): Promise<boolean> {
  const credentials = getSavedCredentials();
  if (!credentials) return false;
  try {
    const { access_token } = await selfhostedLogin(credentials.email, credentials.password);
    const user = jwtToUser(access_token);
    if (!user) return false;
    // Persist exactly what AuthContext.login writes, so every
    // getAccessToken() call (they read localStorage directly) and the next
    // app start see the renewed session without a React round trip. The
    // React-side token state keeps its stale copy — harmless: the identity
    // claims (sub/plan) are unchanged and all API traffic reads localStorage.
    localStorage.setItem('token', access_token);
    localStorage.setItem('user', JSON.stringify(user));
    return true;
  } catch (e) {
    if (e instanceof SelfhostedLoginError && e.reason === 'invalid-credentials') {
      // The credentials themselves were rejected (password changed / account
      // deleted): this session can never heal itself — fall to the login page.
      handleSelfhostedSessionInvalid();
    }
    // network / server / locked: transient — keep the stale token and the
    // saved credentials; the rate limiter bounds how often we retry.
    return false;
  }
}

/**
 * Tear down the dead session and fall back to the login page. A hard
 * navigation rather than SPA router.push: the wrapper runs outside React,
 * and every in-memory store still belongs to the dead session.
 */
export function handleSelfhostedSessionInvalid(): void {
  localStorage.removeItem('token');
  localStorage.removeItem('user');
  clearCredentials();
  window.dispatchEvent(new CustomEvent(SELFHOSTED_SESSION_INVALID_EVENT));
  const { pathname, search } = window.location;
  const redirect = pathname !== '/auth' ? pathname + search : '/';
  window.location.href = `/auth?redirect=${encodeURIComponent(redirect)}`;
}

/**
 * fetch() drop-in for the self-hosted sync-server API: attaches the current
 * Bearer token and, on 401, renews the session (single-flight, rate-limited)
 * and replays the request ONCE with the fresh token. Everything else —
 * including a 401 that could not be healed — returns the original response.
 */
export async function selfhostedAuthFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  const first = await buildAuthInit(init);
  const response = await fetch(input, first.init);
  if (response.status !== 401) return response;
  if (!first.token) return response; // anonymous request: nothing to renew
  if (isLoginEndpoint(input) || !isReplayableBody(init.body)) return response;

  // Fast path: another request renewed the token while this one was in
  // flight with the old one — replay immediately, no re-login round trip.
  const current = await getAccessToken();
  let healed = !!current && current !== first.token && sameAccount(current, first.token);
  if (!healed) healed = await triggerSelfhostedRelogin();
  if (!healed) return response;
  // An account switch racing this request renewed storage to ANOTHER
  // account's token — replaying would file the old account's rows under the
  // new one. Surface the 401 untouched instead.
  if (!sameAccount(await getAccessToken(), first.token)) return response;

  const retry = await buildAuthInit(init);
  return fetch(input, retry.init);
}

/**
 * fetchWithTimeout counterpart for callers that previously used the shared
 * helper (libs/sync.ts): same abort semantics — the replay rides the same
 * signal, so re-login + replay stay inside the caller's total timeout.
 */
export const selfhostedFetchWithTimeout = (
  url: string,
  options: RequestInit = {},
  timeout = 10000,
): Promise<Response> => {
  const controller = new AbortController();
  const id = setTimeout(() => controller.abort('Request timed out'), timeout);
  return selfhostedAuthFetch(url, { ...options, signal: controller.signal }).finally(() =>
    clearTimeout(id),
  );
};
