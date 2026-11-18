/**
 * SelfhostedAuthForm — ⑪-P1 device-identity mode and ⑪-P2 remembered-logins
 * on the self-hosted sign-in screen. Contract: generate → save → sign in
 * passes (id, key) through selfhostedLogin and saveCredentials; import
 * normalizes and persists; a 401 in device mode names ALLOW_AUTO_PROVISION
 * instead of the generic wrong-password message. Remembered logins render as
 * chips that prefill the form; a connection switch resets the previous
 * connection's sync cursors + persisted transfer queue and hard-reloads.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';

const { selfhostedLogin, jwtToUser, saveCredentials } = vi.hoisted(() => ({
  selfhostedLogin: vi.fn(),
  jwtToUser: vi.fn(),
  saveCredentials: vi.fn(),
}));

const connectionsMocks = vi.hoisted(() => ({
  upsertConnectionOnLogin: vi.fn(),
  listConnections: vi.fn(),
  removeConnection: vi.fn(),
  setConnectionMode: vi.fn(),
  resetSyncCursorsForSwitch: vi.fn(),
  clearPersistedTransferQueue: vi.fn(),
  reloadForConnectionSwitch: vi.fn(),
  getActiveConnectionId: vi.fn(),
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

vi.mock('@/libs/selfhost/connections', () => connectionsMocks);

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
    // Defaults that keep ⑪-P1 scenarios free of remembered-login UI and of
    // switch semantics: empty list, same-connection login.
    connectionsMocks.listConnections.mockReset().mockReturnValue([]);
    connectionsMocks.getActiveConnectionId.mockReset().mockReturnValue('');
    connectionsMocks.upsertConnectionOnLogin.mockReset().mockReturnValue({
      connection: { id: 'http://127.0.0.1:8080|a@b.c', switched: false },
      switched: false,
    });
    connectionsMocks.removeConnection.mockReset();
    connectionsMocks.setConnectionMode.mockReset();
    connectionsMocks.resetSyncCursorsForSwitch.mockReset().mockResolvedValue(undefined);
    connectionsMocks.clearPersistedTransferQueue.mockReset();
    connectionsMocks.reloadForConnectionSwitch.mockReset();
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

describe('SelfhostedAuthForm remembered logins (⑪-P2)', () => {
  beforeEach(() => {
    localStorage.clear();
    selfhostedLogin.mockReset().mockResolvedValue({ access_token: 'jwt' });
    jwtToUser.mockReset().mockReturnValue({ id: 'u1' });
    saveCredentials.mockReset();
    connectionsMocks.listConnections.mockReset().mockReturnValue([]);
    connectionsMocks.getActiveConnectionId.mockReset().mockReturnValue('');
    connectionsMocks.upsertConnectionOnLogin.mockReset().mockReturnValue({
      connection: { id: 'x', switched: false },
      switched: false,
    });
    connectionsMocks.removeConnection.mockReset();
    connectionsMocks.setConnectionMode.mockReset();
    connectionsMocks.resetSyncCursorsForSwitch.mockReset().mockResolvedValue(undefined);
    connectionsMocks.clearPersistedTransferQueue.mockReset();
    connectionsMocks.reloadForConnectionSwitch.mockReset();
  });
  afterEach(() => {
    cleanup();
    localStorage.clear();
  });

  const ACCOUNT_CONN = {
    id: 'http://10.0.0.2:8080|alice@home',
    serverUrl: 'http://10.0.0.2:8080',
    email: 'alice@home',
    password: 'pw-alice',
    mode: 'full' as const,
    updatedAt: 1,
  };
  const DEVICE_CONN = {
    id: 'http://10.0.0.3:8080|01234567-89ab-4cde-8f01-23456789abcd',
    serverUrl: 'http://10.0.0.3:8080',
    email: VALID_ID,
    password: VALID_KEY,
    mode: 'metadata' as const,
    updatedAt: 2,
  };

  it('renders no remembered-logins block when the list is empty', () => {
    render(<SelfhostedAuthForm />);
    expect(screen.queryByText(/Remembered logins/)).toBeNull();
  });

  it('a remembered account chip prefills server, account and password', () => {
    connectionsMocks.listConnections.mockReturnValue([ACCOUNT_CONN]);
    render(<SelfhostedAuthForm />);

    fireEvent.click(screen.getByRole('button', { name: /alice@home/ }));
    expect((screen.getByLabelText('Sync server address') as HTMLInputElement).value).toBe(
      'http://10.0.0.2:8080',
    );
    expect((screen.getByLabelText('Account') as HTMLInputElement).value).toBe('alice@home');
    expect((screen.getByLabelText('Your Password') as HTMLInputElement).value).toBe('pw-alice');
    expect(screen.getByText('Sign in')).toBeTruthy(); // stays in account mode
  });

  it('a remembered device chip switches to device mode instead of prefilling', () => {
    connectionsMocks.listConnections.mockReturnValue([DEVICE_CONN]);
    render(<SelfhostedAuthForm />);

    fireEvent.click(screen.getByRole('button', { name: /\[Device\]/ }));
    // Device logins use this device's stored identity: with none stored the
    // generate panel shows (no form prefill happened).
    expect(screen.getByText('Generate a new device identity')).toBeTruthy();
    expect(screen.queryByLabelText('Account')).toBeNull();
    expect((screen.getByLabelText('Sync server address') as HTMLInputElement).value).toBe(
      'http://10.0.0.3:8080',
    );
  });

  it('manage mode exposes the transfer-mode select and two-stage forget', () => {
    connectionsMocks.listConnections.mockReturnValue([ACCOUNT_CONN]);
    render(<SelfhostedAuthForm />);

    fireEvent.click(screen.getByRole('button', { name: 'Manage' }));
    expect(screen.getByLabelText('Transfer mode')).toBeTruthy();
    expect(
      screen.getByText('Metadata-only connections skip book file transfers (covers excepted).'),
    ).toBeTruthy();

    fireEvent.change(screen.getByLabelText('Transfer mode'), { target: { value: 'metadata' } });
    expect(connectionsMocks.setConnectionMode).toHaveBeenCalledWith(ACCOUNT_CONN.id, 'metadata');

    fireEvent.click(screen.getByRole('button', { name: 'Forget' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm forget' }));
    expect(connectionsMocks.removeConnection).toHaveBeenCalledWith(ACCOUNT_CONN.id);
    expect(
      screen.getByText('Login removed from this device; the account stays on the server.'),
    ).toBeTruthy();
  });

  it('a connection switch resets cursors, clears the queue and hard-reloads', async () => {
    connectionsMocks.upsertConnectionOnLogin.mockReturnValue({
      connection: { id: 'http://127.0.0.1:8080|bob@b', switched: true },
      switched: true,
    });
    render(<SelfhostedAuthForm />);
    fireEvent.change(screen.getByLabelText('Sync server address'), {
      target: { value: 'http://127.0.0.1:8080' },
    });
    fireEvent.change(screen.getByLabelText('Account'), { target: { value: 'bob@b' } });
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));

    await waitFor(() => expect(saveCredentials).toHaveBeenCalled());
    expect(connectionsMocks.upsertConnectionOnLogin).toHaveBeenCalledWith(
      'http://127.0.0.1:8080',
      'bob@b',
      '',
    );
    expect(connectionsMocks.resetSyncCursorsForSwitch).toHaveBeenCalledTimes(1);
    expect(connectionsMocks.clearPersistedTransferQueue).toHaveBeenCalledTimes(1);
    expect(connectionsMocks.reloadForConnectionSwitch).toHaveBeenCalledTimes(1);
    expect(connectionsMocks.reloadForConnectionSwitch).toHaveBeenCalledWith(null);
  });

  it('same-connection login keeps the SPA path (no reset, no reload)', async () => {
    render(<SelfhostedAuthForm />);
    fireEvent.change(screen.getByLabelText('Account'), { target: { value: 'a@b.c' } });
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));

    await waitFor(() => expect(saveCredentials).toHaveBeenCalledWith('a@b.c', ''));
    expect(connectionsMocks.resetSyncCursorsForSwitch).not.toHaveBeenCalled();
    expect(connectionsMocks.clearPersistedTransferQueue).not.toHaveBeenCalled();
    expect(connectionsMocks.reloadForConnectionSwitch).not.toHaveBeenCalled();
  });
});
