'use client';

// Multi-platform admin console for the self-hosted sync server. Self-contained
// by design (project constraint): daisyUI + Tailwind classes only, no imports
// from upstream business components, single mount point at src/app/admin/page.
// Reached at /admin by admin accounts on web, desktop, and Android alike.

import React, { useCallback, useEffect, useState } from 'react';
import { useTranslation } from '@/hooks/useTranslation';
import { jwtRole, jwtToUser } from '@/services/selfhostedAuth';
import { adminApi, AdminApiError, AdminUser, CatalogBook } from './adminApi';
import HostMonitorSection from './HostMonitorSection';
import StatsSection from './StatsSection';

type Gate = 'checking' | 'ok' | 'denied' | 'anon';

type Tab = 'overview' | 'users' | 'books' | 'monitor' | 'maintenance';

type Dialog =
  | { kind: 'delete-user'; email: string }
  | { kind: 'reset-password'; email: string }
  | { kind: 'delete-book'; book: CatalogBook }
  | { kind: 'show-password'; password: string; generated: boolean }
  | null;

function formatCreated(unix: string): string {
  const secs = Number(unix);
  if (!Number.isFinite(secs) || secs <= 0) return unix;
  return new Date(secs * 1000).toLocaleString();
}

export default function AdminApp() {
  const _ = useTranslation('selfhost');
  const [gate, setGate] = useState<Gate>('checking');
  const [selfEmail, setSelfEmail] = useState<string>('');
  const [tab, setTab] = useState<Tab>('overview');
  const [users, setUsers] = useState<AdminUser[] | null>(null);
  const [books, setBooks] = useState<CatalogBook[] | null>(null);
  const [ungroupedOnly, setUngroupedOnly] = useState(false);
  const [error, setError] = useState<string>('');
  const [notice, setNotice] = useState<string>('');
  const [dialog, setDialog] = useState<Dialog>(null);
  const [resetInput, setResetInput] = useState<string>('');
  const [creating, setCreating] = useState(false);
  const [newEmail, setNewEmail] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [newRole, setNewRole] = useState<'user' | 'admin'>('user');
  const [gcResult, setGcResult] = useState<string>('');
  const [enrichResult, setEnrichResult] = useState<string>('');

  useEffect(() => {
    const token = typeof window !== 'undefined' ? localStorage.getItem('token') : null;
    setSelfEmail(jwtToUser(token)?.email ?? '');
    setGate(!token ? 'anon' : jwtRole(token) === 'admin' ? 'ok' : 'denied');
  }, []);

  const run = useCallback(
    async (action: () => Promise<string | void>) => {
      setError('');
      setNotice('');
      try {
        const message = await action();
        if (typeof message === 'string') setNotice(message);
      } catch (err) {
        if (err instanceof AdminApiError) {
          setError(
            err.status === 401
              ? _('Session expired — sign in again')
              : err.status === 403
                ? _('Admin access required')
                : err.message,
          );
        } else {
          setError(String(err));
        }
      }
    },
    [_],
  );

  const refreshUsers = useCallback(
    () => run(async () => setUsers(await adminApi.listUsers())),
    [run],
  );
  const refreshBooks = useCallback(
    () => run(async () => setBooks(await adminApi.listBooks())),
    [run],
  );

  useEffect(() => {
    if (gate !== 'ok') return;
    refreshUsers();
    refreshBooks();
  }, [gate, refreshUsers, refreshBooks]);

  if (gate === 'checking') {
    return <Centered>{_('Loading…')}</Centered>;
  }
  if (gate === 'anon') {
    return (
      <Centered>
        <p className='mb-4'>{_('Not signed in')}</p>
        <a href='/auth' className='btn btn-primary btn-sm'>
          {_('Sign in')}
        </a>
      </Centered>
    );
  }
  if (gate === 'denied') {
    return (
      <Centered>
        <p className='mb-4'>{_('Admin access required')}</p>
        <a href='/library' className='btn btn-ghost btn-sm'>
          {_('Back to reading')}
        </a>
      </Centered>
    );
  }

  const isSelf = (email: string) => email === selfEmail;

  const createUser = () =>
    run(async () => {
      if (!newEmail.trim()) throw new AdminApiError(_('Email is required'), 400);
      setCreating(true);
      try {
        const created = await adminApi.createUser(
          newEmail.trim(),
          newPassword === '' ? undefined : newPassword,
          newRole,
        );
        setNewEmail('');
        setNewPassword('');
        setNewRole('user');
        setUsers(await adminApi.listUsers());
        if (created.password === '') {
          return _('Created passwordless account — login needs only the email');
        }
        setDialog({ kind: 'show-password', password: created.password, generated: true });
        return '';
      } finally {
        setCreating(false);
      }
    });

  const submitReset = (email: string, password: string) =>
    run(async () => {
      const res = await adminApi.resetPassword(email, password === '' ? undefined : password);
      setDialog(null);
      setResetInput('');
      if (res.password) {
        setDialog({ kind: 'show-password', password: res.password, generated: true });
        return '';
      }
      return password === '' ? _('Password cleared — email-only login') : _('Password updated');
    });

  return (
    <div className='min-h-screen bg-base-200 p-4 text-base-content md:p-8'>
      <header className='mx-auto mb-6 flex max-w-5xl items-center justify-between gap-2'>
        <div>
          <h1 className='text-2xl font-bold'>{_('Admin Console')}</h1>
          <p className='text-sm opacity-70'>
            {selfEmail ? _('Signed in as {{email}}', { email: selfEmail }) : _('Signed in')}
          </p>
        </div>
        <a href='/library' className='btn btn-ghost btn-sm'>
          {_('Back to reading')}
        </a>
      </header>

      <main className='mx-auto max-w-5xl'>
        {(error || notice) && (
          <div
            className={`alert ${error ? 'alert-error' : 'alert-success'} mb-4 py-2 text-sm`}
            onClick={() => {
              setError('');
              setNotice('');
            }}
          >
            <span>{error || notice}</span>
          </div>
        )}

        <div role='tablist' className='tabs-box tabs mb-4 w-fit'>
          {(
            [
              ['overview', _('Overview')],
              ['users', _('Users')],
              ['books', _('Books')],
              ['monitor', _('Server Monitor')],
              ['maintenance', _('Maintenance')],
            ] as [Tab, string][]
          ).map(([key, label]) => (
            <button
              key={key}
              role='tab'
              className={`tab ${tab === key ? 'tab-active' : ''}`}
              onClick={() => setTab(key)}
            >
              {label}
            </button>
          ))}
        </div>

        {tab === 'overview' && <StatsSection />}

        {tab === 'users' && (
          <section>
            <div className='card bg-base-100 mb-4 shadow'>
              <div className='card-body gap-3 p-4'>
                <h2 className='card-title text-base'>{_('Create user')}</h2>
                <div className='flex flex-wrap items-center gap-2'>
                  <input
                    className='input input-bordered input-sm w-56'
                    placeholder={_('Email')}
                    value={newEmail}
                    onChange={(e) => setNewEmail(e.target.value)}
                  />
                  <input
                    className='input input-bordered input-sm w-56'
                    placeholder={_('Password')}
                    type='text'
                    value={newPassword}
                    onChange={(e) => setNewPassword(e.target.value)}
                  />
                  <select
                    className='select select-bordered select-sm'
                    value={newRole}
                    onChange={(e) => setNewRole(e.target.value as 'user' | 'admin')}
                  >
                    <option value='user'>{_('User')}</option>
                    <option value='admin'>{_('Admin')}</option>
                  </select>
                  <button
                    className='btn btn-primary btn-sm'
                    disabled={creating}
                    onClick={createUser}
                  >
                    {_('Create')}
                  </button>
                </div>
                <p className='text-xs opacity-60'>
                  {_(
                    'Empty password = a random one is generated and shown once. Check the server log for passwordless accounts.',
                  )}
                </p>
              </div>
            </div>

            <div className='card bg-base-100 shadow'>
              <div className='overflow-x-auto p-2'>
                <table className='table-zebra table table-sm'>
                  <thead>
                    <tr>
                      <th>{_('Email')}</th>
                      <th>{_('Role')}</th>
                      <th>{_('Status')}</th>
                      <th>{_('Created')}</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {(users ?? []).map((u) => (
                      <tr key={u.id}>
                        <td className='font-mono text-xs'>{u.email}</td>
                        <td>
                          <span
                            className={`badge badge-sm ${u.role === 'admin' ? 'badge-primary' : 'badge-ghost'}`}
                          >
                            {u.role === 'admin' ? _('Admin') : _('User')}
                          </span>
                        </td>
                        <td>
                          <span
                            className={`badge badge-sm ${u.disabled ? 'badge-warning' : 'badge-success'}`}
                          >
                            {u.disabled ? _('Disabled') : _('Enabled')}
                          </span>
                        </td>
                        <td className='text-xs opacity-70'>{formatCreated(u.createdAt)}</td>
                        <td className='whitespace-nowrap text-right'>
                          <button
                            className='btn btn-ghost btn-xs'
                            onClick={() =>
                              run(async () => {
                                await adminApi.setUserState(u.email, !u.disabled);
                                setUsers(await adminApi.listUsers());
                                return u.disabled ? _('Enabled') : _('Disabled');
                              })
                            }
                          >
                            {u.disabled ? _('Enable') : _('Disable')}
                          </button>
                          {!isSelf(u.email) && (
                            <>
                              <button
                                className='btn btn-ghost btn-xs'
                                onClick={() =>
                                  run(async () => {
                                    await adminApi.setUserRole(
                                      u.email,
                                      u.role === 'admin' ? 'user' : 'admin',
                                    );
                                    setUsers(await adminApi.listUsers());
                                    return _('Role updated');
                                  })
                                }
                              >
                                {u.role === 'admin' ? _('Make user') : _('Make admin')}
                              </button>
                              <button
                                className='btn btn-ghost btn-xs'
                                onClick={() => {
                                  setResetInput('');
                                  setDialog({ kind: 'reset-password', email: u.email });
                                }}
                              >
                                {_('Reset password')}
                              </button>
                              <button
                                className='btn btn-ghost btn-xs text-error'
                                onClick={() => setDialog({ kind: 'delete-user', email: u.email })}
                              >
                                {_('Delete')}
                              </button>
                            </>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {users && users.length === 0 && (
                  <p className='p-4 text-center text-sm opacity-60'>{_('No users')}</p>
                )}
              </div>
            </div>
          </section>
        )}

        {tab === 'books' && (
          <section>
            <label className='mb-2 flex w-fit items-center gap-2 text-sm'>
              <input
                type='checkbox'
                className='checkbox checkbox-sm'
                checked={ungroupedOnly}
                onChange={(e) => setUngroupedOnly(e.target.checked)}
              />
              {_('Ungrouped only')}
            </label>
            <div className='card bg-base-100 shadow'>
              <div className='overflow-x-auto p-2'>
                <table className='table-zebra table table-sm'>
                  <thead>
                    <tr>
                      <th>{_('Title')}</th>
                      <th>{_('Author')}</th>
                      <th>{_('Language')}</th>
                      <th>{_('Hash')}</th>
                      <th>{_('Added by')}</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {(books ?? [])
                      .filter((b) => !ungroupedOnly || !b.workId)
                      .map((b) => (
                        <tr key={b.hash}>
                          <td>{b.title || b.hash.slice(0, 8)}</td>
                          <td className='text-xs'>{b.author}</td>
                          <td className='text-xs opacity-70'>{b.language}</td>
                          <td className='font-mono text-xs opacity-60'>{b.hash.slice(0, 10)}…</td>
                          <td className='font-mono text-xs opacity-60'>{b.addedBy}</td>
                          <td className='text-right'>
                            <button
                              className='btn btn-ghost btn-xs text-error'
                              onClick={() => setDialog({ kind: 'delete-book', book: b })}
                            >
                              {_('Delete')}
                            </button>
                          </td>
                        </tr>
                      ))}
                  </tbody>
                </table>
                {books && books.length === 0 && (
                  <p className='p-4 text-center text-sm opacity-60'>{_('No books')}</p>
                )}
              </div>
            </div>
          </section>
        )}

        {tab === 'monitor' && <HostMonitorSection />}

        {tab === 'maintenance' && (
          <section className='card bg-base-100 max-w-xl shadow'>
            <div className='card-body gap-4 p-4'>
              <div>
                <h2 className='mb-1 text-base font-semibold'>{_('Blob storage')}</h2>
                <p className='mb-2 text-xs opacity-60'>
                  {_('Deletes content-addressed blobs no longer linked by any stored file.')}
                </p>
                <button
                  className='btn btn-sm'
                  onClick={() =>
                    run(async () => {
                      const res = await adminApi.gc();
                      const message = _('Removed {{count}} blob(s)', {
                        count: res.removed,
                      });
                      setGcResult(message);
                      return message;
                    })
                  }
                >
                  {_('Run blob GC')}
                </button>
                {gcResult && <p className='mt-2 text-xs opacity-70'>{gcResult}</p>}
              </div>
              <div>
                <h2 className='mb-1 text-base font-semibold'>{_('Works grouping')}</h2>
                <p className='mb-2 text-xs opacity-60'>
                  {_(
                    'Groups ungrouped catalog books by title + author via Open Library (needs internet). Fine-grained grouping stays on the server page at /admin.',
                  )}
                </p>
                <button
                  className='btn btn-sm'
                  onClick={() =>
                    run(async () => {
                      const res = await adminApi.enrich();
                      const message = _('Grouped {{count}} book(s), {{failed}} failed', {
                        count: res.enriched,
                        failed: res.failed.length,
                      });
                      setEnrichResult(message);
                      setBooks(await adminApi.listBooks());
                      return message;
                    })
                  }
                >
                  {_('Auto-group books (Open Library)')}
                </button>
                {enrichResult && <p className='mt-2 text-xs opacity-70'>{enrichResult}</p>}
              </div>
            </div>
          </section>
        )}
      </main>

      {dialog?.kind === 'show-password' && (
        <Modal onClose={() => setDialog(null)}>
          <h3 className='mb-2 text-lg font-bold'>
            {dialog.generated ? _('Generated password') : _('Password')}
          </h3>
          <p className='mb-3 font-mono text-sm'>{dialog.password}</p>
          <p className='mb-4 text-sm text-error'>
            {_('Copy it now — it will not be shown again.')}
          </p>
          <div className='modal-action'>
            <button
              className='btn btn-primary btn-sm'
              onClick={() => {
                navigator.clipboard?.writeText(dialog.password).catch(() => {});
                setDialog(null);
              }}
            >
              {_('Copy and close')}
            </button>
          </div>
        </Modal>
      )}

      {dialog?.kind === 'delete-user' && (
        <Modal onClose={() => setDialog(null)}>
          <h3 className='mb-2 text-lg font-bold'>{_('Delete user')}</h3>
          <p className='mb-4 text-sm'>
            {_('Delete {{email}}? Their synced data and stored file copies are removed.', {
              email: dialog.email,
            })}
          </p>
          <div className='modal-action'>
            <button className='btn btn-ghost btn-sm' onClick={() => setDialog(null)}>
              {_('Cancel')}
            </button>
            <button
              className='btn btn-error btn-sm'
              onClick={() =>
                run(async () => {
                  await adminApi.deleteUser(dialog.email);
                  setDialog(null);
                  setUsers(await adminApi.listUsers());
                  setBooks(await adminApi.listBooks());
                  return _('User deleted');
                })
              }
            >
              {_('Delete')}
            </button>
          </div>
        </Modal>
      )}

      {dialog?.kind === 'reset-password' && (
        <Modal onClose={() => setDialog(null)}>
          <h3 className='mb-2 text-lg font-bold'>{_('Reset password')}</h3>
          <p className='mb-2 font-mono text-xs opacity-70'>{dialog.email}</p>
          <input
            className='input input-bordered input-sm mb-1 w-full'
            type='text'
            placeholder={_('New password — leave empty to generate a random one')}
            value={resetInput}
            onChange={(e) => setResetInput(e.target.value)}
          />
          <p className='mb-4 text-xs opacity-60'>
            {_(
              'The new password is shown once after saving. Passwordless accounts are a server-CLI capability.',
            )}
          </p>
          <div className='modal-action'>
            <button className='btn btn-ghost btn-sm' onClick={() => setDialog(null)}>
              {_('Cancel')}
            </button>
            <button
              className='btn btn-primary btn-sm'
              onClick={() => submitReset(dialog.email, resetInput.trim())}
            >
              {_('Save')}
            </button>
          </div>
        </Modal>
      )}

      {dialog?.kind === 'delete-book' && (
        <Modal onClose={() => setDialog(null)}>
          <h3 className='mb-2 text-lg font-bold'>{_('Delete book')}</h3>
          <p className='mb-4 text-sm'>
            {_('Delete “{{title}}” from the shared library? All stored copies are removed.', {
              title: dialog.book.title || dialog.book.hash.slice(0, 10),
            })}
          </p>
          <div className='modal-action'>
            <button className='btn btn-ghost btn-sm' onClick={() => setDialog(null)}>
              {_('Cancel')}
            </button>
            <button
              className='btn btn-error btn-sm'
              onClick={() =>
                run(async () => {
                  await adminApi.deleteBook(dialog.book.hash);
                  setDialog(null);
                  setBooks(await adminApi.listBooks());
                  return _('Book deleted');
                })
              }
            >
              {_('Delete')}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function Centered({ children }: { children: React.ReactNode }) {
  return (
    <div className='flex min-h-screen flex-col items-center justify-center bg-base-200 text-base-content'>
      {children}
    </div>
  );
}

function Modal({ children, onClose }: { children: React.ReactNode; onClose: () => void }) {
  return (
    <div className='modal modal-open'>
      <div className='modal-box max-w-md'>{children}</div>
      <button className='modal-backdrop' onClick={onClose} aria-label='close' />
    </div>
  );
}
