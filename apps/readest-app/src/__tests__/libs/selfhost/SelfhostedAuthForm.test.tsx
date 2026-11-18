/**
 * SelfhostedAuthForm — ⑪-P1 device-identity mode on the self-hosted sign-in
 * screen. Contract: generate → save → sign in passes (id, key) through
 * selfhostedLogin and saveCredentials; import normalizes and persists;
 * a 401 in device mode names ALLOW_AUTO_PROVISION instead of the generic
 * wrong-password message.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';

const { selfhostedLogin, jwtToUser, saveCredentials } = vi.hoisted(() => ({
  selfhostedLogin: vi.fn(),
  jwtToUser: vi.fn(),
  saveCredentials: vi.fn(),
}));

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (s: string) => s,
}));

vi.mock('@/utils/supabase', () => ({ SELFHOSTED: true }));

vi.mock('@/services/selfhostedAuth', () => ({
  selfhostedLogin,
  jwtToUser,
  saveCredentials,
}));

vi.mock('@/services/selfhostedServerUrl', () => ({
  getSelfhostedServerUrl: () => 'http://127.0.0.1:8080',
  saveSelfhostedServerUrl: vi.fn(),
}));

import SelfhostedAuthForm from '@/libs/selfhost/SelfhostedAuthForm';
import { getDeviceIdentity } from '@/libs/selfhost/deviceIdentity';

const VALID_ID = '01234567-89ab-4cde-8f01-23456789abcd';
const VALID_KEY = 'ABCDE-FGHJK-MNPQR-STVWX-YZ012';

describe('SelfhostedAuthForm device mode', () => {
  beforeEach(() => {
    localStorage.clear();
    selfhostedLogin.mockReset().mockResolvedValue({ access_token: 'jwt' });
    jwtToUser.mockReset().mockReturnValue({ id: 'u1' });
    saveCredentials.mockReset();
  });
  afterEach(() => {
    cleanup();
    localStorage.clear();
  });

  const openDeviceMode = async () => {
    render(<SelfhostedAuthForm onSelfhostedLogin={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Use a device identity instead' }));
    await screen.findByText('Generate a new device identity');
  };

  it('starts in account mode with the device link', () => {
    render(<SelfhostedAuthForm />);
    expect(screen.getByLabelText('Account')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Use a device identity instead' })).toBeTruthy();
  });

  it('generates an identity, persists it, and signs in with the pair', async () => {
    const onSelfhostedLogin = vi.fn();
    render(<SelfhostedAuthForm onSelfhostedLogin={onSelfhostedLogin} />);
    fireEvent.click(screen.getByRole('button', { name: 'Use a device identity instead' }));
    fireEvent.click(await screen.findByText('Generate a new device identity'));

    const stored = getDeviceIdentity();
    expect(stored).toBeTruthy();
    expect(screen.getByText(stored!.id)).toBeTruthy();
    expect(screen.getByText(stored!.key)).toBeTruthy(); // revealed by default

    fireEvent.click(screen.getByRole('button', { name: 'Sign in with this identity' }));
    await waitFor(() => expect(selfhostedLogin).toHaveBeenCalledWith(stored!.id, stored!.key));
    expect(saveCredentials).toHaveBeenCalledWith(stored!.id, stored!.key);
    expect(onSelfhostedLogin).toHaveBeenCalledWith('jwt', { id: 'u1' });
  });

  it('imports a lowercased/dashed pair and signs in', async () => {
    await openDeviceMode();
    fireEvent.click(screen.getByRole('button', { name: 'Import an existing identity' }));
    fireEvent.change(screen.getByLabelText('Identity ID'), {
      target: { value: VALID_ID.toUpperCase() },
    });
    fireEvent.change(screen.getByLabelText('Identity key'), {
      target: { value: 'abcde fghjk mnpqr stvwx yz012' },
    });
    fireEvent.click(screen.getByRole('button', { name: /^Import$/ }));

    await waitFor(() => expect(screen.getByText('Identity imported to this device.')).toBeTruthy());
    expect(getDeviceIdentity()).toEqual({ id: VALID_ID, key: VALID_KEY });

    fireEvent.click(screen.getByRole('button', { name: 'Sign in with this identity' }));
    await waitFor(() => expect(selfhostedLogin).toHaveBeenCalledWith(VALID_ID, VALID_KEY));
  });

  it('rejects a malformed import without touching storage', async () => {
    await openDeviceMode();
    fireEvent.click(screen.getByRole('button', { name: 'Import an existing identity' }));
    fireEvent.change(screen.getByLabelText('Identity ID'), {
      target: { value: 'not-a-uuid' },
    });
    fireEvent.change(screen.getByLabelText('Identity key'), { target: { value: VALID_KEY } });
    fireEvent.click(screen.getByRole('button', { name: /^Import$/ }));

    expect(
      await screen.findByText(
        'Invalid identity: the ID must be a UUID and the key 25 characters (dashes optional).',
      ),
    ).toBeTruthy();
    expect(getDeviceIdentity()).toBeNull();
  });

  it('names ALLOW_AUTO_PROVISION on a device-mode 401', async () => {
    selfhostedLogin.mockRejectedValue(
      Object.assign(new Error('invalid'), { status: 401, reason: 'invalid-credentials' }),
    );
    await openDeviceMode();
    fireEvent.click(screen.getByText('Generate a new device identity'));
    fireEvent.click(screen.getByRole('button', { name: 'Sign in with this identity' }));

    expect(
      await screen.findByText(
        'The server did not accept this device identity. Check the ID/key and that the server enables ALLOW_AUTO_PROVISION.',
      ),
    ).toBeTruthy();
  });

  it('forgets the identity and drops back to the generate panel', async () => {
    await openDeviceMode();
    fireEvent.click(screen.getByText('Generate a new device identity'));
    fireEvent.click(screen.getByRole('button', { name: 'Forget this identity on this device' }));

    expect(
      await screen.findByText(
        'Only the local record is removed; the account stays on the server. Import your backup to sign in again.',
      ),
    ).toBeTruthy();
    expect(getDeviceIdentity()).toBeNull();
    expect(screen.getByText('Generate a new device identity')).toBeTruthy();
  });
});
