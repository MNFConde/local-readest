/**
 * ⑪-P3 ManualConnectRow — the collapsed-by-default manual connect row in the
 * nearby-send picker. Contract: renders a single disclosure button until
 * expanded; a valid address calls connectManualLocalSend (port defaulted to
 * 53317 when omitted) and confirms with the peer alias; a failure surfaces
 * the error text; collapse clears the row state. parseManualTarget stays real
 * (only the network call is mocked) so the malformed-address path is the
 * production one.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';

const { connectManualLocalSend } = vi.hoisted(() => ({ connectManualLocalSend: vi.fn() }));

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (s: string, opts?: Record<string, unknown>) =>
    opts && 'alias' in opts ? s.replace('{{alias}}', String(opts['alias'])) : s,
}));

vi.mock('@/libs/selfhost/localsendManual', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/libs/selfhost/localsendManual')>();
  return { ...actual, connectManualLocalSend };
});

import ManualConnectRow from '@/libs/selfhost/ManualConnectRow';

describe('ManualConnectRow', () => {
  beforeEach(() => {
    connectManualLocalSend.mockReset();
  });

  afterEach(cleanup);

  it('starts collapsed with just the disclosure button', () => {
    render(<ManualConnectRow />);
    expect(screen.getByText('Add device manually')).toBeTruthy();
    expect(screen.queryByPlaceholderText('Host or IP, e.g. 192.168.1.7:53317')).toBeNull();
  });

  it('expands to the address form and connects with defaulted port', async () => {
    connectManualLocalSend.mockResolvedValueOnce({ alias: 'Desk', fingerprint: 'fp' });
    render(<ManualConnectRow />);
    fireEvent.click(screen.getByText('Add device manually'));
    const input = screen.getByPlaceholderText('Host or IP, e.g. 192.168.1.7:53317');
    fireEvent.change(input, { target: { value: '192.168.1.7' } });
    fireEvent.click(screen.getByText('Connect'));
    await waitFor(() => expect(connectManualLocalSend).toHaveBeenCalledWith('192.168.1.7', 53317));
    expect(await screen.findByText(/Connected to Desk/)).toBeTruthy();
  });

  it('surfaces the failure text on error', async () => {
    connectManualLocalSend.mockRejectedValueOnce(new Error('no LocalSend service at host:1'));
    render(<ManualConnectRow />);
    fireEvent.click(screen.getByText('Add device manually'));
    fireEvent.change(screen.getByPlaceholderText('Host or IP, e.g. 192.168.1.7:53317'), {
      target: { value: 'host:1' },
    });
    fireEvent.click(screen.getByText('Connect'));
    expect(await screen.findByText('no LocalSend service at host:1')).toBeTruthy();
  });

  it('rejects a malformed address without calling the backend', async () => {
    render(<ManualConnectRow />);
    fireEvent.click(screen.getByText('Add device manually'));
    const input = screen.getByPlaceholderText('Host or IP, e.g. 192.168.1.7:53317');
    fireEvent.change(input, { target: { value: 'host:notaport' } });
    const button = screen.getByText('Connect').closest('button');
    expect(button?.disabled).toBe(false);
    fireEvent.click(button!);
    await waitFor(() =>
      expect(screen.getByText('Enter a valid address like 192.168.1.7:53317')).toBeTruthy(),
    );
    expect(connectManualLocalSend).not.toHaveBeenCalled();
  });

  it('collapse clears error and confirmation state', async () => {
    connectManualLocalSend.mockResolvedValueOnce({ alias: 'Desk', fingerprint: 'fp' });
    render(<ManualConnectRow />);
    fireEvent.click(screen.getByText('Add device manually'));
    fireEvent.change(screen.getByPlaceholderText('Host or IP, e.g. 192.168.1.7:53317'), {
      target: { value: '192.168.1.7' },
    });
    fireEvent.click(screen.getByText('Connect'));
    await screen.findByText(/Connected to Desk/);
    fireEvent.click(screen.getByLabelText('Collapse manual connect'));
    expect(screen.getByText('Add device manually')).toBeTruthy();
    expect(screen.queryByText(/Connected to Desk/)).toBeNull();
  });
});
