// SELFHOSTED (fork): remembered multi-server connections (ROADMAP ⑪-P2,
// lightweight). Every successful sign-in on the selfhosted login form is
// recorded here, so returning to a previous server/account is one click on
// the login page — no server changes, no merged view: one connection is
// active at a time (the global server-url/credentials keys still hold the
// active one, exactly as before).
//
// Each connection carries a transfer `mode`: 'full' (default) or 'metadata'
// — a metadata-only connection syncs book records (and covers) but never
// transfers book files. The gates that consult it live where the native
// file channel funnels: TransferManager.isBookUploadAllowed / queueDownload
// and useBookTransferActions' explicit upload/download actions.

import i18n from '@/i18n/i18n';
import { eventDispatcher } from '@/utils/event';
import env from '@/services/environment';
import { normalizeServerUrl } from '@/services/selfhostedServerUrl';

export type ConnectionMode = 'full' | 'metadata';

export interface SelfhostConnection {
  // `${serverUrl}|${email-lowercase}` — the same pair on a different server
  // is a different connection, and so is a different account on one server.
  id: string;
  serverUrl: string;
  email: string;
  // The saved login secret (password or device key), so re-signing in to a
  // remembered connection is one click. Same plaintext semantics as the
  // single `selfhosted_credentials` entry it mirrors.
  password: string;
  mode: ConnectionMode;
  updatedAt: number;
}

const CONNECTIONS_KEY = 'selfhosted_connections';
const ACTIVE_KEY = 'selfhosted_active_connection';
// TransferManager's persisted queue (`TRANSFER_QUEUE_KEY` there) — cleared on
// a connection switch so queued transfers for the old account's books can
// never land in the new account.
const TRANSFER_QUEUE_KEY = 'readest_transfer_queue';

export const connectionId = (serverUrl: string, email: string): string =>
  `${normalizeServerUrl(serverUrl)}|${email.trim().toLowerCase()}`;

export function listConnections(): SelfhostConnection[] {
  if (typeof window === 'undefined') return [];
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(CONNECTIONS_KEY) ?? '[]');
    return Array.isArray(parsed) ? (parsed as SelfhostConnection[]) : [];
  } catch {
    return [];
  }
}

function saveConnections(connections: SelfhostConnection[]): void {
  localStorage.setItem(CONNECTIONS_KEY, JSON.stringify(connections));
}

export function getActiveConnectionId(): string {
  if (typeof window === 'undefined') return '';
  return localStorage.getItem(ACTIVE_KEY) ?? '';
}

export function getActiveConnection(): SelfhostConnection | null {
  const id = getActiveConnectionId();
  return listConnections().find((c) => c.id === id) ?? null;
}

export interface UpsertResult {
  connection: SelfhostConnection;
  /** True when the active connection changed — the caller must reset the
   * previous connection's sync cursors and persisted transfer queue. */
  switched: boolean;
}

/**
 * Record a successful login as the active connection. Re-login to a known
 * connection refreshes its password and keeps its mode; a first login starts
 * in 'full' mode. `switched` also fires on a legacy device's very first
 * recorded login: zeroing then-existing cursors only costs one LWW re-pull,
 * while skipping it could hide a whole other account's library.
 */
export function upsertConnectionOnLogin(
  serverUrl: string,
  email: string,
  password: string,
): UpsertResult {
  const normalized = normalizeServerUrl(serverUrl);
  const id = connectionId(normalized, email);
  const connections = listConnections();
  const previousActive = getActiveConnectionId();
  const existing = connections.find((c) => c.id === id);
  const connection: SelfhostConnection = existing
    ? { ...existing, password, updatedAt: Date.now() }
    : {
        id,
        serverUrl: normalized,
        email: email.trim(),
        password,
        mode: 'full',
        updatedAt: Date.now(),
      };
  saveConnections(
    existing
      ? connections.map((c) => (c.id === id ? connection : c))
      : [...connections, connection],
  );
  localStorage.setItem(ACTIVE_KEY, id);
  return { connection, switched: previousActive !== id };
}

export function removeConnection(id: string): void {
  saveConnections(listConnections().filter((c) => c.id !== id));
  if (getActiveConnectionId() === id) localStorage.removeItem(ACTIVE_KEY);
}

export function setConnectionMode(id: string, mode: ConnectionMode): void {
  saveConnections(listConnections().map((c) => (c.id === id ? { ...c, mode } : c)));
}

/** Whether the active connection skips book-file transfers. False outside
 * selfhosted mode or before any login — gates must be no-ops there. */
export function isMetadataOnlyConnection(): boolean {
  return getActiveConnection()?.mode === 'metadata';
}

/**
 * Zero the device-global sync cursors in settings.json (books/configs/notes
 * pulls and the replica HLC map) so the newly active connection re-pulls
 * everything. Settings are device-global — they are NOT account-scoped like
 * the library shelf — so a switch must not let connection A's cursor hide
 * connection B's rows. LWW merge keeps anything newer on either side.
 */
export async function resetSyncCursorsForSwitch(): Promise<void> {
  try {
    const appService = await env.getAppService();
    const settings = await appService.loadSettings();
    settings.lastSyncedAtBooks = 0;
    settings.lastSyncedAtConfigs = 0;
    settings.lastSyncedAtNotes = 0;
    settings.lastSyncedAtReplicas = {};
    await appService.saveSettings(settings);
  } catch (err) {
    // Without a usable app service there is nothing to reset; the pull window
    // (lookback + >3-day zeroing) still converges, just slower.
    console.warn('[selfhost] failed to reset sync cursors on connection switch', err);
  }
}

export function clearPersistedTransferQueue(): void {
  try {
    localStorage.removeItem(TRANSFER_QUEUE_KEY);
  } catch {
    // localStorage unavailable — nothing persisted to clear.
  }
}

/**
 * Hard-reload after a connection switch (supersedes any in-flight SPA
 * navigate) so every store — shelf, settings cursors, in-memory transfer
 * queue — rebuilds from the newly active connection's persisted state.
 * Extracted from the login form so tests can stub it instead of jsdom's
 * unforgeable window.location.
 */
export function reloadForConnectionSwitch(redirectTo: string | null): void {
  window.location.replace(redirectTo && redirectTo.startsWith('/') ? redirectTo : '/library');
}

// Explicit upload/download on a metadata-only connection must never silently
// no-op (same contract as the Books-sync-off hints). The messages live here
// so the upstream-file gate hunks stay two lines and carry no fork strings.
export function toastMetadataUploadRefused(title: string): void {
  eventDispatcher.dispatch('toast', {
    type: 'info',
    timeout: 5000,
    message: i18n.t('Metadata-only connection skips book file uploads ({{title}})', {
      ns: 'selfhost',
      title,
    }),
  });
}

export function toastMetadataDownloadRefused(title: string): void {
  eventDispatcher.dispatch('toast', {
    type: 'info',
    timeout: 5000,
    message: i18n.t('Metadata-only connection skips book file downloads ({{title}})', {
      ns: 'selfhost',
      title,
    }),
  });
}
