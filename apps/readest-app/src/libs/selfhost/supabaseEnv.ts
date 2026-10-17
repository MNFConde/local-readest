import { getRuntimeConfig } from '@/services/runtimeConfig';

/**
 * Self-hosted mode toggle. When set (`NEXT_PUBLIC_SELFHOSTED=1`), the app talks
 * to a self-hosted Readest sync server (code login + JWT) instead of Supabase.
 * The official Supabase path is preserved when this is off. Re-exported from
 * `@/utils/supabase` so callsites keep importing from the shared module.
 */
export const SELFHOSTED = process.env['NEXT_PUBLIC_SELFHOSTED'] === '1';

// In self-hosted mode the Supabase client is never used, so we skip resolving
// the default base64 env vars (which may be absent and would crash atob() at
// module load). The official path still reads them normally when SELFHOSTED off.
const b64Url = process.env['NEXT_PUBLIC_DEFAULT_SUPABASE_URL_BASE64'];
const b64Key = process.env['NEXT_PUBLIC_DEFAULT_SUPABASE_KEY_BASE64'];
export const supabaseUrl = SELFHOSTED
  ? ''
  : getRuntimeConfig()?.supabaseUrl ||
    process.env['SUPABASE_URL'] ||
    process.env['NEXT_PUBLIC_SUPABASE_URL'] ||
    (b64Url ? atob(b64Url) : '');
export const supabaseAnonKey = SELFHOSTED
  ? ''
  : getRuntimeConfig()?.supabaseAnonKey ||
    process.env['SUPABASE_ANON_KEY'] ||
    process.env['NEXT_PUBLIC_SUPABASE_ANON_KEY'] ||
    (b64Key ? atob(b64Key) : '');
