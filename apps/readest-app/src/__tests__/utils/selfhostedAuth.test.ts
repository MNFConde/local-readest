import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// 模拟 JWT（HS256 不验签，只 decode）：sub=owner, plan=pro
const b64url = (s: string) => Buffer.from(s, 'utf-8').toString('base64');
const fakeJwt =
  b64url('{"alg":"HS256","typ":"JWT"}') +
  '.' +
  b64url(JSON.stringify({ sub: 'owner', plan: 'pro', exp: 9999999999 })) +
  '.sig';

// The functions below branch on `SELFHOSTED`, which is a module-level constant
// derived from `NEXT_PUBLIC_SELFHOSTED`. We stub that env var and reset the
// module cache so `supabase.ts` / `access.ts` re-evaluate in self-hosted mode.
async function importAccessInSelfhosted() {
  vi.stubEnv('NEXT_PUBLIC_SELFHOSTED', '1');
  vi.resetModules();
  return import('@/utils/access');
}

describe('selfhosted auth', () => {
  beforeEach(() => {
    localStorage.setItem('token', fakeJwt);
  });
  afterEach(() => {
    localStorage.clear();
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it('getAccessToken returns stored token', async () => {
    const { getAccessToken } = await importAccessInSelfhosted();
    expect(await getAccessToken()).toBe(fakeJwt);
  });

  it('getUserID derives sub from JWT without supabase', async () => {
    const { getUserID } = await importAccessInSelfhosted();
    expect(await getUserID()).toBe('owner');
  });

  it('getSubscriptionPlan reads plan=pro from token (no quota limit)', async () => {
    const { getSubscriptionPlan } = await importAccessInSelfhosted();
    expect(getSubscriptionPlan(fakeJwt)).toBe('pro');
  });

  it('jwtSub parses sub from a self-hosted JWT', async () => {
    const { jwtSub } = await import('@/services/selfhostedAuth');
    expect(jwtSub(fakeJwt)).toBe('owner');
    expect(jwtSub(null)).toBeNull();
    expect(jwtSub('invalid')).toBeNull();
  });

  it('jwtToUser derives id (and email when present) from a self-hosted JWT', async () => {
    const { jwtToUser } = await import('@/services/selfhostedAuth');
    expect(jwtToUser(fakeJwt)).toEqual({ id: 'owner', email: undefined });
    // non-ASCII (UTF-8) claim must decode correctly, not return null
    const b64urlUtf8 = (s: string) => Buffer.from(s, 'utf-8').toString('base64');
    const utf8Jwt =
      b64urlUtf8('{"alg":"HS256","typ":"JWT"}') +
      '.' +
      b64urlUtf8(JSON.stringify({ sub: 'owner', email: '用户@example.com', exp: 9999999999 })) +
      '.sig';
    expect(jwtToUser(utf8Jwt)).toEqual({ id: 'owner', email: '用户@example.com' });
    expect(jwtToUser(null)).toBeNull();
    expect(jwtToUser('invalid')).toBeNull();
  });

  it('saveCredentials round-trips (empty password = valid passwordless credential)', async () => {
    const { saveCredentials, getSavedCredentials, clearCredentials } = await import(
      '@/services/selfhostedAuth'
    );
    saveCredentials('alice@example.com', 'pw');
    expect(getSavedCredentials()).toEqual({ email: 'alice@example.com', password: 'pw' });
    saveCredentials('free@example.com', '');
    expect(getSavedCredentials()).toEqual({ email: 'free@example.com', password: '' });
    clearCredentials();
    expect(getSavedCredentials()).toBeNull();
  });

  it('getSavedCredentials falls back to the legacy login-code key (zero migration)', async () => {
    const { getSavedCredentials } = await import('@/services/selfhostedAuth');
    localStorage.setItem('selfhosted_login_code', 'legacy-code');
    expect(getSavedCredentials()).toEqual({ email: '', password: 'legacy-code' });
  });

  it('selfhostedLogin posts {email,password} to <base>/api/auth', async () => {
    localStorage.setItem('selfhosted_server_url', 'http://127.0.0.1:8080');
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      headers: new Headers(),
      text: async () => JSON.stringify({ access_token: fakeJwt, token_type: 'bearer' }),
    });
    vi.stubGlobal('fetch', fetchMock);
    try {
      const { selfhostedLogin } = await import('@/services/selfhostedAuth');
      await selfhostedLogin('alice@example.com', 'pw');
      expect(fetchMock).toHaveBeenCalledWith(
        'http://127.0.0.1:8080/api/auth',
        expect.objectContaining({
          method: 'POST',
          body: JSON.stringify({ email: 'alice@example.com', password: 'pw' }),
        }),
      );
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('selfhostedLogin maps a 401 to invalid-credentials', async () => {
    localStorage.setItem('selfhosted_server_url', 'http://127.0.0.1:8080');
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: false,
        status: 401,
        headers: new Headers(),
        text: async () => '{"error":"invalid code"}',
      }),
    );
    try {
      const { selfhostedLogin } = await import('@/services/selfhostedAuth');
      await expect(selfhostedLogin('alice@example.com', 'bad')).rejects.toMatchObject({
        reason: 'invalid-credentials',
      });
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
