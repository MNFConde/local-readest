/**
 * HostMonitorSection — the admin console's server-monitor tab, fed by the
 * supervisor's metrics snapshots.
 *
 * Contract under test: connected snapshots render per-service state, and the
 * "not connected" / error states degrade to a notice instead of breaking the
 * page (the supervisor is optional: it only exists in bundle deployments).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';

const { getHostSnapshot } = vi.hoisted(() => ({ getHostSnapshot: vi.fn() }));

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (s: string) => s,
}));

vi.mock('@/services/selfhostedServerUrl', () => ({
  getSelfhostedServerUrl: () => 'http://127.0.0.1:8080',
}));

vi.mock('@/admin/adminApi', () => ({
  AdminApiError: class extends Error {},
  adminApi: { getHostSnapshot },
}));

import HostMonitorSection from '@/admin/HostMonitorSection';

const connectedSnapshot = {
  available: true,
  received_at: 1760000000,
  age_secs: 2,
  snapshot: {
    schema: 1,
    ts: 1760000000,
    supervisor: { version: '0.12.14', pid: 42, uptime_secs: 100, headless: false },
    host: { os: 'windows', cores: 8, total_memory_bytes: 17_000_000_000 },
    services: [
      {
        name: 'sync-server',
        state: 'running',
        pid: 101,
        uptime_secs: 95,
        restarts: 0,
        probe: { ok: true, status: 200, rtt_ms: 3 },
        memory_bytes: 45_000_000,
        cpu_percent: 0.5,
      },
      {
        name: 'web',
        state: 'restarting',
        pid: null,
        restarts: 2,
        consecutive_failures: 2,
        circuit_reason: null,
      },
    ],
  },
};

describe('HostMonitorSection', () => {
  beforeEach(() => {
    getHostSnapshot.mockReset();
  });
  afterEach(() => {
    cleanup();
  });

  it('renders services and supervisor info when a snapshot is available', async () => {
    getHostSnapshot.mockResolvedValue(connectedSnapshot);
    render(<HostMonitorSection />);
    await waitFor(() => expect(screen.getByText('sync-server')).toBeTruthy());
    expect(screen.getByText('web')).toBeTruthy();
    expect(screen.getByText('running')).toBeTruthy();
    expect(screen.getByText('restarting')).toBeTruthy();
    // Supervisor summary line renders (identity-mocked i18n leaves
    // {{placeholders}} literal — asserting the raw key proves the line drew
    // from a connected snapshot rather than the degraded notices).
    expect(screen.getByText(/Updated \{\{age\}\}s ago · supervisor v\{\{version\}\}/)).toBeTruthy();
    // Probe RTT from the running service.
    expect(screen.getByText('3ms')).toBeTruthy();
  });

  it('shows the not-connected notice when no supervisor feed exists', async () => {
    getHostSnapshot.mockResolvedValue({ available: false });
    render(<HostMonitorSection />);
    await waitFor(() => expect(screen.getByText(/Supervisor not connected/)).toBeTruthy());
    expect(screen.queryByText('sync-server')).toBeNull();
  });

  it('degrades to an error notice on API failure without crashing', async () => {
    getHostSnapshot.mockRejectedValue(new Error('cannot reach server'));
    render(<HostMonitorSection />);
    // Non-AdminApiError failures stringify as "Error: <message>".
    await waitFor(() => expect(screen.getByText(/cannot reach server/)).toBeTruthy());
    expect(screen.getByText('Server Monitor')).toBeTruthy();
  });
});
