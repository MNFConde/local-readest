'use client';

// SELFHOSTED (fork): recommended-inbox entry (ROADMAP ⑬). Self-gating menu
// item + modal listing the account's pending book offers; each can be
// accepted (files hard-linked in, snapshot landed through the LWW upsert) or
// declined (nothing lands). No polling loop: the pending count refreshes on
// mount, on dialog open, and after each decision. Self-contained by design:
// daisyUI classes and fork-owned services only, no upstream business imports.

import React, { useEffect, useState } from 'react';
import clsx from 'clsx';
import { MdClose } from 'react-icons/md';
import { PiTray } from 'react-icons/pi';

import { useTranslation } from '@/hooks/useTranslation';
import MenuItem from '@/components/MenuItem';
import { SELFHOSTED } from '@/utils/supabase';
import {
  PushInboxItem,
  SelfhostPushError,
  acceptPush,
  declinePush,
  listInbox,
} from '@/services/selfhostPush';

interface SelfhostInboxMenuItemProps {
  /** Called when the dialog opens, so the host dropdown can close. */
  onNavigated?: () => void;
}

const SelfhostInboxMenuItem: React.FC<SelfhostInboxMenuItemProps> = ({ onNavigated }) => {
  const _ = useTranslation('selfhost');
  const [signedIn, setSignedIn] = useState(false);
  const [showDialog, setShowDialog] = useState(false);
  const [items, setItems] = useState<PushInboxItem[]>([]);
  const [decidingId, setDecidingId] = useState<number | null>(null);
  const [summary, setSummary] = useState('');
  const [error, setError] = useState('');

  // The stored token is re-read after every render of the host menu: the
  // menu outlives a login in the same page session (same pattern as
  // AdminConsoleMenuItem).
  useEffect(() => {
    if (!SELFHOSTED) return;
    setSignedIn(!!window.localStorage.getItem('token'));
  });

  // Badge + dialog list share one fetch; no polling loop — refresh happens
  // on sign-in, on dialog open, and after each accept/decline. (The catch
  // fallback string is intentionally not a dependency.)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!signedIn) return;
    let alive = true;
    setError('');
    listInbox()
      .then((fresh) => {
        if (alive) setItems(fresh);
      })
      .catch((e) => {
        if (alive) {
          setError(e instanceof SelfhostPushError ? e.message : _('Push failed'));
        }
      });
    return () => {
      alive = false;
    };
  }, [signedIn, showDialog]);

  if (!SELFHOSTED || !signedIn) return null;

  const refresh = async () => {
    try {
      setItems(await listInbox());
    } catch (e) {
      setError(e instanceof SelfhostPushError ? e.message : _('Push failed'));
    }
  };

  const decide = async (id: number, action: 'accept' | 'decline') => {
    if (decidingId !== null) return;
    setDecidingId(id);
    setError('');
    setSummary('');
    try {
      if (action === 'accept') {
        const result = await acceptPush(id);
        setSummary(_('Accepted, linked {{files}} file(s).', { files: result.filesLinked }));
      } else {
        await declinePush(id);
      }
      await refresh();
    } catch (e) {
      setError(e instanceof SelfhostPushError ? e.message : _('Push failed'));
    } finally {
      setDecidingId(null);
    }
  };

  return (
    <>
      <MenuItem
        label={_('Recommended inbox') + (items.length > 0 ? ` (${items.length})` : '')}
        Icon={PiTray}
        onClick={() => {
          setSummary('');
          setShowDialog(true);
          onNavigated?.();
        }}
      />
      {showDialog && (
        <div className='fixed inset-0 z-50 flex items-center justify-center'>
          <div
            className={clsx(
              'modal-box bg-base-100 overflow-y-auto rounded-2xl shadow-xl',
              'max-h-[85%] w-[95%] min-w-64 max-w-[440px] p-6 sm:w-[70%]',
            )}
          >
            <button
              onClick={() => setShowDialog(false)}
              aria-label={_('Close')}
              className='btn btn-ghost btn-xs absolute right-2 top-2 rounded-lg p-1'
            >
              <MdClose size={16} />
            </button>
            <h2 className='text-center text-lg font-bold'>{_('Recommended inbox')}</h2>

            <div className='mt-4'>
              {items.length === 0 ? (
                <p className='mt-2 text-center text-sm text-neutral-500'>
                  {_('No pending recommendations')}
                </p>
              ) : (
                <ul className='mt-1 flex flex-col gap-3'>
                  {items.map((item) => (
                    <li key={item.id} className='rounded-lg border border-base-200 p-3 text-sm'>
                      <p className='font-bold'>{item.title || item.bookHash}</p>
                      <p className='mt-1 text-xs text-neutral-500'>
                        {_('From {{user}}', { user: item.from })}
                      </p>
                      {item.message && (
                        <p className='mt-1 text-xs italic text-neutral-500'>{item.message}</p>
                      )}
                      <div className='mt-2 flex items-center justify-end gap-2'>
                        <button
                          onClick={() => decide(item.id, 'decline')}
                          disabled={decidingId !== null}
                          className='btn btn-ghost btn-sm rounded-lg font-normal normal-case'
                        >
                          {decidingId === item.id ? _('Working…') : _('Decline')}
                        </button>
                        <button
                          onClick={() => decide(item.id, 'accept')}
                          disabled={decidingId !== null}
                          className='btn btn-primary btn-sm rounded-lg font-normal normal-case'
                        >
                          {decidingId === item.id ? _('Working…') : _('Accept')}
                        </button>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
              {summary && <p className='mt-3 text-center text-xs text-neutral-500'>{summary}</p>}
            </div>

            {error && <p className='mt-3 text-center text-xs error'>{error}</p>}
          </div>
        </div>
      )}
    </>
  );
};

export default SelfhostInboxMenuItem;
