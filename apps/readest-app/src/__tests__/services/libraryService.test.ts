import { describe, test, expect, vi, afterEach } from 'vitest';
import { BaseDir, FileSystem } from '@/types/system';
import { Book } from '@/types/book';
import { getLibraryFilename } from '@/utils/book';

// A minimal in-memory Books/ tree: exactly the surface libraryService's
// safeLoadJSON/safeSaveJSON round-trip touches.
const makeFs = () => {
  const files = new Map<string, string>();
  const key = (path: string, base: BaseDir) => `${base}/${path}`;
  const fs = {
    exists: async (path: string, base: BaseDir) => files.has(key(path, base)),
    createDir: async () => {},
    readFile: async (path: string, base: BaseDir) => {
      const value = files.get(key(path, base));
      if (value === undefined) throw new Error('not found');
      return value;
    },
    writeFile: async (path: string, base: BaseDir, content: string) => {
      files.set(key(path, base), content);
    },
  } as unknown as FileSystem;
  return { fs, files };
};

const book = (hash: string): Book => ({ hash, title: `Book ${hash}` }) as Book;

describe('getLibraryFilename scoping', () => {
  test('user id maps to library_<uid>.json; guests and legacy callers keep library.json', () => {
    expect(getLibraryFilename('uid-a')).toBe('library_uid-a.json');
    expect(getLibraryFilename(null)).toBe('library.json');
    expect(getLibraryFilename()).toBe('library.json');
  });
});

describe('libraryService account scoping', () => {
  const cover = async () => 'cover-url';

  test('each account loads and saves its own shelf file', async () => {
    const { loadLibraryBooks, saveLibraryBooks } = await import('@/services/libraryService');
    const { fs } = makeFs();

    await saveLibraryBooks(fs, [book('a1')], undefined, 'uid-a');
    await saveLibraryBooks(fs, [book('b1')], undefined, 'uid-b');
    await saveLibraryBooks(fs, [book('g1')], undefined, null);

    expect((await loadLibraryBooks(fs, cover, 'uid-a')).map((b) => b.hash)).toEqual(['a1']);
    expect((await loadLibraryBooks(fs, cover, 'uid-b')).map((b) => b.hash)).toEqual(['b1']);
    expect((await loadLibraryBooks(fs, cover, null)).map((b) => b.hash)).toEqual(['g1']);
  });

  test('the merge floor applies per scope, not across shelves', async () => {
    const { loadLibraryBooks, saveLibraryBooks } = await import('@/services/libraryService');
    const { fs } = makeFs();

    await saveLibraryBooks(fs, [book('a1')], undefined, 'uid-a');
    await saveLibraryBooks(fs, [book('a2')], undefined, 'uid-a');
    await saveLibraryBooks(fs, [book('g1')], undefined, null);

    const shelfA = await loadLibraryBooks(fs, cover, 'uid-a');
    expect(shelfA.map((b) => b.hash).sort()).toEqual(['a1', 'a2']);
    expect((await loadLibraryBooks(fs, cover, null)).map((b) => b.hash)).toEqual(['g1']);
  });

  test('replace shrinks only the addressed scope', async () => {
    const { loadLibraryBooks, saveLibraryBooks } = await import('@/services/libraryService');
    const { fs } = makeFs();

    await saveLibraryBooks(fs, [book('a1'), book('a2')], undefined, 'uid-a');
    await saveLibraryBooks(fs, [book('a1')], { replace: true }, 'uid-a');

    expect((await loadLibraryBooks(fs, cover, 'uid-a')).map((b) => b.hash)).toEqual(['a1']);
  });
});

describe('shelf-scope tracker', () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
    localStorage.clear();
  });

  test('outside self-hosted mode the gate is always open', async () => {
    const { setShelfScope, isShelfScopeCurrent } = await import('@/services/libraryService');
    setShelfScope('uid-a');
    expect(isShelfScopeCurrent()).toBe(true);
  });

  test('in self-hosted mode the shelf must match the signed-in account', async () => {
    vi.stubEnv('NEXT_PUBLIC_SELFHOSTED', '1');
    vi.resetModules();
    const { setShelfScope, isShelfScopeCurrent } = await import('@/services/libraryService');

    const token = `x.${btoa(JSON.stringify({ sub: 'uid-a' }))}.sig`;
    localStorage.setItem('token', token);

    // No shelf read yet (undefined scope) — treat as stale, syncs must wait.
    expect(isShelfScopeCurrent()).toBe(false);

    setShelfScope('uid-a');
    expect(isShelfScopeCurrent()).toBe(true);

    // The account flipped (login as B) but the shelf was not re-read yet.
    localStorage.setItem('token', `x.${btoa(JSON.stringify({ sub: 'uid-b' }))}.sig`);
    expect(isShelfScopeCurrent()).toBe(false);

    setShelfScope('uid-b');
    expect(isShelfScopeCurrent()).toBe(true);

    // Logged out: only the guest shelf counts as current.
    localStorage.removeItem('token');
    expect(isShelfScopeCurrent()).toBe(false);
    setShelfScope(null);
    expect(isShelfScopeCurrent()).toBe(true);
  });
});
