/**
 * selfhostArchive — wire contract for /api/archive/* (⑫): the regular-user
 * archive export/import client. Asserts URL/method/headers/body and the
 * error-surface mapping; the server side is covered by sync-server contract
 * tests.
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

describe('selfhostArchive', () => {
  let anchorClicks: string[];

  beforeEach(() => {
    localStorage.setItem('token', TOKEN);
    localStorage.setItem('selfhosted_server_url', BASE);
    anchorClicks = [];
    vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:archive');
    vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      anchorClicks.push(this.download);
    });
  });
  afterEach(() => {
    localStorage.clear();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    vi.resetModules();
  });

  it('exportArchive GETs /api/archive/export with Bearer and saves via anchor', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      headers: new Headers({
        'Content-Disposition': 'attachment; filename="readest-archive-a-20261118-101010.tar"',
      }),
      blob: async () => new Blob(['tar-bytes']),
    });
    vi.stubGlobal('fetch', fetchMock);

    const { exportArchive } = await import('@/services/selfhostArchive');
    const name = await exportArchive('metadata');

    expect(fetchMock).toHaveBeenCalledWith(
      `${BASE}/api/archive/export?scope=metadata`,
      expect.anything(),
    );
    const init = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect((init.headers as Record<string, string>)['Authorization']).toBe(`Bearer ${TOKEN}`);
    expect(name).toBe('readest-archive-a-20261118-101010.tar');
    expect(anchorClicks).toEqual(['readest-archive-a-20261118-101010.tar']);
  });

  it('importArchive POSTs the raw tar body and returns the summary', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse(200, { books: 3, filesLinked: 2, blobsStored: 1 }));
    vi.stubGlobal('fetch', fetchMock);

    const { importArchive } = await import('@/services/selfhostArchive');
    const file = new File(['tar-bytes'], 'readest-archive.tar', { type: 'application/x-tar' });
    const summary = await importArchive(file);

    expect(fetchMock).toHaveBeenCalledWith(`${BASE}/api/archive/import`, expect.anything());
    const init = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(init.method).toBe('POST');
    expect((init.headers as Record<string, string>)['Content-Type']).toBe('application/x-tar');
    expect((init.headers as Record<string, string>)['Authorization']).toBe(`Bearer ${TOKEN}`);
    expect(init.body).toBe(file);
    expect(summary.books).toBe(3);
  });

  it('surfaces the server error body on a rejected import', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse(400, { error: 'missing manifest.json' })),
    );
    const { importArchive, SelfhostArchiveError } = await import('@/services/selfhostArchive');
    const file = new File(['x'], 'a.tar');
    await expect(importArchive(file)).rejects.toThrow(SelfhostArchiveError);
    await expect(importArchive(file)).rejects.toThrow('missing manifest.json');
  });

  it('refuses to fetch without a stored token', async () => {
    localStorage.removeItem('token');
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    const { exportArchive } = await import('@/services/selfhostArchive');
    await expect(exportArchive('full')).rejects.toThrow('Not signed in');
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
