/**
 * SelfhostDeviceMenuItem — ⑪-P1 settings-menu entry for the device identity.
 * Gate: token + stored identity (both re-read every render); dialog shows the
 * id, exports the backup, reveals the key, and Forget clears the local record.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';

const { downloadDeviceIdentityBackup, clearDeviceIdentity, getDeviceIdentity } = vi.hoisted(() => ({
  downloadDeviceIdentityBackup: vi.fn(),
  clearDeviceIdentity: vi.fn(),
  getDeviceIdentity: vi.fn(),
}));

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (s: string) => s,
}));

vi.mock('@/utils/supabase', () => ({ SELFHOSTED: true }));

vi.mock('@/services/selfhostedServerUrl', () => ({
  getSelfhostedServerUrl: () => 'http://127.0.0.1:8080',
}));

vi.mock('@/libs/selfhost/deviceIdentity', () => ({
  downloadDeviceIdentityBackup,
  clearDeviceIdentity,
  getDeviceIdentity,
}));

import SelfhostDeviceMenuItem from '@/app/library/components/SelfhostDeviceMenuItem';

const IDENTITY = {
  id: '01234567-89ab-4cde-8f01-23456789abcd',
  key: 'ABCDE-FGHJK-MNPQR-STVWX-YZ012',
};

describe('SelfhostDeviceMenuItem', () => {
  beforeEach(() => {
    localStorage.clear();
    getDeviceIdentity.mockReset().mockReturnValue(IDENTITY);
    downloadDeviceIdentityBackup
      .mockReset()
      .mockReturnValue('readest-device-identity-01234567.txt');
    clearDeviceIdentity.mockReset();
  });
  afterEach(() => {
    cleanup();
    localStorage.clear();
  });

  it('stays hidden without a stored token', () => {
    getDeviceIdentity.mockReturnValue(IDENTITY);
    const { container } = render(<SelfhostDeviceMenuItem />);
    expect(container.textContent).toBe('');
  });

  it('stays hidden without a stored identity even when signed in', () => {
    localStorage.setItem('token', 'jwt');
    getDeviceIdentity.mockReturnValue(null);
    const { container } = render(<SelfhostDeviceMenuItem />);
    expect(container.textContent).toBe('');
  });

  it('shows the id in the dialog and exports the backup', async () => {
    localStorage.setItem('token', 'jwt');
    render(<SelfhostDeviceMenuItem onNavigated={vi.fn()} />);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Device identity' }));

    expect(await screen.findByText(IDENTITY.id)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Export backup' }));
    await waitFor(() =>
      expect(downloadDeviceIdentityBackup).toHaveBeenCalledWith(IDENTITY, 'http://127.0.0.1:8080'),
    );
    // Mocked t() returns the key verbatim; the locale mapping is i18n's job.
    expect(screen.getByText('Backup downloaded: {{name}}')).toBeTruthy();
  });

  it('reveals the key behind the Show key toggle', async () => {
    localStorage.setItem('token', 'jwt');
    render(<SelfhostDeviceMenuItem />);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Device identity' }));
    expect(screen.queryByText(IDENTITY.key)).toBeNull();
    fireEvent.click(await screen.findByRole('button', { name: 'Show key' }));
    expect(screen.getByText(IDENTITY.key)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Hide key' }));
    expect(screen.queryByText(IDENTITY.key)).toBeNull();
  });

  it('forgets the identity after confirm and unmounts itself', async () => {
    localStorage.setItem('token', 'jwt');
    // After Forget the real module returns null; mirror that so the
    // re-read-every-render gate hides the item.
    clearDeviceIdentity.mockImplementation(() => getDeviceIdentity.mockReturnValue(null));
    const { container } = render(<SelfhostDeviceMenuItem />);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Device identity' }));
    fireEvent.click(await screen.findByText('Forget this identity on this device'));
    fireEvent.click(screen.getByRole('button', { name: /^Forget$/ }));

    expect(clearDeviceIdentity).toHaveBeenCalled();
    await waitFor(() => expect(container.textContent).toBe(''));
  });
});
