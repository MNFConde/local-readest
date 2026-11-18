/**
 * connections — ⑪-P2 remembered multi-server connections. Contract: every
 * successful login upserts one {server|account} record and moves the active
 * pointer; re-login keeps the record's transfer mode; a pointer change means
 * "switched" (callers then reset cursors + persisted queue); the metadata
 * flag answers only for the active connection; the switch helpers do the
 * settings.json cursor zeroing and queue-key removal; the toast helpers go
 * out as info toasts through eventDispatcher.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const { getAppService, dispatch, t } = vi.hoisted(() => ({
  getAppService: vi.fn(),
  dispatch: vi.fn(),
  t: vi.fn(),
}));

vi.mock('@/services/environment', () => ({ default: { getAppService } }));
vi.mock('@/utils/event', () => ({ eventDispatcher: { dispatch } }));
vi.mock('@/i18n/i18n', () => ({ default: { t } }));

import {
  clearPersistedTransferQueue,
  connectionId,
  getActiveConnection,
  getActiveConnectionId,
  isMetadataOnlyConnection,
  listConnections,
  reloadForConnectionSwitch,
  removeConnection,
  resetSyncCursorsForSwitch,
  setConnectionMode,
  toastMetadataDownloadRefused,
  toastMetadataUploadRefused,
  upsertConnectionOnLogin,
} from '@/libs/selfhost/connections';

describe('connections (⑪-P2)', () => {
  beforeEach(() => {
    localStorage.clear();
    getAppService.mockReset();
    dispatch.mockReset();
    t.mockReset().mockImplementation((key: string, opts?: { title?: string }) =>
      opts?.title ? `${key}|${opts.title}` : key,
    );
  });
  afterEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('normalizes server urls and lowercases the email part of the id', () => {
    expect(connectionId('127.0.0.1:8080', 'Alice@Home')).toBe('http://127.0.0.1:8080|alice@home');
    expect(connectionId('http://10.0.0.2:8080/', 'alice@home')).toBe(
      connectionId('10.0.0.2:8080', 'ALICE@home'),
    );
  });

  it('first login records a full-mode connection and reports switched', () => {
    const { connection, switched } = upsertConnectionOnLogin('10.0.0.2:8080', 'alice@home', 'pw');
    expect(switched).toBe(true); // no previous active pointer
    expect(connection.mode).toBe('full');
    expect(getActiveConnectionId()).toBe(connection.id);
    expect(listConnections()).toHaveLength(1);
  });

  it('re-login to the same connection refreshes the password, keeps mode, no switch', () => {
    const first = upsertConnectionOnLogin('10.0.0.2:8080', 'alice@home', 'pw');
    setConnectionMode(first.connection.id, 'metadata');
    const second = upsertConnectionOnLogin('http://10.0.0.2:8080/', 'Alice@Home', 'pw2');

    expect(second.switched).toBe(false);
    expect(second.connection.id).toBe(first.connection.id);
    expect(second.connection.password).toBe('pw2');
    expect(second.connection.mode).toBe('metadata');
    expect(listConnections()).toHaveLength(1);
  });

  it('logging into another connection switches the active pointer', () => {
    const a = upsertConnectionOnLogin('http://a:8080', 'alice@a', 'pw-a');
    const b = upsertConnectionOnLogin('http://b:8080', 'bob@b', 'pw-b');

    expect(b.switched).toBe(true);
    expect(getActiveConnectionId()).toBe(b.connection.id);
    expect(getActiveConnection()?.email).toBe('bob@b');
    expect(listConnections().map((c) => c.id)).toEqual([a.connection.id, b.connection.id]);
  });

  it('removeConnection drops the record and clears an active pointer', () => {
    const a = upsertConnectionOnLogin('http://a:8080', 'alice@a', 'pw-a');
    upsertConnectionOnLogin('http://b:8080', 'bob@b', 'pw-b');

    removeConnection(a.connection.id);
    expect(listConnections()).toHaveLength(1);
    expect(getActiveConnectionId()).not.toBe(a.connection.id); // was not active

    removeConnection(getActiveConnectionId());
    expect(listConnections()).toHaveLength(0);
    expect(getActiveConnectionId()).toBe('');
    expect(getActiveConnection()).toBeNull();
  });

  it('metadata mode answers only for the active connection', () => {
    const a = upsertConnectionOnLogin('http://a:8080', 'alice@a', 'pw-a');
    upsertConnectionOnLogin('http://b:8080', 'bob@b', 'pw-b');
    setConnectionMode(a.connection.id, 'metadata');

    expect(isMetadataOnlyConnection()).toBe(false); // active is bob (full)
    setConnectionMode(getActiveConnectionId(), 'metadata');
    expect(isMetadataOnlyConnection()).toBe(true);

    removeConnection(getActiveConnectionId());
    expect(isMetadataOnlyConnection()).toBe(false); // no active connection
  });

  it('resetSyncCursorsForSwitch zeroes the device-global cursors on disk', async () => {
    const loadSettings = vi.fn().mockResolvedValue({
      lastSyncedAtBooks: 1234,
      lastSyncedAtConfigs: 2345,
      lastSyncedAtNotes: 3456,
      lastSyncedAtReplicas: { bookshelf: 'HLC' },
      theme: 'dark',
    });
    const saveSettings = vi.fn().mockResolvedValue(undefined);
    getAppService.mockResolvedValue({ loadSettings, saveSettings });

    await resetSyncCursorsForSwitch();

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0]![0];
    expect(saved.lastSyncedAtBooks).toBe(0);
    expect(saved.lastSyncedAtConfigs).toBe(0);
    expect(saved.lastSyncedAtNotes).toBe(0);
    expect(saved.lastSyncedAtReplicas).toEqual({});
    expect(saved.theme).toBe('dark'); // the rest of settings.json is untouched
  });

  it('resetSyncCursorsForSwitch swallows an unavailable app service', async () => {
    getAppService.mockRejectedValue(new Error('no service'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(resetSyncCursorsForSwitch()).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalled();
  });

  it('clearPersistedTransferQueue removes the queue key', () => {
    localStorage.setItem('readest_transfer_queue', '{"transfers":{}}');
    clearPersistedTransferQueue();
    expect(localStorage.getItem('readest_transfer_queue')).toBeNull();
  });

  it('toast helpers dispatch info toasts with the selfhost-ns key', () => {
    toastMetadataUploadRefused('Book A');
    expect(dispatch).toHaveBeenCalledWith('toast', {
      type: 'info',
      timeout: 5000,
      message: 'Metadata-only connection skips book file uploads ({{title}})|Book A',
    });

    toastMetadataDownloadRefused('Book B');
    expect(dispatch).toHaveBeenCalledWith('toast', {
      type: 'info',
      timeout: 5000,
      message: 'Metadata-only connection skips book file downloads ({{title}})|Book B',
    });
  });

  it('reloadForConnectionSwitch lands on the redirect target or /library', () => {
    const replace = vi.fn();
    vi.stubGlobal('window', { location: { replace } });

    reloadForConnectionSwitch('/library');
    expect(replace).toHaveBeenLastCalledWith('/library');
    reloadForConnectionSwitch(null);
    expect(replace).toHaveBeenLastCalledWith('/library');
    reloadForConnectionSwitch('https://evil.example.com');
    expect(replace).toHaveBeenLastCalledWith('/library'); // non-local redirect ignored
    reloadForConnectionSwitch('/reader/some-book');
    expect(replace).toHaveBeenLastCalledWith('/reader/some-book');
  });
});
