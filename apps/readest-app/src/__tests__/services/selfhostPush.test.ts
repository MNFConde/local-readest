/**
 * selfhostPush — wire contract for /api/pushes/* (⑬): the regular-user
 * book-push client. Asserts URL/method/headers/body and the error-surface
 * mapping; the server side is covered by sync-server contract tests.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const BASE = 'http://127.0.0.1:8080';
const TOKEN = 'jwt-token-1';

function jsonResponse(status: number, body: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: new Headers(),
    text: async () => (body === undefined ? '' : JSON.stringify(body)),
  };
}

describe('selfhostPush', () => {
  beforeEach(() => {
    localStorage.setItem('token', TOKEN);
    localStorage.setItem('selfhosted_server_url', BASE);
  });
  afterEach(() => {
    localStorage.clear();
    vi.unstubAllGlobals();
    vi.resetModules();
  });

  it('createPush POSTs JSON with Bearer to /api/pushes', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(200, { pushed: true, id: 7 }));
    vi.stubGlobal('fetch', fetchMock);

    const { createPush } = await import('@/services/selfhostPush');
    const result = await createPush({ to: 'bob@x.com', bookHash: 'h1', message: 'hi' });

    expect(fetchMock).toHaveBeenCalledWith(`${BASE}/api/pushes`, expect.anything());
    const init = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(init.method).toBe('POST');
    const headers = init.headers as Record<string, string>;
    expect(headers['Authorization']).toBe(`Bearer ${TOKEN}`);
    expect(headers['Content-Type']).toBe('application/json');
    expect(JSON.parse(init.body as string)).toEqual({
      to: 'bob@x.com',
      bookHash: 'h1',
      message: 'hi',
    });
    expect(result.id).toBe(7);
  });

  it('listInbox GETs /api/pushes/inbox and returns the items array', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      jsonResponse(200, {
        items: [
          {
            id: 3,
            from: 'alice@x.com',
            bookHash: 'h9',
            title: 'A Book',
            author: null,
            message: '',
            createdAt: '2026-11-18T00:00:00Z',
          },
        ],
      }),
    );
    vi.stubGlobal('fetch', fetchMock);

    const { listInbox } = await import('@/services/selfhostPush');
    const items = await listInbox();

    expect(fetchMock).toHaveBeenCalledWith(`${BASE}/api/pushes/inbox`, expect.anything());
    const init = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(init.method).toBe('GET');
    expect(items).toHaveLength(1);
    expect(items[0]?.from).toBe('alice@x.com');
  });

  it('listInbox tolerates a missing items field', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse(200, {})));
    const { listInbox } = await import('@/services/selfhostPush');
    await expect(listInbox()).resolves.toEqual([]);
  });

  it('acceptPush and declinePush POST the decision endpoints', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(200, { accepted: true, filesLinked: 2, filesSkipped: 0 }))
      .mockResolvedValueOnce(jsonResponse(200, { declined: 5 }));
    vi.stubGlobal('fetch', fetchMock);

    const { acceptPush, declinePush } = await import('@/services/selfhostPush');
    const accepted = await acceptPush(5);
    const declined = await declinePush(5);

    expect(accepted.filesLinked).toBe(2);
    expect(declined.declined).toBe(5);
    expect(fetchMock.mock.calls[0]?.[0]).toBe(`${BASE}/api/pushes/5/accept`);
    expect(fetchMock.mock.calls[1]?.[0]).toBe(`${BASE}/api/pushes/5/decline`);
    const init = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(init.method).toBe('POST');
  });

  it('surfaces the server error body on a rejected push', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse(404, { error: 'no such user' })));
    const { createPush, SelfhostPushError } = await import('@/services/selfhostPush');
    await expect(createPush({ to: 'ghost@x.com', bookHash: 'h1' })).rejects.toThrow(
      SelfhostPushError,
    );
    await expect(createPush({ to: 'ghost@x.com', bookHash: 'h1' })).rejects.toThrow('no such user');
  });

  it('refuses to fetch without a stored token', async () => {
    localStorage.removeItem('token');
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    const { listInbox } = await import('@/services/selfhostPush');
    await expect(listInbox()).rejects.toThrow('Not signed in');
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
