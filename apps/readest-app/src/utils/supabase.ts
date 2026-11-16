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

// Some ISPs block *.supabase.co (#6416), so Readest Cloud sends password sign-in
// and the calls that keep such a session alive through a Worker on readest.com
// (workers/auth-proxy). OAuth and email links stay on the Supabase host: their
// callbacks land there, and a proxy can't change that.
const AUTH_PROXY_URL = 'https://auth-cf.readest.com';

const isProxiedAuthRequest = ({ pathname, searchParams }: URL) =>
  pathname === '/auth/v1/logout' ||
  (pathname === '/auth/v1/token' &&
    ['password', 'refresh_token'].includes(searchParams.get('grant_type') ?? ''));

const proxyAuthFetch: typeof fetch = (input, init) => {
  const url = new URL(input instanceof Request ? input.url : input);
  return isProxiedAuthRequest(url)
    ? fetch(`${AUTH_PROXY_URL}${url.pathname}${url.search}`, init)
    : fetch(input, init);
};

export const supabase: SupabaseClient | null = createSupabaseClientSafe();

// FORK(selfhost): avoid calling createClient with empty url/key (which throws
// at module load when the Supabase env vars are absent). The official path
// requires those vars at runtime; if missing, callsites using `supabase!` will
// fail loudly.
function createSupabaseClientSafe(): SupabaseClient | null {
  if (SELFHOSTED) return null;
  if (!supabaseUrl || !supabaseAnonKey) return null;
  return createClient(
    supabaseUrl,
    supabaseAnonKey,
    READEST_CLOUD_HOSTS.includes(new URL(supabaseUrl).hostname)
      ? { auth: { storageKey: 'sb-readest-auth-token' }, global: { fetch: proxyAuthFetch } }
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
