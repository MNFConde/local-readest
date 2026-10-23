import { getAccessToken } from './access';
// SELFHOSTED (fork): authenticated API calls route through the silent 401
// re-login wrapper (libs/selfhost/authFetch) so an expired JWT renews itself.
import { selfhostedAuthFetch } from '@/libs/selfhost/authFetch';
import { SELFHOSTED } from '@/utils/supabase';

// Resolves `fetch` at CALL time (not module-eval time) so test suites that
// stub globalThis.fetch keep intercepting the non-selfhosted path too.
const apiFetch: typeof fetch = (input, init) =>
  SELFHOSTED ? selfhostedAuthFetch(input, init) : fetch(input, init);

export const fetchWithTimeout = (url: string, options: RequestInit = {}, timeout = 10000) => {
  const controller = new AbortController();
  const id = setTimeout(() => controller.abort('Request timed out'), timeout);

  return fetch(url, {
    ...options,
    signal: controller.signal,
  }).finally(() => clearTimeout(id));
};

export const fetchWithAuth = async (url: string, options: RequestInit) => {
  const token = await getAccessToken();
  if (!token) {
    throw new Error('Not authenticated');
  }
  const headers = {
    ...options.headers,
    Authorization: `Bearer ${token}`,
  };

  const response = await apiFetch(url, { ...options, headers });

  if (!response.ok) {
    const errorData = await response.json();
    console.error('Error:', errorData.error || response.statusText);
    throw new Error(errorData.error || 'Request failed');
  }

  return response;
};
