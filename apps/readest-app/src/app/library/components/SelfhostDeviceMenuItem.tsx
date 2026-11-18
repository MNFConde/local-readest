'use client';

// SELFHOSTED (fork): device identity entry (ROADMAP ⑪-P1). Self-gating menu
// item + modal showing this device's {id, key} pair: export a backup file,
// reveal the key, or forget the local record (the server account stays —
// importing the backup brings the identity back). Self-contained by design:
// daisyUI classes and fork-owned modules only, no upstream business imports
// (same isolation rule as SelfhostArchiveMenuItem).

import React, { useEffect, useState } from 'react';
import clsx from 'clsx';
import { MdClose } from 'react-icons/md';
import { PiHardDrives } from 'react-icons/pi';

import { useTranslation } from '@/hooks/useTranslation';
import MenuItem from '@/components/MenuItem';
import { SELFHOSTED } from '@/utils/supabase';
import { getSelfhostedServerUrl } from '@/services/selfhostedServerUrl';
import {
  clearDeviceIdentity,
  downloadDeviceIdentityBackup,
  getDeviceIdentity,
} from '@/libs/selfhost/deviceIdentity';
import type { DeviceIdentity } from '@/libs/selfhost/deviceIdentity';

interface SelfhostDeviceMenuItemProps {
  /** Called when the dialog opens, so the host dropdown can close. */
  onNavigated?: () => void;
}

const SelfhostDeviceMenuItem: React.FC<SelfhostDeviceMenuItemProps> = ({ onNavigated }) => {
  const _ = useTranslation('selfhost');
  const [signedIn, setSignedIn] = useState(false);
  const [hasIdentity, setHasIdentity] = useState(false);
  const [showDialog, setShowDialog] = useState(false);
  const [identity, setIdentity] = useState<DeviceIdentity | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [notice, setNotice] = useState('');
  const [confirmForget, setConfirmForget] = useState(false);

  // The stored token and identity are re-read after every render of the host
  // menu: the menu outlives a login in the same page session, and the
  // identity can appear/disappear via the sign-in screen or Forget (same
  // pattern as AdminConsoleMenuItem).
  useEffect(() => {
    if (!SELFHOSTED) return;
    setSignedIn(!!window.localStorage.getItem('token'));
    setHasIdentity(!!getDeviceIdentity());
  });

  useEffect(() => {
    if (showDialog) setIdentity(getDeviceIdentity());
  }, [showDialog]);

  if (!SELFHOSTED || !signedIn || !hasIdentity) return null;

  const handleBackup = () => {
    if (!identity) return;
    const name = downloadDeviceIdentityBackup(identity, getSelfhostedServerUrl());
    setNotice(_('Backup downloaded: {{name}}', { name }));
  };

  const handleCopy = async () => {
    if (!identity) return;
    try {
      await navigator.clipboard.writeText(identity.key);
      setNotice(_('Copied.'));
    } catch {
      // Clipboard denied: the reveal state still allows manual copying.
    }
  };

  const handleForget = () => {
    clearDeviceIdentity();
    setIdentity(null);
    setHasIdentity(false);
    setShowDialog(false);
  };

  return (
    <>
      <MenuItem
        label={_('Device identity')}
        Icon={PiHardDrives}
        onClick={() => {
          setShowDialog(true);
          onNavigated?.();
        }}
      />
      {showDialog && identity && (
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
            <h2 className='text-center text-lg font-bold'>{_('Device identity')}</h2>

            <p className='mt-3 text-sm leading-relaxed'>
              {_(
                'This device signs in with a generated identity. The key cannot be recovered if lost — keep a backup.',
              )}
            </p>
            <div className='mt-3 rounded-lg px-3 py-2.5 text-sm leading-relaxed eink-bordered'>
              <div className='text-neutral-content/70 break-all font-mono text-xs select-all'>
                {identity.id}
              </div>
              {revealed && (
                <div className='text-neutral-content/70 mt-1 break-all font-mono text-xs select-all'>
                  {identity.key}
                </div>
              )}
            </div>
            <div className='mt-3 flex flex-wrap justify-end gap-2'>
              <button
                onClick={() => setRevealed((r) => !r)}
                className='btn btn-outline btn-sm rounded-lg font-normal normal-case'
              >
                {revealed ? _('Hide key') : _('Show key')}
              </button>
              <button
                onClick={handleCopy}
                className='btn btn-outline btn-sm rounded-lg font-normal normal-case'
              >
                {_('Copy')}
              </button>
              <button
                onClick={handleBackup}
                className='btn btn-primary btn-sm rounded-lg font-normal normal-case'
              >
                {_('Export backup')}
              </button>
            </div>

            <div className='mt-4 border-t border-base-200 pt-4'>
              {confirmForget ? (
                <div className='text-sm leading-relaxed'>
                  <p>
                    {_(
                      'Only the local record is removed; the account stays on the server. Import your backup to sign in again.',
                    )}
                  </p>
                  <div className='mt-2 flex justify-end gap-2'>
                    <button
                      onClick={() => setConfirmForget(false)}
                      className='btn btn-ghost btn-sm rounded-lg font-normal normal-case'
                    >
                      {_('Cancel')}
                    </button>
                    <button
                      onClick={handleForget}
                      className='btn btn-error btn-sm rounded-lg font-normal normal-case'
                    >
                      {_('Forget')}
                    </button>
                  </div>
                </div>
              ) : (
                <div className='text-center'>
                  <button
                    onClick={() => setConfirmForget(true)}
                    className='link text-error text-sm'
                  >
                    {_('Forget this identity on this device')}
                  </button>
                </div>
              )}
            </div>

            {notice && <p className='mt-3 text-center text-xs text-neutral-500'>{notice}</p>}
          </div>
        </div>
      )}
    </>
  );
};

export default SelfhostDeviceMenuItem;
