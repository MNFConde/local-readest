'use client';

// SELFHOSTED (fork): the self-hosted sign-in form, extracted verbatim from
// the SELFHOSTED blocks that used to live inline in
// app/auth/components/EmailPasswordAuth.tsx — the shared file now stays
// upstream text plus a one-line early return. Sign-in only (view switching
// is a Supabase-flow concern): runtime server address + account (email) +
// optional password (passwordless accounts).

import { useEffect, useState } from 'react';
import { useTranslation } from '@/hooks/useTranslation';
import { selfhostedLogin, jwtToUser, saveCredentials } from '@/services/selfhostedAuth';
import type { SelfhostedUser } from '@/services/selfhostedAuth';
import { getSelfhostedServerUrl, saveSelfhostedServerUrl } from '@/services/selfhostedServerUrl';

interface SelfhostedAuthFormProps {
  // Called with the JWT issued by the sync server. The caller is responsible
  // for logging in AND navigating away (via SPA router.push) so the behavior
  // matches the OAuth path on every platform without a full WebView reload.
  onSelfhostedLogin?: (accessToken: string, user: SelfhostedUser) => void;
}

export default function SelfhostedAuthForm({ onSelfhostedLogin }: SelfhostedAuthFormProps) {
  const _ = useTranslation('selfhost');
  // Server address entered at runtime (persisted) instead of baked in at
  // build time, so a shared installer works for any user.
  const [serverUrl, setServerUrl] = useState(() => getSelfhostedServerUrl());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [keyboardInset, setKeyboardInset] = useState(0);

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

  const handleSubmit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    // Read credentials from the form's DOM state: Android/iOS password
    // managers fill WebView inputs without events React state can track.
    const formData = new FormData(event.currentTarget);
    const password = String(formData.get('password') || '');
    const email = String(formData.get('email') || '').trim();
    setError('');
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
      const err = e as Error & { status?: number; lockedUntil?: number; reason?: string };
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
        // 兜底：旧版本错误可能没有 reason 字段，仍按 429 处理
        if (err.lockedUntil) {
          const mins = Math.max(1, Math.ceil((err.lockedUntil - Date.now()) / 60000));
          setError(
            _('Too many failed attempts. This device is locked. Try again in {{minutes}} min.', {
              minutes: mins,
            }),
          );
        } else {
          setError(_('Too many failed attempts. This device is temporarily locked.'));
        }
      } else {
        setError(_('Incorrect email or password'));
      }
    } finally {
      setLoading(false);
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
      <div className='flex flex-col'>
        <label className='flex select-none items-center justify-between px-1 py-2' htmlFor='email'>
          <span className='text-sm'>{_('Email address')}</span>
        </label>
        <input
          id='email'
          name='email'
          type='email'
          required
          placeholder={_('Your email address')}
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
