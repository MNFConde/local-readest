'use client';

// SELFHOSTED (fork): the self-hosted sign-in form, extracted verbatim from
// the SELFHOSTED blocks that used to live inline in
// app/auth/components/EmailPasswordAuth.tsx — the shared file now stays
// upstream text plus a one-line early return. Sign-in only (view switching
// is a Supabase-flow concern): runtime server address + account (username
// or email — the server treats it as an opaque identifier) + optional
// password (passwordless accounts).
//
// ROADMAP ⑪-P1: besides the account form there is a device-identity mode —
// generate/import a local {id, key} pair and sign in with it; the server
// auto-provisions the account when ALLOW_AUTO_PROVISION is on. The key is
// shown exactly once at generation and the UI pushes a backup download hard:
// there is no server-side recovery for a lost key.

import { useEffect, useState } from 'react';
import { useTranslation } from '@/hooks/useTranslation';
import { selfhostedLogin, jwtToUser, saveCredentials } from '@/services/selfhostedAuth';
import type { SelfhostedUser } from '@/services/selfhostedAuth';
import { getSelfhostedServerUrl, saveSelfhostedServerUrl } from '@/services/selfhostedServerUrl';
import {
  clearDeviceIdentity,
  downloadDeviceIdentityBackup,
  generateDeviceIdentity,
  getDeviceIdentity,
  parseDeviceIdentity,
  saveDeviceIdentity,
} from '@/libs/selfhost/deviceIdentity';
import type { DeviceIdentity } from '@/libs/selfhost/deviceIdentity';

interface SelfhostedAuthFormProps {
  // Called with the JWT issued by the sync server. The caller is responsible
  // for logging in AND navigating away (via SPA router.push) so the behavior
  // matches the OAuth path on every platform without a full WebView reload.
  onSelfhostedLogin?: (accessToken: string, user: SelfhostedUser) => void;
}

type AuthMode = 'account' | 'device';

export default function SelfhostedAuthForm({ onSelfhostedLogin }: SelfhostedAuthFormProps) {
  const _ = useTranslation('selfhost');
  // Server address entered at runtime (persisted) instead of baked in at
  // build time, so a shared installer works for any user.
  const [serverUrl, setServerUrl] = useState(() => getSelfhostedServerUrl());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [keyboardInset, setKeyboardInset] = useState(0);
  const [mode, setMode] = useState<AuthMode>('account');
  const [identity, setIdentity] = useState<DeviceIdentity | null>(() => getDeviceIdentity());
  const [revealed, setRevealed] = useState(false);
  const [importing, setImporting] = useState(false);

  useEffect(() => {
    const vv = window.visualViewport;
    if (!vv) return;
    const update = () => {
      const layoutH = document.documentElement.clientHeight;
      const offset = layoutH - vv.height - vv.offsetTop;
      setKeyboardInset(offset > 1 ? offset : 0);
    };
    vv.addEventListener('resize', update);
    vv.addEventListener('scroll', update);
    update();
    return () => {
      vv.removeEventListener('resize', update);
      vv.removeEventListener('scroll', update);
    };
  }, []);

  // The Android WebView does not resize with the keyboard, and Chromium
  // resets the visual-viewport pan when the IME advances focus (keyboard
  // Next) without re-scrolling the newly focused editable. The keyboard
  // spacer gives the page scroller enough range, and the focus handler
  // scrolls the input back into the visible area above the keyboard.
  const keepAboveKeyboard = (event: React.FocusEvent<HTMLInputElement>) => {
    const input = event.currentTarget;
    setTimeout(() => {
      if (document.activeElement === input) {
        input.scrollIntoView({ block: 'center', behavior: 'smooth' });
      }
    }, 300);
  };

  const describeLoginError = (
    err: Error & { status?: number; reason?: string },
    device: boolean,
  ) => {
    if (err.reason === 'network') {
      // 连不上服务端：URL 没配对 / 服务端没启动 / 防火墙 / CORS
      setError(
        _(
          'Cannot connect to the sync server. Check that the server is running and the server address is correctly configured in the app.',
        ),
      );
    } else if (err.reason === 'no-url') {
      setError(_('Please enter your sync server address above before signing in.'));
    } else if (err.reason === 'server') {
      setError(
        _(
          'The sync server encountered an internal error. Please check the server logs and try again later.',
        ),
      );
    } else if (err.reason === 'locked' || err.status === 429) {
      setError(_('Too many failed attempts. This device is temporarily locked.'));
    } else if (device) {
      // 401 in device mode almost always means the server has not enabled
      // auto-provisioning (or the pair is wrong) — say so instead of the
      // account-form wording.
      setError(
        _(
          'The server did not accept this device identity. Check the ID/key and that the server enables ALLOW_AUTO_PROVISION.',
        ),
      );
    } else {
      setError(_('Incorrect account or password'));
    }
  };

  const loginWith = async (email: string, password: string, device: boolean) => {
    setError('');
    setNotice('');
    setLoading(true);
    try {
      saveSelfhostedServerUrl(serverUrl);
      const { access_token } = await selfhostedLogin(email, password);
      const user = jwtToUser(access_token);
      if (!user) throw new Error('invalid token');
      // Persist the credentials so an expired JWT can be refreshed without
      // re-prompting (mirrors Supabase's persisted refresh_token).
      saveCredentials(email, password);
      onSelfhostedLogin?.(access_token, user);
    } catch (e) {
      describeLoginError(e as Error & { status?: number; reason?: string }, device);
    } finally {
      setLoading(false);
    }
  };

  const handleSubmit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    // Read credentials from the form's DOM state: Android/iOS password
    // managers fill WebView inputs without events React state can track.
    const formData = new FormData(event.currentTarget);
    const password = String(formData.get('password') || '');
    const email = String(formData.get('email') || '').trim();
    await loginWith(email, password, false);
  };

  const handleGenerate = () => {
    setError('');
    setNotice('');
    const fresh = generateDeviceIdentity();
    saveDeviceIdentity(fresh);
    setIdentity(fresh);
    setRevealed(true);
  };

  // Plain-div import block: nested <form>s are invalid HTML (the sign-in
  // form wraps everything), so the inputs are read by id on click instead of
  // via FormData.
  const handleImportInputs = () => {
    setError('');
    setNotice('');
    const read = (id: string) =>
      (document.getElementById(id) as HTMLInputElement | null)?.value ?? '';
    try {
      const imported = parseDeviceIdentity(read('device-id'), read('device-key'));
      saveDeviceIdentity(imported);
      setIdentity(imported);
      setRevealed(false);
      setImporting(false);
      setNotice(_('Identity imported to this device.'));
    } catch {
      setError(
        _('Invalid identity: the ID must be a UUID and the key 25 characters (dashes optional).'),
      );
    }
  };

  const handleBackupFile = async (event: React.ChangeEvent<HTMLInputElement>) => {
    setError('');
    setNotice('');
    const file = event.target.files?.[0];
    event.target.value = ''; // allow re-picking the same file
    if (!file) return;
    try {
      const text = await file.text();
      const id = /^\s*id:\s*(\S+)/im.exec(text)?.[1] ?? '';
      const key = /^\s*key:\s*(\S+)/im.exec(text)?.[1] ?? '';
      const imported = parseDeviceIdentity(id, key);
      saveDeviceIdentity(imported);
      setIdentity(imported);
      setRevealed(false);
      setImporting(false);
      setNotice(_('Identity imported to this device.'));
    } catch {
      setError(
        _('Invalid identity: the ID must be a UUID and the key 25 characters (dashes optional).'),
      );
    }
  };

  const handleForget = () => {
    clearDeviceIdentity();
    setIdentity(null);
    setRevealed(false);
    setImporting(false);
    setError('');
    setNotice(
      _(
        'Only the local record is removed; the account stays on the server. Import your backup to sign in again.',
      ),
    );
  };

  const handleBackupDownload = () => {
    if (!identity) return;
    const name = downloadDeviceIdentityBackup(identity, serverUrl);
    setNotice(_('Backup downloaded: {{name}}', { name }));
  };

  const handleCopy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setNotice(_('Copied.'));
    } catch {
      // Clipboard can be denied (insecure context / WebView); the reveal
      // state already lets the user copy manually.
    }
  };

  return (
    <form id='auth-sign-in' method='post' onSubmit={handleSubmit} className='w-full space-y-4'>
      <div className='flex flex-col'>
        <label
          className='flex select-none items-center justify-between px-1 py-2'
          htmlFor='server-url'
        >
          <span className='text-sm'>{_('Sync server address')}</span>
        </label>
        <input
          id='server-url'
          name='server-url'
          // type='text' (not 'url') so a schemeless `127.0.0.1:8080` can be
          // typed freely; normalizeServerUrl adds the http:// on save.
          type='text'
          inputMode='url'
          value={serverUrl}
          onChange={(e) => setServerUrl(e.target.value)}
          placeholder={_('https://sync.your-server.com')}
          autoComplete='url'
          className='input eink-bordered w-full rounded-lg placeholder:text-sm'
          disabled={loading}
          onFocus={keepAboveKeyboard}
        />
      </div>

      {mode === 'account' ? (
        <>
          <div className='flex flex-col'>
            <label
              className='flex select-none items-center justify-between px-1 py-2'
              htmlFor='email'
            >
              <span className='text-sm'>{_('Account')}</span>
            </label>
            <input
              id='email'
              name='email'
              // type='text' (not 'email'): the server accepts any account
              // identifier, and native browser validation would block plain
              // usernames. name/id stay 'email' — that is the API field name.
              type='text'
              required
              placeholder={_('Account or email')}
              autoComplete='username'
              className='input eink-bordered w-full rounded-lg placeholder:text-sm'
              disabled={loading}
              onFocus={keepAboveKeyboard}
            />
          </div>
          <div className='flex flex-col'>
            <label
              className='flex select-none items-center justify-between px-1 py-2'
              htmlFor='password'
            >
              <span className='text-sm'>{_('Your Password')}</span>
            </label>
            <input
              id='password'
              name='password'
              type='password'
              // Self-hosted accounts may be passwordless: the field stays optional.
              required={false}
              placeholder={_('Your password')}
              autoComplete='current-password'
              className='input eink-bordered w-full rounded-lg placeholder:text-sm'
              disabled={loading}
              onFocus={keepAboveKeyboard}
            />
          </div>
          <button type='submit' className='btn btn-primary w-full rounded-lg' disabled={loading}>
            {loading && <span className='loading loading-spinner loading-sm' aria-hidden='true' />}
            {loading ? _('Signing in ...') : _('Sign in')}
          </button>
          <div className='text-center'>
            <button
              type='button'
              className='link text-sm'
              onClick={() => {
                setError('');
                setNotice('');
                setIdentity(getDeviceIdentity());
                setMode('device');
              }}
            >
              {_('Use a device identity instead')}
            </button>
          </div>
        </>
      ) : (
        <>
          {identity ? (
            <>
              <div className='eink-bordered rounded-lg px-3 py-2.5 text-sm leading-relaxed'>
                <div className='mb-1 font-medium'>{_('This device has a device identity')}</div>
                <div className='text-neutral-content/70 break-all font-mono text-xs'>
                  {identity.id}
                </div>
                <div className='mt-2 flex flex-wrap gap-2'>
                  <button
                    type='button'
                    className='btn btn-sm btn-outline rounded-lg'
                    onClick={() => setRevealed((r) => !r)}
                  >
                    {revealed ? _('Hide key') : _('Show key')}
                  </button>
                  <button
                    type='button'
                    className='btn btn-sm btn-outline rounded-lg'
                    onClick={handleBackupDownload}
                  >
                    {_('Export backup')}
                  </button>
                </div>
                {revealed && (
                  <div className='mt-2'>
                    <div className='text-neutral-content/70 font-mono text-xs break-all select-all'>
                      {identity.key}
                    </div>
                    <button
                      type='button'
                      className='link mt-1 text-xs'
                      onClick={() => handleCopy(identity.key)}
                    >
                      {_('Copy')}
                    </button>
                  </div>
                )}
              </div>
              <button
                type='button'
                className='btn btn-primary w-full rounded-lg'
                disabled={loading}
                onClick={() => loginWith(identity.id, identity.key, true)}
              >
                {loading && (
                  <span className='loading loading-spinner loading-sm' aria-hidden='true' />
                )}
                {loading ? _('Signing in ...') : _('Sign in with this identity')}
              </button>
              <div className='text-center'>
                <button type='button' className='link text-sm text-error' onClick={handleForget}>
                  {_('Forget this identity on this device')}
                </button>
              </div>
            </>
          ) : (
            <>
              <div className='eink-bordered rounded-lg px-3 py-2.5 text-sm leading-relaxed'>
                {_(
                  'A device identity is generated on this device and becomes its account on the sync server. The key cannot be recovered if lost — back it up.',
                )}
              </div>
              <button
                type='button'
                className='btn btn-primary w-full rounded-lg'
                onClick={handleGenerate}
                disabled={loading}
              >
                {_('Generate a new device identity')}
              </button>
              {!importing && (
                <div className='text-center'>
                  <button
                    type='button'
                    className='link text-sm'
                    onClick={() => {
                      setError('');
                      setImporting(true);
                    }}
                  >
                    {_('Import an existing identity')}
                  </button>
                </div>
              )}
              {importing && (
                <div className='space-y-3'>
                  <div className='flex flex-col'>
                    <label className='px-1 py-1 text-sm' htmlFor='device-id'>
                      {_('Identity ID')}
                    </label>
                    <input
                      id='device-id'
                      name='device-id'
                      type='text'
                      placeholder='01234567-89ab-4cde-8f01-23456789abcd'
                      className='input eink-bordered w-full rounded-lg font-mono text-xs placeholder:text-xs'
                      disabled={loading}
                    />
                  </div>
                  <div className='flex flex-col'>
                    <label className='px-1 py-1 text-sm' htmlFor='device-key'>
                      {_('Identity key')}
                    </label>
                    <input
                      id='device-key'
                      name='device-key'
                      type='text'
                      // Not type='password': the key is random base32, not a
                      // memorable secret, and showing it helps transcription.
                      placeholder='ABCDE-FGHJK-MNPQR-STVWX-YZ012'
                      className='input eink-bordered w-full rounded-lg font-mono text-xs placeholder:text-xs'
                      disabled={loading}
                    />
                  </div>
                  <div className='flex items-center justify-between'>
                    <label className='link text-sm cursor-pointer'>
                      {_('Read a backup file')}
                      <input
                        type='file'
                        accept='.txt,text/plain'
                        className='hidden'
                        onChange={handleBackupFile}
                      />
                    </label>
                    <button
                      type='button'
                      className='btn btn-sm btn-primary rounded-lg'
                      onClick={handleImportInputs}
                    >
                      {_('Import')}
                    </button>
                  </div>
                </div>
              )}
            </>
          )}
          <div className='text-center'>
            <button
              type='button'
              className='link text-sm'
              onClick={() => {
                setError('');
                setNotice('');
                setImporting(false);
                setMode('account');
              }}
            >
              {_('Sign in with an account instead')}
            </button>
          </div>
        </>
      )}

      {notice && (
        <div className='eink-bordered rounded-lg border px-3 py-2.5 text-center text-sm leading-relaxed'>
          {notice}
        </div>
      )}
      {error && (
        <div className='eink-bordered border-error/30 bg-error/5 text-error rounded-lg border px-3 py-2.5 text-center text-sm leading-relaxed'>
          {error}
        </div>
      )}
      {keyboardInset > 0 && (
        <div data-keyboard-spacer='' aria-hidden='true' style={{ height: `${keyboardInset}px` }} />
      )}
    </form>
  );
}
