// Self-contained admin API client for the /admin page. Lives in its own
// subtree on purpose: it must never import upstream business components so
// an upstream merge keeps this directory conflict-free. It only talks to the
// self-hosted sync server's /api/admin surface (docs/selfhosted/sync-protocol.md).

import { getSelfhostedServerUrl } from '@/services/selfhostedServerUrl';

export class AdminApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.name = 'AdminApiError';
    this.status = status;
  }
}

export interface AdminUser {
  email: string;
  id: string;
  disabled: boolean;
  createdAt: string;
  role: string;
}

export interface CatalogBook {
  hash: string;
  title: string;
  author: string;
  language: string;
  workId: string | null;
  sourceHash: string | null;
  addedAt: string;
  addedBy: string;
}

function authHeader(): Record<string, string> {
  const token = typeof window !== 'undefined' ? localStorage.getItem('token') : null;
  if (!token) {
    throw new AdminApiError('Not signed in', 401);
  }
  return { Authorization: `Bearer ${token}` };
}

async function adminFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const baseUrl = getSelfhostedServerUrl();
  if (!baseUrl) {
    throw new AdminApiError('Sync server URL is not configured', 0);
  }
  // Outside the try: a missing token must surface as its own error, not be
  // swallowed into the network-failure branch below.
  const headers = authHeader();
  let res: Response;
  try {
    res = await fetch(`${baseUrl}/api/admin${path}`, {
      ...init,
      headers: {
        ...headers,
        ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      },
    });
  } catch {
    // Network-level failure (refused, DNS, CORS): indistinguishable here.
    throw new AdminApiError(`Cannot reach the sync server at ${baseUrl}`, 0);
  }
  if (res.status === 401) {
    throw new AdminApiError('Session expired — sign in again', 401);
  }
  if (res.status === 403) {
    throw new AdminApiError('Admin access required', 403);
  }
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    let message = `Request failed (HTTP ${res.status})`;
    try {
      const parsed = JSON.parse(text) as { error?: unknown };
      if (typeof parsed.error === 'string') message = parsed.error;
    } catch {
      // non-JSON error body: keep the generic message
    }
    throw new AdminApiError(message, res.status);
  }
  const text = await res.text();
  return (text ? JSON.parse(text) : {}) as T;
}

export interface CreateUserResult {
  email: string;
  /** Random server-generated password, returned exactly once ('' = passwordless). */
  password: string;
  role: string;
}

export const adminApi = {
  listUsers: () => adminFetch<{ users: AdminUser[] }>('/users').then((r) => r.users),

  createUser: (email: string, password?: string, role: string = 'user') =>
    adminFetch<CreateUserResult>('/users', {
      method: 'POST',
      body: JSON.stringify(password === undefined ? { email, role } : { email, password, role }),
    }),

  deleteUser: (email: string) =>
    adminFetch<{ deleted: string }>(`/users/${encodeURIComponent(email)}`, { method: 'DELETE' }),

  /** Explicit password (may be '' = passwordless); omitted → server generates one. */
  resetPassword: (email: string, password?: string) =>
    adminFetch<{ updated: number; password?: string }>(
      `/users/${encodeURIComponent(email)}/password`,
      { method: 'POST', body: JSON.stringify(password === undefined ? {} : { password }) },
    ),

  setUserRole: (email: string, role: 'admin' | 'user') =>
    adminFetch<{ updated: number; role: string }>(`/users/${encodeURIComponent(email)}/role`, {
      method: 'POST',
      body: JSON.stringify({ role }),
    }),

  setUserState: (email: string, disabled: boolean) =>
    adminFetch<{ updated: number }>(`/users/${encodeURIComponent(email)}/state`, {
      method: 'POST',
      body: JSON.stringify({ disabled }),
    }),

  listBooks: () => adminFetch<{ books: CatalogBook[] }>('/books').then((r) => r.books),

  deleteBook: (hash: string) =>
    adminFetch<{ deleted: number }>(`/books/${encodeURIComponent(hash)}`, { method: 'DELETE' }),

  gc: () => adminFetch<{ removed: number }>('/gc', { method: 'POST' }),

  enrich: () =>
    adminFetch<{ enriched: number; failed: { hash: string; error: string }[] }>('/enrich', {
      method: 'POST',
    }),
};
