// SELFHOSTED (fork): account-scoped shelf bookkeeping, extracted verbatim
// from services/libraryService.ts and app/library/page.tsx so both shared
// files stay near upstream text. Every account shelves its own
// `library_<uid>.json`; guests and every non-self-hosted build keep the
// device-wide `library.json`.

import { useEffect, useRef } from 'react';
import { SELFHOSTED } from '@/utils/supabase';
import { jwtToUser } from '@/services/selfhostedAuth';
import { useAuth } from '@/context/AuthContext';
import { useEnv } from '@/context/EnvContext';
import { useLibraryStore } from '@/store/libraryStore';

// The account scope the on-disk shelf was last loaded or saved for. The
// in-memory library store can lag behind a login/logout until the shelf is
// re-read, so sync paths consult `isShelfScopeCurrent` before pushing or
// merging — otherwise the previous scope's books ride into the new account
// (the store still holds them at the instant the user flips).
let shelfScope: string | null | undefined = undefined;

export const setShelfScope = (userId: string | null | undefined) => {
  shelfScope = userId ?? null;
};

export const isShelfScopeCurrent = (): boolean => {
  if (!SELFHOSTED || typeof window === 'undefined') return true;
  const uid = jwtToUser(localStorage.getItem('token'))?.id ?? null;
  return shelfScope === uid;
};

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
