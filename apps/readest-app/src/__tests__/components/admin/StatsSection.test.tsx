/**
 * StatsSection — the admin console's Overview tab, fed by the aggregate
 * /api/admin/stats payload.
 *
 * Contract under test: a full payload renders every card (counts, sizes,
 * dedup savings, version/uptime footnote), and API failures degrade to an
 * error notice instead of breaking the page.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';

const { getStats } = vi.hoisted(() => ({ getStats: vi.fn() }));

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (s: string) => s,
}));

vi.mock('@/services/selfhostedServerUrl', () => ({
  getSelfhostedServerUrl: () => 'http://127.0.0.1:8080',
}));

vi.mock('@/admin/adminApi', () => ({
  AdminApiError: class extends Error {},
  adminApi: { getStats },
}));

import StatsSection from '@/admin/StatsSection';

const fullStats = {
  users: { total: 3, admins: 1, disabled: 1 },
  books: { synced: 12, catalog: 5, works: 2 },
  storage: {
    files_bytes: 2_500_000,
    blobs_bytes: 1_500_000,
    blobs_count: 4,
    db_bytes: 81_920,
    dedup_saved_bytes: 1_000_000,
  },
  server: { version: '0.12.16', uptime_secs: 3600 },
};

describe('StatsSection', () => {
  beforeEach(() => {
    getStats.mockReset();
  });
  afterEach(() => {
    cleanup();
  });

  it('renders every card from a full stats payload', async () => {
    getStats.mockResolvedValue(fullStats);
    render(<StatsSection />);
    await waitFor(() => expect(screen.getByText('3')).toBeTruthy());
    // Counts.
    expect(screen.getByText('Users')).toBeTruthy();
    expect(screen.getByText('{{admins}} admin · {{disabled}} disabled')).toBeTruthy();
    expect(screen.getByText('12')).toBeTruthy(); // synced books
    expect(screen.getByText('5')).toBeTruthy(); // catalog books
    expect(screen.getByText('{{works}} works')).toBeTruthy();
    // Sizes via humanBytes formatting.
    expect(screen.getByText('2.4M')).toBeTruthy(); // stored files
    expect(screen.getByText('1.4M')).toBeTruthy(); // blob store
    expect(screen.getByText('{{count}} blobs')).toBeTruthy();
    expect(screen.getByText('976.6K')).toBeTruthy(); // dedup savings
    // Server footnote drawn from the same payload.
    expect(screen.getByText(/v\{\{version\}\} · up \{\{uptime\}\} · database/)).toBeTruthy();
  });

  it('shows zeros as em dashes instead of fake numbers', async () => {
    getStats.mockResolvedValue({
      ...fullStats,
      users: { total: 0, admins: 0, disabled: 0 },
      books: { synced: 0, catalog: 0, works: 0 },
      storage: { ...fullStats.storage, files_bytes: 0, blobs_bytes: 0, dedup_saved_bytes: 0 },
    });
    render(<StatsSection />);
    await waitFor(() => expect(screen.getAllByText('—').length).toBeGreaterThan(0));
    // Zero totals are real counts, rendered as the number 0.
    expect(screen.getAllByText('0').length).toBe(3); // users, synced, catalog
  });

  it('degrades to an error notice on API failure without crashing', async () => {
    getStats.mockRejectedValue(new Error('cannot reach server'));
    render(<StatsSection />);
    // Non-AdminApiError failures stringify as "Error: <message>".
    await waitFor(() => expect(screen.getByText(/cannot reach server/)).toBeTruthy());
    expect(screen.getByText('Overview')).toBeTruthy();
  });
});
