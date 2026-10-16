'use client';

// SELFHOSTED (fork): guest-shelf import. Each account shelves its own
// `library_<uid>.json` while the device-wide guest shelf stays in
// `library.json`; this dialog lets a signed-in account pull books from the
// guest shelf into its own. Book files live in hash-keyed dirs shared by
// every scope, so importing shelves the existing entry without copying any
// file. Self-contained by design: the only thing it asks of the library page
// is the mount point.

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { MdClose, MdDownload } from 'react-icons/md';

import { Book } from '@/types/book';
import { useEnv } from '@/context/EnvContext';
import { useAuth } from '@/context/AuthContext';
import { useTranslation } from '@/hooks/useTranslation';
import { useLibraryStore } from '@/store/libraryStore';
import { SELFHOSTED } from '@/utils/supabase';
import { eventDispatcher } from '@/utils/event';

const GuestShelfImport: React.FC = () => {
  const _ = useTranslation();
  const { envConfig, appService } = useEnv();
  const { user } = useAuth();
  const libraryBooks = useLibraryStore((state) => state.library);

  const [importable, setImportable] = useState<Book[]>([]);
  const [dismissed, setDismissed] = useState(false);
  const [showDialog, setShowDialog] = useState(false);
  const [selectedHashes, setSelectedHashes] = useState<Set<string>>(new Set());
  const [importing, setImporting] = useState(false);

  // Books on the guest shelf this account could still import: not tombstoned
  // there, and not already shelved here (a hash present in any state counts —
  // resurrecting an account-deleted book via re-import would surprise).
  useEffect(() => {
    if (!SELFHOSTED || !user) return;
    let cancelled = false;
    (async () => {
      try {
        const service = appService ?? (await envConfig.getAppService());
        if (!service?.peekLibraryBooks) return;
        const guestBooks = await service.peekLibraryBooks(null);
        if (cancelled) return;
        const knownHashes = new Set(useLibraryStore.getState().library.map((book) => book.hash));
        setImportable(guestBooks.filter((book) => !book.deletedAt && !knownHashes.has(book.hash)));
      } catch (error) {
        console.warn('Failed to check the guest shelf for importable books:', error);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [appService, envConfig, user, libraryBooks]);

  const openDialog = useCallback(() => {
    setSelectedHashes(new Set(importable.map((book) => book.hash)));
    setShowDialog(true);
  }, [importable]);

  const toggleBook = useCallback((hash: string) => {
    setSelectedHashes((prev) => {
      const next = new Set(prev);
      if (next.has(hash)) {
        next.delete(hash);
      } else {
        next.add(hash);
      }
      return next;
    });
  }, []);

  const handleImport = useCallback(async () => {
    if (!appService || selectedHashes.size === 0) return;
    setImporting(true);
    try {
      const selected = importable.filter((book) => selectedHashes.has(book.hash));
      for (const book of selected) {
        book.coverImageUrl = await appService.generateCoverImageUrl(book);
      }
      // Merge into the current account's shelf and persist through the
      // scoped save funnel (updateBooks saves via appService.saveLibraryBooks).
      await useLibraryStore.getState().updateBooks(envConfig, selected);
      eventDispatcher.dispatch('toast', {
        type: 'success',
        message: _('Imported {{count}} book(s) from the guest shelf', {
          count: selected.length,
        }),
      });
      setShowDialog(false);
    } catch (error) {
      console.error('Guest shelf import failed:', error);
      eventDispatcher.dispatch('toast', {
        type: 'error',
        message: _('Failed to import from the guest shelf.'),
      });
    } finally {
      setImporting(false);
    }
  }, [appService, envConfig, importable, selectedHashes]);

  const selectedCount = useMemo(() => selectedHashes.size, [selectedHashes]);

  if (!SELFHOSTED || !user || dismissed || importable.length === 0) return null;

  return (
    <>
      <div
        className={clsx(
          'fixed bottom-4 right-4 z-40 flex items-center gap-2 rounded-xl bg-base-100 p-3',
          'shadow-lg ring-1 ring-neutral/10 sm:right-8 sm:bottom-8',
        )}
      >
        <span className='text-sm'>
          {_('Your guest shelf has {{count}} book(s) not in this account.', {
            count: importable.length,
          })}
        </span>
        <button
          onClick={openDialog}
          className='btn btn-primary btn-sm rounded-lg font-normal normal-case'
        >
          <MdDownload size={16} />
          {_('Import')}
        </button>
        <button
          onClick={() => setDismissed(true)}
          aria-label={_('Close')}
          className='btn btn-ghost btn-xs rounded-lg p-1'
        >
          <MdClose size={16} />
        </button>
      </div>

      {showDialog && (
        <div className='fixed inset-0 z-50 flex items-center justify-center'>
          <div
            className={clsx(
              'modal-box bg-base-100 overflow-y-auto rounded-2xl shadow-xl',
              'max-h-[85%] w-[95%] min-w-64 max-w-[440px] p-6 sm:w-[70%]',
            )}
          >
            <h2 className='text-center text-lg font-bold'>{_('Import from Guest Shelf')}</h2>
            <p className='mt-2 text-center text-xs text-neutral-500'>
              {_(
                'Importing shelves the book for this account; files are shared on the device and are not copied.',
              )}
            </p>
            <ul className='mt-4 max-h-72 overflow-y-auto'>
              {importable.map((book) => (
                <li key={book.hash}>
                  <label className='flex cursor-pointer items-center gap-3 rounded-lg p-2 hover:bg-base-200'>
                    <input
                      type='checkbox'
                      className='checkbox checkbox-sm checkbox-primary'
                      checked={selectedHashes.has(book.hash)}
                      onChange={() => toggleBook(book.hash)}
                    />
                    {book.coverImageUrl ? (
                      <img
                        src={book.coverImageUrl}
                        alt=''
                        className='h-12 w-8 rounded object-cover'
                      />
                    ) : (
                      <div className='h-12 w-8 rounded bg-neutral-200' />
                    )}
                    <div className='min-w-0 flex-1'>
                      <p className='truncate text-sm font-medium'>{book.title}</p>
                      {book.author && (
                        <p className='truncate text-xs text-neutral-500'>{book.author}</p>
                      )}
                    </div>
                  </label>
                </li>
              ))}
            </ul>
            <div className='mt-4 flex items-center justify-end gap-2'>
              <button
                onClick={() => setShowDialog(false)}
                className='btn btn-ghost btn-sm rounded-lg font-normal normal-case'
              >
                {_('Cancel')}
              </button>
              <button
                onClick={handleImport}
                disabled={importing || selectedCount === 0}
                className='btn btn-primary btn-sm rounded-lg font-normal normal-case'
              >
                {_('Import {{count}} book(s)', { count: selectedCount })}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
};

export default GuestShelfImport;
