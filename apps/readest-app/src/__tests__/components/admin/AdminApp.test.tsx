/**
 * AdminApp — the users and books tabs' observability columns: per-user
 * storage + last-active in the users table, per-book holder counts with the
 * holders dialog, and the delete-book confirmation stating the impact
 * surface. The auth gate and API layer are mocked; the wire contract for the
 * new fields lives in the contract tests (sync-server) and adminApi.test.ts.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';

const {
  listUsers,
  listBooks,
  getBookHolders,
  deleteBook,
  getStats,
  downloadBackup,
  listSnapshots,
  createSnapshot,
  downloadSnapshot,
  deleteSnapshot,
} = vi.hoisted(() => ({
  listUsers: vi.fn(),
  listBooks: vi.fn(),
  getBookHolders: vi.fn(),
  deleteBook: vi.fn(),
  getStats: vi.fn(),
  downloadBackup: vi.fn(),
  listSnapshots: vi.fn(),
  createSnapshot: vi.fn(),
  downloadSnapshot: vi.fn(),
  deleteSnapshot: vi.fn(),
}));

vi.mock('@/hooks/useTranslation', () => {
  // One stable translator instance: AdminApp's run/refresh callbacks key off
  // it (useTranslation identity), so a per-call arrow would re-fire effects.
  const t = (s: string) => s;
  return { useTranslation: () => t };
});

vi.mock('@/services/selfhostedServerUrl', () => ({
  getSelfhostedServerUrl: () => 'http://127.0.0.1:8080',
}));

vi.mock('@/services/selfhostedAuth', () => ({
  jwtRole: () => 'admin',
  jwtToUser: () => ({ email: 'admin@t.test' }),
}));

vi.mock('@/admin/adminApi', () => ({
  AdminApiError: class extends Error {},
  adminApi: {
    listUsers,
    listBooks,
    getBookHolders,
    deleteBook,
    getStats,
    downloadBackup,
    listSnapshots,
    createSnapshot,
    downloadSnapshot,
    deleteSnapshot,
  },
}));

import AdminApp from '@/admin/AdminApp';

const users = [
  {
    email: 'a@t.test',
    id: 'uid-a',
    disabled: false,
    createdAt: '1700000000',
    role: 'user',
    bytesUsed: 2_500_000,
    lastSeenAt: 1_700_000_100,
  },
  {
    email: 'b@t.test',
    id: 'uid-b',
    disabled: false,
    createdAt: '1700000001',
    role: 'admin',
    bytesUsed: 0,
    lastSeenAt: null,
  },
];

const books = [
  {
    hash: 'hashAAA111',
    title: 'Shared Book',
    author: 'Someone',
    language: 'zh',
    workId: null,
    sourceHash: null,
    addedAt: '2026-01-01',
    addedBy: 'uid-a',
    holders: 2,
  },
  {
    hash: 'hashLONE222',
    title: 'Lone Book',
    author: '',
    language: '',
    workId: null,
    sourceHash: null,
    addedAt: '2026-01-02',
    addedBy: 'uid-b',
    holders: 0,
  },
];

const holders = [
  { userId: 'uid-a', email: 'a@t.test', deleted: false, syncedAt: '2026-01-01' },
  { userId: 'uid-b', email: 'b@t.test', deleted: true, syncedAt: '2026-01-02' },
];

describe('AdminApp users/books observability', () => {
  beforeEach(() => {
    listUsers.mockReset().mockResolvedValue(users);
    listBooks.mockReset().mockResolvedValue(books);
    getBookHolders.mockReset().mockResolvedValue(holders);
    deleteBook.mockReset().mockResolvedValue({ deleted: 1 });
    getStats.mockReset().mockResolvedValue(undefined);
    downloadBackup.mockReset().mockResolvedValue(undefined);
    listSnapshots
      .mockReset()
      .mockResolvedValue({ enabled: false, intervalHours: null, keep: 3, snapshots: [] });
    createSnapshot.mockReset();
    downloadSnapshot.mockReset().mockResolvedValue(undefined);
    deleteSnapshot.mockReset().mockResolvedValue({ deleted: 'x' });
    localStorage.setItem('token', 'header.payload.sig');
  });
  afterEach(() => {
    cleanup();
    localStorage.clear();
  });

  it('renders the users table with storage and last-active columns', async () => {
    render(<AdminApp />);
    fireEvent.click(screen.getByRole('tab', { name: 'Users' }));
    await waitFor(() => expect(screen.getByText('a@t.test')).toBeTruthy());
    expect(screen.getByText('Storage')).toBeTruthy();
    expect(screen.getByText('Last active')).toBeTruthy();
    // Storage via humanBytes; zero usage stays an em dash.
    expect(screen.getByText('2.4M')).toBeTruthy();
    expect(screen.getByText('—')).toBeTruthy();
    // A stamped user shows the local time; a never-active one shows Never.
    expect(screen.getByText(new Date(1_700_000_100 * 1000).toLocaleString())).toBeTruthy();
    expect(screen.getByText('Never')).toBeTruthy();
  });

  it('shows holder counts per book and opens the holders dialog', async () => {
    render(<AdminApp />);
    fireEvent.click(screen.getByRole('tab', { name: 'Books' }));
    await waitFor(() => expect(screen.getByText('Shared Book')).toBeTruthy());
    expect(screen.getByText('Holders')).toBeTruthy();
    // The count cells are buttons opening the dialog.
    fireEvent.click(screen.getByRole('button', { name: '2' }));
    await waitFor(() => expect(getBookHolders).toHaveBeenCalledWith('hashAAA111'));
    expect(screen.getByText('a@t.test')).toBeTruthy();
    expect(screen.getByText('b@t.test')).toBeTruthy();
    expect(screen.getByText('Active')).toBeTruthy();
    expect(screen.getByText('Deleted')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(screen.queryByText('(deleted account)')).toBeNull());
  });

  it('states the impact surface in the delete-book confirmation', async () => {
    render(<AdminApp />);
    fireEvent.click(screen.getByRole('tab', { name: 'Books' }));
    await waitFor(() => expect(screen.getByText('Shared Book')).toBeTruthy());
    // Two rows, two Delete buttons; delete the shared (held) book first.
    const [sharedDelete, loneDelete] = screen.getAllByRole('button', { name: 'Delete' });
    fireEvent.click(sharedDelete!);
    expect(
      screen.getByText(
        'Delete “{{title}}”? Held by {{count}} account(s) — every stored copy is removed.',
      ),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    // The unheld book gets the no-holders variant.
    fireEvent.click(loneDelete!);
    expect(
      screen.getByText(
        'Delete “{{title}}” from the shared library? No account currently holds it.',
      ),
    ).toBeTruthy();
  });

  it('downloads the backup from the Maintenance tab', async () => {
    render(<AdminApp />);
    fireEvent.click(screen.getByRole('tab', { name: 'Maintenance' }));
    expect(screen.getByText('Download backup')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Download backup' }));
    await waitFor(() => expect(downloadBackup).toHaveBeenCalledTimes(1));
    expect(await screen.findByText('Backup downloaded')).toBeTruthy();
  });

  it('surfaces a failed backup download as an error', async () => {
    downloadBackup.mockRejectedValue(
      Object.assign(new Error('Backup failed (HTTP 500)'), { status: 500 }),
    );
    render(<AdminApp />);
    fireEvent.click(screen.getByRole('tab', { name: 'Maintenance' }));
    fireEvent.click(screen.getByRole('button', { name: 'Download backup' }));
    await waitFor(() => expect(screen.getByText(/Backup failed/)).toBeTruthy());
  });

  // ── ⑫-b snapshot chain card ──────────────────────────────────────────────

  it('renders the snapshot chain state and creates a snapshot', async () => {
    listSnapshots.mockResolvedValue({
      enabled: true,
      intervalHours: 24,
      keep: 3,
      snapshots: [{ name: 'readest-snapshot-20261119-010101.tar', bytes: 1024 }],
    });
    createSnapshot.mockResolvedValue({ name: 'readest-snapshot-20261119-020202.tar', bytes: 2048 });
    render(<AdminApp />);
    fireEvent.click(screen.getByRole('tab', { name: 'Maintenance' }));
    await waitFor(() => expect(listSnapshots).toHaveBeenCalledTimes(1));
    expect(screen.getByText('Enabled: every {{hours}}h, keeping {{keep}}')).toBeTruthy();
    expect(screen.getByText('readest-snapshot-20261119-010101.tar')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Take snapshot now' }));
    await waitFor(() => expect(createSnapshot).toHaveBeenCalledTimes(1));
    // Raw-key translator: the notice shows the key; the list refreshes (call #2).
    expect(await screen.findByText('Snapshot {{name}} created')).toBeTruthy();
    await waitFor(() => expect(listSnapshots).toHaveBeenCalledTimes(2));
  });

  it('shows the disabled state and empty list when the chain is off', async () => {
    render(<AdminApp />);
    fireEvent.click(screen.getByRole('tab', { name: 'Maintenance' }));
    expect(screen.getByText('Disabled — set SNAPSHOT_INTERVAL_HOURS to enable')).toBeTruthy();
    expect(await screen.findByText('No snapshots yet')).toBeTruthy();
    expect(screen.queryByText('Take snapshot now')).toBeTruthy();
  });

  it('downloads and deletes a snapshot row', async () => {
    listSnapshots.mockResolvedValue({
      enabled: false,
      intervalHours: null,
      keep: 3,
      snapshots: [{ name: 'snap-a.tar', bytes: 512 }],
    });
    render(<AdminApp />);
    fireEvent.click(screen.getByRole('tab', { name: 'Maintenance' }));
    await screen.findByText('snap-a.tar');
    fireEvent.click(screen.getByRole('button', { name: 'Download' }));
    await waitFor(() => expect(downloadSnapshot).toHaveBeenCalledWith('snap-a.tar'));
    expect(await screen.findByText(/snap-a\.tar/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(deleteSnapshot).toHaveBeenCalledWith('snap-a.tar'));
    await waitFor(() => expect(listSnapshots).toHaveBeenCalledTimes(2));
  });
});
