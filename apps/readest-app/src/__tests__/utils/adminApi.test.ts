import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const b64url = (s: string) => Buffer.from(s, 'utf-8').toString('base64');
const adminJwt =
  b64url('{"alg":"HS256","typ":"JWT"}') +
  '.' +
  b64url(JSON.stringify({ sub: 'owner', plan: 'pro', role: 'admin', exp: 9999999999 })) +
  '.sig';

const BASE = 'http://127.0.0.1:8080';

function mockFetch(status: number, body: unknown) {
  const fetchMock = vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    headers: new Headers(),
    text: async () => (body === undefined ? '' : JSON.stringify(body)),
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

describe('adminApi', () => {
  beforeEach(() => {
    localStorage.setItem('token', adminJwt);
    localStorage.setItem('selfhosted_server_url', BASE);
  });
  afterEach(() => {
    localStorage.clear();
    vi.unstubAllGlobals();
    vi.resetModules();
  });

  it('listUsers GETs /api/admin/users with the Bearer token', async () => {
    const fetchMock = mockFetch(200, {
      users: [{ email: 'a@x', id: 'id1', disabled: false, createdAt: '1', role: 'admin' }],
    });
    const { adminApi } = await import('@/admin/adminApi');
    const users = await adminApi.listUsers();
    expect(users).toHaveLength(1);
    expect(users[0]?.role).toBe('admin');
    expect(fetchMock).toHaveBeenCalledWith(`${BASE}/api/admin/users`, expect.anything());
    const init = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect((init.headers as Record<string, string>)['Authorization']).toBe(`Bearer ${adminJwt}`);
  });

  it('createUser omits the password field when unset (server generates one)', async () => {
    const fetchMock = mockFetch(200, { email: 'n@x', password: 'rand', role: 'user' });
    const { adminApi } = await import('@/admin/adminApi');
    const created = await adminApi.createUser('n@x');
    expect(created.password).toBe('rand');
    const init = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(init.method).toBe('POST');
    expect(init.body).toBe(JSON.stringify({ email: 'n@x', role: 'user' }));
  });

  it('deleteUser and deleteBook address the encoded path with DELETE', async () => {
    const fetchMock = mockFetch(200, { deleted: 1 });
    const { adminApi } = await import('@/admin/adminApi');
    await adminApi.deleteUser('a@x.com');
    expect(fetchMock.mock.calls[0]?.[0]).toBe(`${BASE}/api/admin/users/a%40x.com`);
    expect((fetchMock.mock.calls[0]?.[1] as RequestInit).method).toBe('DELETE');
    await adminApi.deleteBook('abc123');
    expect(fetchMock.mock.calls[1]?.[0]).toBe(`${BASE}/api/admin/books/abc123`);
    expect((fetchMock.mock.calls[1]?.[1] as RequestInit).method).toBe('DELETE');
  });

  it('maps 401 to a session-expired error and 403 to admin-required', async () => {
    const { adminApi, AdminApiError } = await import('@/admin/adminApi');
    mockFetch(401, { error: 'x' });
    await expect(adminApi.listUsers()).rejects.toMatchObject({
      status: 401,
      message: 'Session expired — sign in again',
    });
    mockFetch(403, { error: 'x' });
    await expect(adminApi.listUsers()).rejects.toBeInstanceOf(AdminApiError);
    await expect(adminApi.listUsers()).rejects.toMatchObject({ status: 403 });
  });

  it('surfaces the server error body on other failures', async () => {
    mockFetch(409, { error: 'email already exists' });
    const { adminApi } = await import('@/admin/adminApi');
    await expect(adminApi.createUser('dup@x', undefined, 'user')).rejects.toMatchObject({
      status: 409,
      message: 'email already exists',
    });
  });

  it('throws without a stored token before any request', async () => {
    localStorage.removeItem('token');
    const fetchMock = mockFetch(200, {});
    const { adminApi } = await import('@/admin/adminApi');
    await expect(adminApi.listUsers()).rejects.toMatchObject({ status: 401 });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
