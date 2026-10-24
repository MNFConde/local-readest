/**
 * HostMonitorSection — the admin console's server-monitor tab, fed by the
 * supervisor's metrics snapshots.
 *
 * Contract under test: connected snapshots render per-service state, the
 * "not connected" / error states degrade to a notice instead of breaking the
 * page (the supervisor is optional: it only exists in bundle deployments),
 * and since ⑩ the per-service start/stop/restart buttons queue commands via
 * /api/admin/service-command — including the confirm guard before stopping
 * sync-server (this panel cannot recover itself from that stop).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';

const { getHostSnapshot, sendServiceCommand } = vi.hoisted(() => ({
  getHostSnapshot: vi.fn(),
  sendServiceCommand: vi.fn(),
}));

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (s: string) => s,
}));

vi.mock('@/services/selfhostedServerUrl', () => ({
  getSelfhostedServerUrl: () => 'http://127.0.0.1:8080',
}));

vi.mock('@/admin/adminApi', () => ({
  AdminApiError: class extends Error {},
  adminApi: { getHostSnapshot, sendServiceCommand },
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
    sendServiceCommand.mockReset();
    sendServiceCommand.mockResolvedValue(undefined);
    vi.stubGlobal(
      'confirm',
      vi.fn(() => true),
    );
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
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

  it('queues a restart for the selected service and confirms', async () => {
    getHostSnapshot.mockResolvedValue(connectedSnapshot);
    render(<HostMonitorSection />);
    await waitFor(() => expect(screen.getByText('web')).toBeTruthy());
    // Two rows × three buttons; restart the second row (web is row 2).
    const restartButtons = screen.getAllByText('Restart');
    expect(restartButtons.length).toBe(2);
    fireEvent.click(restartButtons[1]!);
    await waitFor(() => expect(sendServiceCommand).toHaveBeenCalledWith('web', 'restart'));
    await waitFor(() => expect(screen.getByText(/Command queued/)).toBeTruthy());
  });

  it('confirms before stopping sync-server and drops the command on cancel', async () => {
    const confirm = vi.fn(() => false);
    vi.stubGlobal('confirm', confirm);
    getHostSnapshot.mockResolvedValue(connectedSnapshot);
    render(<HostMonitorSection />);
    await waitFor(() => expect(screen.getByText('sync-server')).toBeTruthy());
    const stopButtons = screen.getAllByText('Stop');
    expect(stopButtons.length).toBe(2);
    // Stopping sync-server warns: this panel's own mailbox dies with it.
    fireEvent.click(stopButtons[0]!);
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(sendServiceCommand).not.toHaveBeenCalled();
    // web stops without a confirm (its recovery channel stays alive).
    fireEvent.click(stopButtons[1]!);
    expect(confirm).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(sendServiceCommand).toHaveBeenCalledWith('web', 'stop'));
  });

  it('surfaces a command failure as an error notice', async () => {
    sendServiceCommand.mockRejectedValue(new Error('queue rejected'));
    getHostSnapshot.mockResolvedValue(connectedSnapshot);
    render(<HostMonitorSection />);
    await waitFor(() => expect(screen.getByText('sync-server')).toBeTruthy());
    fireEvent.click(screen.getAllByText('Start')[0]!);
    await waitFor(() => expect(screen.getByText(/queue rejected/)).toBeTruthy());
  });
});
