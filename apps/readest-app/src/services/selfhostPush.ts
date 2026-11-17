// SELFHOSTED (fork): user-to-user book push client (ROADMAP ⑬) — offer one
// of your books (file + metadata snapshot, never notes or progress) to
// another account on the same server, and manage the pending inbox as the
// recipient. Regular-user surface: the token is the account's own JWT, no
// admin role involved. Contract: docs/selfhosted/sync-protocol.md §6.9.

import { getSelfhostedServerUrl } from '@/services/selfhostedServerUrl';

export class SelfhostPushError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.name = 'SelfhostPushError';
    this.status = status;
  }
}

export interface PushInboxItem {
  id: number;
  from: string;
  bookHash: string;
  title: string | null;
  author: string | null;
  message: string;
  createdAt: string;
}

export interface PushAcceptSummary {
  accepted: boolean;
  filesLinked: number;
  filesSkipped: number;
}

function authHeader(): Record<string, string> {
  const token = typeof window !== 'undefined' ? localStorage.getItem('token') : null;
  if (!token) {
    throw new SelfhostPushError('Not signed in', 401);
  }
  return { Authorization: `Bearer ${token}` };
}

function requireBaseUrl(): string {
  const baseUrl = getSelfhostedServerUrl();
  if (!baseUrl) {
    throw new SelfhostPushError('Sync server URL is not configured', 0);
  }
  return baseUrl;
}

/** Shared request shape: JSON in, JSON out, server error strings surfaced. */
async function jsonRequest<T>(path: string, method: string, body?: unknown): Promise<T> {
  // Outside the try: a missing token/config must surface as its own error,
  // not be swallowed into the network-failure branch below.
  const auth = authHeader();
  const base = requireBaseUrl();
  let res: Response;
  try {
    res = await fetch(`${base}${path}`, {
      method,
      headers: { ...auth, ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}) },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  } catch {
    throw new SelfhostPushError('Cannot reach the sync server', 0);
  }
  const text = await res.text().catch(() => '');
  if (!res.ok) {
    let message = `Request failed (HTTP ${res.status})`;
    try {
      const parsed = JSON.parse(text) as { error?: unknown };
      if (typeof parsed.error === 'string') message = parsed.error;
    } catch {
      // non-JSON error body: keep the generic message
    }
    throw new SelfhostPushError(message, res.status);
  }
  return (text ? JSON.parse(text) : {}) as T;
}

/** POST /api/pushes — offer a book to another account on this server. */
export function createPush(params: {
  to: string;
  bookHash: string;
  message?: string;
}): Promise<{ pushed: boolean; id: number }> {
  return jsonRequest('/api/pushes', 'POST', params);
}

/** GET /api/pushes/inbox — the caller's pending offers, newest first. */
export async function listInbox(): Promise<PushInboxItem[]> {
  const body = await jsonRequest<{ items?: PushInboxItem[] }>('/api/pushes/inbox', 'GET');
  return body.items ?? [];
}

/** POST /api/pushes/{id}/accept — land the offer into the caller's library. */
export function acceptPush(id: number): Promise<PushAcceptSummary> {
  return jsonRequest(`/api/pushes/${id}/accept`, 'POST');
}

/** POST /api/pushes/{id}/decline — refuse the offer; nothing lands. */
export function declinePush(id: number): Promise<{ declined: number }> {
  return jsonRequest(`/api/pushes/${id}/decline`, 'POST');
}
