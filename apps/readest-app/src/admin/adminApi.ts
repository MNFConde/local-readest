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
  /** Per-account stored copies at full size (quota semantics), bytes. */
  bytesUsed: number;
  /** Last login/sync activity, unix seconds; null = never active. */
  lastSeenAt: number | null;
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
  /** Accounts with an active (non-deleted) library row for this hash. */
  holders: number;
}

export interface BookHolder {
  userId: string;
  email: string | null;
  deleted: boolean;
  syncedAt: string | null;
}

// ── supervisor server monitor (sync-protocol.md §6.6) ─────────────────────
// Host/process metrics pushed by readest-supervisor. Fields are optional on
// purpose: the server stores the payload verbatim and the schema may grow.

export interface HostServiceStatus {
  name: string;
  state: string;
  pid?: number | null;
  uptime_secs?: number | null;
  restarts?: number;
  consecutive_failures?: number;
  last_exit?: { code?: number | null; uptime_secs?: number } | null;
  probe?: { ok: boolean; status: number; rtt_ms: number } | null;
  memory_bytes?: number;
  cpu_percent?: number;
  circuit_reason?: string | null;
}

export interface HostSnapshot {
  schema?: number;
  ts?: number;
  supervisor?: {
    version?: string;
    pid?: number;
    uptime_secs?: number;
    headless?: boolean;
  };
  host?: { os?: string; cores?: number; total_memory_bytes?: number };
  services?: HostServiceStatus[];
}

export interface HostSnapshotQuery {
  available: boolean;
  received_at?: number;
  age_secs?: number;
  snapshot?: HostSnapshot;
}

// ── aggregate overview stats (sync-protocol.md §6.7) ───────────────────────
// One read-only /api/admin/stats payload feeding the Overview tab's cards.
// Snake_case mirrors the server's JSON verbatim (same convention as the
// host-snapshot feed).

export interface ServerStats {
  users: { total: number; admins: number; disabled: number };
  books: { synced: number; catalog: number; works: number };
  storage: {
    files_bytes: number;
    blobs_bytes: number;
    blobs_count: number;
    db_bytes: number;
    dedup_saved_bytes: number;
  };
  server: { version: string; uptime_secs: number };
}

function authHeader(): Record<string, string> {
  const token = typeof window !== 'undefined' ? localStorage.getItem('token') : null;
  if (!token) {
    throw new AdminApiError('Not signed in', 401);
  }
  return { Authorization: `Bearer ${token}` };
}

/** Shared transport: base URL, auth header, and the 401/403 mapping. */
async function adminFetchRaw(path: string, init?: RequestInit): Promise<Response> {
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
  return res;
}

async function adminFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await adminFetchRaw(path, init);
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

  /** Who holds this content — the delete-impact view (includes deleted rows). */
  getBookHolders: (hash: string) =>
    adminFetch<{ holders: BookHolder[] }>(`/books/${encodeURIComponent(hash)}/holders`).then(
      (r) => r.holders,
    ),

  deleteBook: (hash: string) =>
    adminFetch<{ deleted: number }>(`/books/${encodeURIComponent(hash)}`, { method: 'DELETE' }),

  gc: () => adminFetch<{ removed: number }>('/gc', { method: 'POST' }),

  enrich: () =>
    adminFetch<{ enriched: number; failed: { hash: string; error: string }[] }>('/enrich', {
      method: 'POST',
    }),

  /** Newest supervisor metrics snapshot; available=false when not connected. */
  getHostSnapshot: () => adminFetch<HostSnapshotQuery>('/host-snapshot'),

  /** Aggregated overview numbers (counts, disk sizes, version, uptime). */
  getStats: () => adminFetch<ServerStats>('/stats'),

  /**
   * Downloads the full-server backup tar (GET /backup) as a file. Binary, so
   * it bypasses the JSON adminFetch; the auth header rules out a plain link,
   * hence fetch → blob → anchor click.
   */
  downloadBackup: async (): Promise<void> => {
    const res = await adminFetchRaw('/backup');
    if (!res.ok) {
      throw new AdminApiError(`Backup failed (HTTP ${res.status})`, res.status);
    }
    const blob = await res.blob();
    const name =
      /filename="([^"]+)"/.exec(res.headers.get('Content-Disposition') ?? '')?.[1] ??
      'readest-backup.tar';
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = name;
    anchor.click();
    URL.revokeObjectURL(url);
  },
};
