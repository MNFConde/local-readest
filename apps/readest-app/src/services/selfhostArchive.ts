// SELFHOSTED (fork): user archive transfer client — packages the signed-in
// account's books, records, and (optionally) book files into a portable tar,
// and imports one back into any account on this server (LWW merge, never
// clobbering newer data). Regular-user surface: the token is the account's
// own JWT, no admin role involved. Contract: docs/selfhosted/sync-protocol.md
// §6.8.

import { getSelfhostedServerUrl } from '@/services/selfhostedServerUrl';

export type ArchiveScope = 'full' | 'metadata';

export class SelfhostArchiveError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.name = 'SelfhostArchiveError';
    this.status = status;
  }
}

export interface ArchiveImportSummary {
  books: number;
  notes: number;
  configs: number;
  statBooks: number;
  statPages: number;
  replicas: number;
  replicaKeys: number;
  blobsStored: number;
  blobsDeduped: number;
  blobsSkipped: number;
  filesLinked: number;
  filesSkipped: number;
}

function authHeader(): Record<string, string> {
  const token = typeof window !== 'undefined' ? localStorage.getItem('token') : null;
  if (!token) {
    throw new SelfhostArchiveError('Not signed in', 401);
  }
  return { Authorization: `Bearer ${token}` };
}

function requireBaseUrl(): string {
  const baseUrl = getSelfhostedServerUrl();
  if (!baseUrl) {
    throw new SelfhostArchiveError('Sync server URL is not configured', 0);
  }
  return baseUrl;
}

/**
 * GET /api/archive/export → browser save. Binary + auth header rules out a
 * plain link, hence fetch → blob → anchor click (same idiom as the admin
 * backup download). Returns the saved filename.
 */
export async function exportArchive(scope: ArchiveScope): Promise<string> {
  // Outside the try: a missing token/config must surface as its own error,
  // not be swallowed into the network-failure branch below.
  const headers = authHeader();
  const base = requireBaseUrl();
  let res: Response;
  try {
    res = await fetch(`${base}/api/archive/export?scope=${scope}`, { headers });
  } catch {
    throw new SelfhostArchiveError('Cannot reach the sync server', 0);
  }
  if (!res.ok) {
    throw new SelfhostArchiveError(`Export failed (HTTP ${res.status})`, res.status);
  }
  const blob = await res.blob();
  const name =
    /filename="([^"]+)"/.exec(res.headers.get('Content-Disposition') ?? '')?.[1] ??
    `readest-archive-${scope}.tar`;
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  URL.revokeObjectURL(url);
  return name;
}

/** POST /api/archive/import with the raw tar as the request body. */
export async function importArchive(file: File): Promise<ArchiveImportSummary> {
  const auth = authHeader();
  const base = requireBaseUrl();
  let res: Response;
  try {
    res = await fetch(`${base}/api/archive/import`, {
      method: 'POST',
      headers: { ...auth, 'Content-Type': 'application/x-tar' },
      body: file,
    });
  } catch {
    throw new SelfhostArchiveError('Cannot reach the sync server', 0);
  }
  const text = await res.text().catch(() => '');
  if (!res.ok) {
    let message = `Import failed (HTTP ${res.status})`;
    try {
      const parsed = JSON.parse(text) as { error?: unknown };
      if (typeof parsed.error === 'string') message = parsed.error;
    } catch {
      // non-JSON error body: keep the generic message
    }
    throw new SelfhostArchiveError(message, res.status);
  }
  return (text ? JSON.parse(text) : {}) as ArchiveImportSummary;
}
