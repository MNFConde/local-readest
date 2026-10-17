import { createClient, type SupabaseClient } from '@supabase/supabase-js';
// FORK(selfhost): the SELFHOSTED flag and the env-guarded url/key resolution
// live in the fork module (empty strings under SELFHOSTED, so atob never runs
// without the base64 env); re-exported here for the existing import surface.
import { SELFHOSTED, supabaseAnonKey, supabaseUrl } from '@/libs/selfhost/supabaseEnv';

export { SELFHOSTED };

// supabase-js persists the session under a key derived from the URL's first
// host label. Readest Cloud keeps the key from its original host so moving the
// client to another of its hosts doesn't sign everyone out.
const READEST_CLOUD_HOSTS = [
  'readest.supabase.co',
  'vbsyxfusjjqdxkjqlysc.supabase.co',
  'auth.readest.com',
];

export const supabase: SupabaseClient | null = createSupabaseClientSafe();

// Avoid calling createClient with empty url/key (which throws at module load
// when the Supabase env vars are absent). The official path requires those vars
// at runtime; if missing, callsites using `supabase!` will fail loudly.
function createSupabaseClientSafe(): SupabaseClient | null {
  if (SELFHOSTED) return null;
  if (!supabaseUrl || !supabaseAnonKey) return null;
  return createClient(
    supabaseUrl,
    supabaseAnonKey,
    READEST_CLOUD_HOSTS.includes(new URL(supabaseUrl).hostname)
      ? { auth: { storageKey: 'sb-readest-auth-token' } }
      : undefined,
  );
}

export const createSupabaseClient = (accessToken?: string) => {
  return createClient(supabaseUrl, supabaseAnonKey, {
    global: {
      headers: accessToken
        ? {
            Authorization: `Bearer ${accessToken}`,
          }
        : {},
    },
  });
};

export const createSupabaseAdminClient = () => {
  const supabaseAdminKey = process.env['SUPABASE_ADMIN_KEY'] || '';
  return createClient(supabaseUrl, supabaseAdminKey, {
    auth: {
      persistSession: false,
      autoRefreshToken: false,
      detectSessionInUrl: false,
    },
  });
};
