'use client';

// SELFHOSTED (fork): book push entry (ROADMAP ⑬). Self-gating menu item +
// modal: offer one of your uploaded books (file + metadata snapshot, never
// notes or progress) to another account on this server; the recipient finds
// it in the Recommended Inbox. Self-contained by design: daisyUI classes and
// fork-owned services only, no upstream business imports (same isolation rule
// as SelfhostArchiveMenuItem / GuestShelfImport).

import React, { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { MdClose } from 'react-icons/md';
import { PiPaperPlaneTilt } from 'react-icons/pi';

import { useTranslation } from '@/hooks/useTranslation';
import MenuItem from '@/components/MenuItem';
import { SELFHOSTED } from '@/utils/supabase';
import { useLibraryStore } from '@/store/libraryStore';
import { SelfhostPushError, createPush } from '@/services/selfhostPush';

interface SelfhostPushMenuItemProps {
  /** Called when the dialog opens, so the host dropdown can close. */
  onNavigated?: () => void;
}

const SelfhostPushMenuItem: React.FC<SelfhostPushMenuItemProps> = ({ onNavigated }) => {
  const _ = useTranslation('selfhost');
  const [signedIn, setSignedIn] = useState(false);
  const [showDialog, setShowDialog] = useState(false);
  const [recipient, setRecipient] = useState('');
  const [bookHash, setBookHash] = useState('');
  const [message, setMessage] = useState('');
  const [pushing, setPushing] = useState(false);
  const [pushedTo, setPushedTo] = useState('');
  const [error, setError] = useState('');

  // Only books actually uploaded to the server can be offered — a local-only
  // file has no server-side copy to link. Feed books never have uploadedAt,
  // so this filter doubles as the feed exclusion.
  const pushableBooks = useMemo(
    () =>
      useLibraryStore
        .getState()
        .library.filter((b) => b.uploadedAt && !b.deletedAt)
        .sort((a, b) => (a.title || '').localeCompare(b.title || '')),
    // Recompute when the dialog reopens; the shelf is stable meanwhile.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [showDialog],
  );

  // The stored token is re-read after every render of the host menu: the
  // menu outlives a login in the same page session (same pattern as
  // AdminConsoleMenuItem).
  useEffect(() => {
    if (!SELFHOSTED) return;
    setSignedIn(!!window.localStorage.getItem('token'));
  });

  if (!SELFHOSTED || !signedIn) return null;

  const handlePush = async () => {
    if (pushing || !recipient.trim() || !bookHash) return;
    setPushing(true);
    setError('');
    try {
      await createPush({ to: recipient.trim(), bookHash, message: message.trim() || undefined });
      setPushedTo(recipient.trim());
      setRecipient('');
      setMessage('');
    } catch (e) {
      setError(e instanceof SelfhostPushError ? e.message : _('Push failed'));
    } finally {
      setPushing(false);
    }
  };

  return (
    <>
      <MenuItem
        label={_('Push a book')}
        Icon={PiPaperPlaneTilt}
        onClick={() => {
          setBookHash(pushableBooks[0]?.hash ?? '');
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
            <h2 className='text-center text-lg font-bold'>{_('Push a book')}</h2>

            <div className='mt-4'>
              <label htmlFor='selfhost-push-recipient' className='text-sm font-bold'>
                {_('Recipient account')}
              </label>
              <input
                id='selfhost-push-recipient'
                type='text'
                value={recipient}
                onChange={(e) => {
                  setRecipient(e.target.value);
                  setPushedTo('');
                  setError('');
                }}
                className='input input-bordered input-sm mt-2 w-full'
              />
              <label htmlFor='selfhost-push-book' className='mt-4 block text-sm font-bold'>
                {_('Pick a book')}
              </label>
              {pushableBooks.length === 0 ? (
                <p className='mt-2 text-xs text-neutral-500'>{_('No uploaded books to push')}</p>
              ) : (
                <select
                  id='selfhost-push-book'
                  value={bookHash}
                  onChange={(e) => setBookHash(e.target.value)}
                  className='select select-bordered select-sm mt-2 w-full'
                >
                  {pushableBooks.map((book) => (
                    <option key={book.hash} value={book.hash}>
                      {book.title || book.hash}
                    </option>
                  ))}
                </select>
              )}
              <label htmlFor='selfhost-push-message' className='mt-4 block text-sm font-bold'>
                {_('Message (optional)')}
              </label>
              <input
                id='selfhost-push-message'
                type='text'
                value={message}
                maxLength={500}
                onChange={(e) => setMessage(e.target.value)}
                className='input input-bordered input-sm mt-2 w-full'
              />
              <div className='mt-4 flex items-center justify-end'>
                <button
                  onClick={handlePush}
                  disabled={pushing || !recipient.trim() || !bookHash}
                  className='btn btn-primary btn-sm rounded-lg font-normal normal-case'
                >
                  {pushing ? _('Pushing…') : _('Push')}
                </button>
              </div>
              {pushedTo && (
                <p className='mt-2 text-center text-xs text-neutral-500'>
                  {_('Pushed to {{user}}', { user: pushedTo })}
                </p>
              )}
            </div>

            {error && <p className='mt-3 text-center text-xs error'>{error}</p>}
          </div>
        </div>
      )}
    </>
  );
};

export default SelfhostPushMenuItem;
