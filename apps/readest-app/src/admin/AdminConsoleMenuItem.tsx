'use client';

import React, { useEffect, useState } from 'react';
import { useRouter } from 'next/navigation';
import { PiShieldStar } from 'react-icons/pi';

import { useTranslation } from '@/hooks/useTranslation';
import MenuItem from '@/components/MenuItem';
import { SELFHOSTED } from '@/utils/supabase';
import { jwtRole } from '@/services/selfhostedAuth';

interface AdminConsoleMenuItemProps {
  /** Called once the navigation starts, so the host dropdown can close. */
  onNavigated?: () => void;
}

/**
 * Library settings-menu entry into the admin console page (`/admin`).
 *
 * Visibility mirrors the server's admin gate client-side: only a self-hosted
 * build whose stored JWT carries the `admin` role renders the entry. Like
 * every client-side role check this is cosmetic — /api/admin enforces the
 * role server-side on every request.
 *
 * The stored token is re-read after every render of the host menu: the menu
 * outlives a login in the same page session, and the auth context update
 * that rewrites `localStorage.token` re-renders the menu, so the entry
 * appears (or disappears) without a page reload.
 */
const AdminConsoleMenuItem: React.FC<AdminConsoleMenuItemProps> = ({ onNavigated }) => {
  const _ = useTranslation('selfhost');
  const router = useRouter();
  const [isAdmin, setIsAdmin] = useState(false);

  useEffect(() => {
    if (!SELFHOSTED) return;
    setIsAdmin(jwtRole(window.localStorage.getItem('token')) === 'admin');
  });

  if (!SELFHOSTED || !isAdmin) return null;

  return (
    <MenuItem
      label={_('Admin Console')}
      Icon={PiShieldStar}
      onClick={() => {
        router.push('/admin');
        onNavigated?.();
      }}
    />
  );
};

export default AdminConsoleMenuItem;
