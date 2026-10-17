// SELFHOSTED (fork): account-scoped shelf bookkeeping, extracted verbatim
// from services/libraryService.ts and app/library/page.tsx so both shared
// files stay near upstream text. Every account shelves its own
// `library_<uid>.json`; guests and every non-self-hosted build keep the
// device-wide `library.json`.
//
// Server-safe on purpose: services/appService.ts imports this from React
// Server Component graphs, so no react hooks here — the reload hook lives
// in useShelfScopeReload.ts.

import { SELFHOSTED } from '@/utils/supabase';
import { jwtToUser } from '@/services/selfhostedAuth';

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
