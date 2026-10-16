import { getSelfhostedServerUrl } from './selfhostedServerUrl';

export interface SelfhostedLoginResult {
  access_token: string;
  token_type: string;
}

export interface SelfhostedUser {
  id: string;
  email?: string;
  // Mirrors the Supabase `User` shape so UI code can read avatar/name metadata
  // from either auth backend through the same accessor.
  user_metadata?: Record<string, unknown>;
}

const DEVICE_ID_KEY = 'selfhosted_device_id';
const CREDENTIALS_KEY = 'selfhosted_credentials';
// Pre-password clients persisted the bare secret under this key. Kept as a
// read-only fallback: an updated client can still refresh a session created
// by an older one, because the server accepts a password without an email as
// the legacy code.
const LEGACY_LOGIN_CODE_KEY = 'selfhosted_login_code';

export interface SavedCredentials {
  email: string;
  password: string;
}

/**
 * Persist the account credentials so an expired JWT can be refreshed without
 * re-prompting the user. This mirrors the official Supabase path persisting
 * `refresh_token` in localStorage: the password is as sensitive as the token
 * itself, so treat the whole localStorage payload as the credential.
 */
export function saveCredentials(email: string, password: string): void {
  if (typeof window === 'undefined') return;
  localStorage.setItem(CREDENTIALS_KEY, JSON.stringify({ email, password }));
}

/** Saved credentials, falling back to the pre-password login-code key. */
export function getSavedCredentials(): SavedCredentials | null {
  if (typeof window === 'undefined') return null;
  try {
    const raw = localStorage.getItem(CREDENTIALS_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<SavedCredentials>;
      if (parsed && typeof parsed.password === 'string') {
        return {
          email: typeof parsed.email === 'string' ? parsed.email : '',
          password: parsed.password,
        };
      }
    }
  } catch {
    // Corrupted payload: fall through to the legacy key.
  }
  const legacy = localStorage.getItem(LEGACY_LOGIN_CODE_KEY);
  return legacy ? { email: '', password: legacy } : null;
}

export function clearCredentials(): void {
  if (typeof window === 'undefined') return;
  localStorage.removeItem(CREDENTIALS_KEY);
  localStorage.removeItem(LEGACY_LOGIN_CODE_KEY);
}

/**
 * Self-hosted server base URL. Read lazily at call time from the runtime store
 * (set from the login form) so a pre-built installer works without per-user
 * rebuilds. In self-hosted mode this MUST point at the user's own sync server —
 * it never falls back to the official `web.readest.com`. If unset the login
 * call fails loudly instead of leaking the shared code to a third-party domain.
 */
const getSelfhostedBaseUrl = (): string => getSelfhostedServerUrl();

/**
 * A stable per-installation device id, persisted in localStorage. Sent to the
 * sync server as `X-Device-Id` so brute-force locks are bound to the device
 * rather than a shared NAT IP.
 */
export function getDeviceId(): string {
  if (typeof window === 'undefined') return '';
  let id = localStorage.getItem(DEVICE_ID_KEY);
  if (!id) {
    id =
      typeof crypto !== 'undefined' && 'randomUUID' in crypto
        ? crypto.randomUUID()
        : ((crypto as Crypto | undefined)?.randomUUID?.() ?? '');
    localStorage.setItem(DEVICE_ID_KEY, id);
  }
  return id;
}

/**
 * Login against a self-hosted Readest sync server with an account (email) +
 * password. A passwordless account (server-side created with an empty
 * password) signs in with an empty password. The server returns a JWT
 * (`plan: pro`, `sub: owner`).
 */
export type SelfhostedLoginErrorReason =
  | 'no-url' // 未配置服务端地址
  | 'network' // 连不上服务端（DNS/连接被拒/超时/CORS）
  | 'server' // 服务端 5xx 内部错误
  | 'locked' // 429 设备被限流锁定
  | 'invalid-credentials'; // 服务端明确拒绝：账号或密码错误

export class SelfhostedLoginError extends Error {
  reason: SelfhostedLoginErrorReason;
  status?: number;
  lockedUntil?: number;
  constructor(
    message: string,
    reason: SelfhostedLoginErrorReason,
    extra?: { status?: number; lockedUntil?: number },
  ) {
    super(message);
    this.name = 'SelfhostedLoginError';
    this.reason = reason;
    this.status = extra?.status;
    this.lockedUntil = extra?.lockedUntil;
  }
}

export async function selfhostedLogin(
  email: string,
  password: string,
): Promise<SelfhostedLoginResult> {
  const baseUrl = getSelfhostedBaseUrl();
  if (!baseUrl) {
    throw new SelfhostedLoginError('Self-hosted server URL is not configured', 'no-url');
  }
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  const deviceId = getDeviceId();
  if (deviceId) headers['X-Device-Id'] = deviceId;
  // The sync server registers its API surface under /api (see main.go); the
  // other clients (sync/storage/translate) already use ${base}/api via
  // getAPIBaseUrl, so the login endpoint must live on the same prefix.
  let res: Response;
  try {
    res = await fetch(`${baseUrl}/api/auth`, {
      method: 'POST',
      headers,
      body: JSON.stringify({ email, password }),
    });
  } catch {
    // Network-level failure: DNS resolution, connection refused, timeout, or a
    // CORS/pre-flight rejection all surface here as a rejected promise rather
    // than a non-2xx response. Previously these were silently folded into
    // "invalid code", making it impossible to tell a bad password from a dead
    // server.
    throw new SelfhostedLoginError(`Cannot reach the sync server at ${baseUrl}`, 'network');
  }
  if (!res.ok) {
    const status = res.status;
    // 429 = device locked after too many failures; surface the retry delay.
    if (status === 429) {
      const raw = res.headers.get('Retry-After') ?? '';
      const retryAfter = Number(raw);
      const lockedUntil =
        !Number.isNaN(retryAfter) && retryAfter > 0
          ? Date.now() + retryAfter * 1000
          : !Number.isNaN(Date.parse(raw))
            ? Date.parse(raw)
            : undefined;
      throw new SelfhostedLoginError('Too many failed attempts', 'locked', {
        status,
        lockedUntil,
      });
    }
    // 5xx = server-side error, not a credential problem.
    if (status >= 500) {
      throw new SelfhostedLoginError(
        `Sync server returned an internal error (${status})`,
        'server',
        { status },
      );
    }
    // Any other non-2xx (400/401/etc.) means the server rejected the
    // credentials.
    throw new SelfhostedLoginError('invalid credentials', 'invalid-credentials', { status });
  }
  // Read the body as text first so a non-JSON response (HTML error page,
  // nginx default page, redirect body, stray prefix/BOM) doesn't surface as a
  // raw `SyntaxError: Unexpected non-whitespace character after JSON` with no
  // actionable context. A 2xx with an unparseable body means the server
  // contract changed or a proxy intercepted the response — report it clearly
  // instead of letting the raw exception leak to the console.
  const text = await res.text();
  let parsed: SelfhostedLoginResult;
  try {
    parsed = JSON.parse(text) as SelfhostedLoginResult;
  } catch {
    throw new SelfhostedLoginError(
      `Sync server returned a non-JSON response (HTTP ${res.status}); ` +
        `check the server URL and that ${baseUrl}/api/auth is reachable`,
      'server',
      { status: res.status },
    );
  }
  if (!parsed || typeof parsed.access_token !== 'string') {
    throw new SelfhostedLoginError(
      `Sync server response is missing 'access_token' (HTTP ${res.status})`,
      'server',
      { status: res.status },
    );
  }
  return parsed;
}

/**
 * Decode a Base64URL JWT payload (the middle segment) into an object, with
 * UTF-8 safe handling so non-ASCII claims don't corrupt via `atob`.
 */
function decodeJwtPayload(token: string | null | undefined): Record<string, unknown> | null {
  if (!token) return null;
  try {
    const parts = token.split('.');
    if (parts.length < 2) return null;
    const b64 = parts[1];
    if (!b64) return null;
    // JWT payload 使用 Base64URL，转换为标准 Base64 以兼容 atob
    const normalized = b64.replace(/-/g, '+').replace(/_/g, '/');
    const padded = normalized.padEnd(normalized.length + ((4 - (normalized.length % 4)) % 4), '=');
    const binary = atob(padded);
    const json =
      typeof TextDecoder !== 'undefined'
        ? new TextDecoder().decode(Uint8Array.from(binary, (c) => c.charCodeAt(0)))
        : decodeURIComponent(escape(binary));
    return JSON.parse(json) as Record<string, unknown>;
  } catch {
    return null;
  }
}

/**
 * Derive a minimal `User`-like object from a self-hosted JWT.
 *
 * SECURITY: client-side decoding is for UI/identity display only. Never use the
 * decoded `sub` or `plan` as a trust boundary — the self-hosted sync server
 * enforces authorization server-side in `middleware.RequireAuth` (HS256 verify).
 * Any client-side feature gate must be treated as cosmetic; the server is the
 * source of truth for access control.
 */
export function jwtToUser(token: string | null | undefined): SelfhostedUser | null {
  const payload = decodeJwtPayload(token);
  const sub = typeof payload?.['sub'] === 'string' ? payload['sub'] : null;
  if (!sub) return null;
  const email = typeof payload?.['email'] === 'string' ? payload['email'] : undefined;
  return { id: sub, email };
}

/**
 * Authorization role carried in the self-hosted JWT (`'admin'` | `'user'`).
 * Tokens issued before the role model have no claim — treated as `'user'`.
 *
 * SECURITY: see {@link jwtToUser} — client-side decoding gates UI entry only;
 * the server enforces the admin role on every /api/admin request.
 */
export function jwtRole(token: string | null | undefined): 'admin' | 'user' | null {
  if (!token) return null;
  const payload = decodeJwtPayload(token);
  return payload?.['role'] === 'admin' ? 'admin' : 'user';
}

/**
 * Parse the `sub` claim from a JWT payload WITHOUT verifying the signature.
 * Thin wrapper over {@link jwtToUser} kept for callers that only need the id.
 *
 * SECURITY: see {@link jwtToUser} — client-side decoding is display-only.
 */
export function jwtSub(token: string | null | undefined): string | null {
  return jwtToUser(token)?.id ?? null;
}

/**
 * Whether a self-hosted JWT is missing or already past its `exp` claim.
 * Used by the auth refresh path to decide whether to re-login with the
 * persisted login code. A token without an `exp` claim is treated as
 * expired so the client re-validates against the server.
 */
export function isTokenExpired(token: string | null | undefined): boolean {
  const payload = decodeJwtPayload(token);
  const exp = typeof payload?.['exp'] === 'number' ? payload['exp'] : null;
  if (exp === null) return true;
  return Date.now() >= exp * 1000;
}

/**
 * The self-hosted counterpart of Supabase's session refresh, called from
 * AuthContext when the Supabase client is absent: the JWT is short-lived, so
 * once the current token has expired, re-login with the persisted
 * credentials to keep the user signed in. If they are no longer valid the
 * next API call will 401 and surface a re-login; the stale token is kept
 * until then (callers fall back to the login screen on 401).
 */
export async function refreshSelfhostedSession(
  login: (token: string, user: SelfhostedUser) => void,
): Promise<void> {
  const storedToken = localStorage.getItem('token');
  if (storedToken && !isTokenExpired(storedToken)) return;
  const credentials = getSavedCredentials();
  if (!credentials) return;
  try {
    const { access_token } = await selfhostedLogin(credentials.email, credentials.password);
    const user = jwtToUser(access_token);
    if (user) login(access_token, user);
  } catch {
    // Keep the stale token; callers fall back to the login screen on 401.
  }
}
