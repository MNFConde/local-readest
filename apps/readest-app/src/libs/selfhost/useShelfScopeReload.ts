// SELFHOSTED (fork): client-side hook that re-reads the shelf when the
// account scope flips. Split out of shelfScope.ts because that module is
// also imported by server-safe services (appService), and a module pulling
// in react hooks cannot enter a React Server Component graph.

import { useEffect, useRef } from 'react';
import { SELFHOSTED } from '@/utils/supabase';
import { useAuth } from '@/context/AuthContext';
import { useEnv } from '@/context/EnvContext';
import { useLibraryStore } from '@/store/libraryStore';

// The on-disk shelf switches with the signed-in account, and a login/logout
// can land while the library page stays mounted (in-app login dialog, logout
// from settings). Re-read the shelf on scope change so the new account's
// books show; without it the previous scope's shelf lingers until a remount.
// The store's initial value matches the first render, so the effect only
// fires on real transitions — the cold load is initLibrary's. No-ops in
// non-self-hosted builds.
export const useShelfScopeReload = () => {
  const { user } = useAuth();
  const { envConfig } = useEnv();
  const setLibrary = useLibraryStore((state) => state.setLibrary);
  const scopedUserId = SELFHOSTED ? (user?.id ?? null) : null;
  const prevScopedUserIdRef = useRef<string | null>(scopedUserId);
  useEffect(() => {
    if (prevScopedUserIdRef.current === scopedUserId) return;
    prevScopedUserIdRef.current = scopedUserId;
    let cancelled = false;
    const reloadShelf = async () => {
      const appService = await envConfig.getAppService();
      const library = await appService.loadLibraryBooks();
      if (!cancelled) setLibrary(library);
    };
    reloadShelf().catch((error) =>
      console.error('Failed to reload the shelf after account switch:', error),
    );
    return () => {
      cancelled = true;
    };
  }, [scopedUserId, envConfig, setLibrary]);
};
