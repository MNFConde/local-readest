'use client';

// SELFHOSTED (fork): archive transfer entry (ROADMAP ⑫). Self-gating menu
// item + modal: export packs the signed-in account (records, optionally book
// files) into a portable tar the user keeps or carries to another server;
// import merges one back into ANY account on this server through the regular
// LWW upserts, so newer data on either side always wins. Self-contained by
// design: daisyUI classes and fork-owned services only, no upstream business
// imports (same isolation rule as src/admin and GuestShelfImport).

import React, { useEffect, useState } from 'react';
import clsx from 'clsx';
import { MdClose } from 'react-icons/md';
import { PiArchive } from 'react-icons/pi';

import { useTranslation } from '@/hooks/useTranslation';
import MenuItem from '@/components/MenuItem';
import { SELFHOSTED } from '@/utils/supabase';
import {
  ArchiveImportSummary,
  ArchiveScope,
  SelfhostArchiveError,
  exportArchive,
  importArchive,
} from '@/services/selfhostArchive';

interface SelfhostArchiveMenuItemProps {
  /** Called when the dialog opens, so the host dropdown can close. */
  onNavigated?: () => void;
}

const SelfhostArchiveMenuItem: React.FC<SelfhostArchiveMenuItemProps> = ({ onNavigated }) => {
  const _ = useTranslation('selfhost');
  const [signedIn, setSignedIn] = useState(false);
  const [showDialog, setShowDialog] = useState(false);
  const [scope, setScope] = useState<ArchiveScope>('full');
  const [exporting, setExporting] = useState(false);
  const [file, setFile] = useState<File | null>(null);
  const [importing, setImporting] = useState(false);
  const [summary, setSummary] = useState<ArchiveImportSummary | null>(null);
  const [error, setError] = useState('');

  // The stored token is re-read after every render of the host menu: the
  // menu outlives a login in the same page session (same pattern as
  // AdminConsoleMenuItem).
  useEffect(() => {
    if (!SELFHOSTED) return;
    setSignedIn(!!window.localStorage.getItem('token'));
  });

  if (!SELFHOSTED || !signedIn) return null;

  const handleExport = async () => {
    if (exporting) return;
    setExporting(true);
    setError('');
    try {
      await exportArchive(scope);
    } catch (e) {
      setError(e instanceof SelfhostArchiveError ? e.message : _('Export failed'));
    } finally {
      setExporting(false);
    }
  };

  const handleImport = async () => {
    if (importing || !file) return;
    setImporting(true);
    setError('');
    setSummary(null);
    try {
      setSummary(await importArchive(file));
      setFile(null);
    } catch (e) {
      setError(e instanceof SelfhostArchiveError ? e.message : _('Import failed'));
    } finally {
      setImporting(false);
    }
  };

  return (
    <>
      <MenuItem
        label={_('Archive Transfer')}
        Icon={PiArchive}
        onClick={() => {
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
            <h2 className='text-center text-lg font-bold'>{_('Archive Transfer')}</h2>

            <div className='mt-4'>
              <h3 className='text-sm font-bold'>{_('Export archive')}</h3>
              <label className='mt-2 flex cursor-pointer items-center gap-2 rounded-lg p-2 text-sm hover:bg-base-200'>
                <input
                  type='radio'
                  name='archive-scope'
                  className='radio radio-primary radio-sm'
                  checked={scope === 'full'}
                  onChange={() => setScope('full')}
                />
                {_('Full archive (records + book files)')}
              </label>
              <label className='flex cursor-pointer items-center gap-2 rounded-lg p-2 text-sm hover:bg-base-200'>
                <input
                  type='radio'
                  name='archive-scope'
                  className='radio radio-primary radio-sm'
                  checked={scope === 'metadata'}
                  onChange={() => setScope('metadata')}
                />
                {_('Metadata only (records, no book files)')}
              </label>
              <div className='mt-2 flex items-center justify-end'>
                <button
                  onClick={handleExport}
                  disabled={exporting}
                  className='btn btn-primary btn-sm rounded-lg font-normal normal-case'
                >
                  {exporting ? _('Preparing export…') : _('Export archive')}
                </button>
              </div>
            </div>

            <div className='mt-4 border-t border-base-200 pt-4'>
              <h3 className='text-sm font-bold'>{_('Import archive')}</h3>
              <input
                type='file'
                accept='.tar,application/x-tar'
                onChange={(e) => {
                  setFile(e.target.files?.[0] ?? null);
                  setSummary(null);
                  setError('');
                }}
                className='file-input file-input-bordered file-input-sm mt-2 w-full'
              />
              <div className='mt-2 flex items-center justify-end'>
                <button
                  onClick={handleImport}
                  disabled={importing || !file}
                  className='btn btn-primary btn-sm rounded-lg font-normal normal-case'
                >
                  {importing ? _('Importing…') : _('Import archive')}
                </button>
              </div>
              {summary && (
                <p className='mt-2 text-center text-xs text-neutral-500'>
                  {_('Imported {{books}} book(s), linked {{files}} file(s).', {
                    books: summary.books,
                    files: summary.filesLinked,
                  })}
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

export default SelfhostArchiveMenuItem;
