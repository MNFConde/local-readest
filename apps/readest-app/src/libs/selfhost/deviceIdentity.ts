// SELFHOSTED (fork): device identity (ROADMAP ⑪-P1) — a locally generated
// {id, key} pair that auto-provisions its own `kind='device'` account on the
// sync server (ALLOW_AUTO_PROVISION). The id doubles as the account email
// (strict UUID), the key as the password; the server never stores the key in
// the clear and offers no recovery, so everything here leans on the backup
// file this module can download.

export interface DeviceIdentity {
  id: string;
  key: string;
}

const STORAGE_KEY = 'selfhosted_device_identity';

// Crockford base32: no I/L/O/U, so a transcribed key can be typed back
// case-insensitively without lookalike-letter mistakes.
const KEY_ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
const KEY_GROUP = 5;
const KEY_GROUPS = 5;
const KEY_CHARS = KEY_GROUP * KEY_GROUPS; // 25 chars ≈ 125 bits

function randomBytes(n: number): Uint8Array {
  const g = globalThis.crypto;
  if (!g?.getRandomValues) {
    throw new Error('crypto.getRandomValues unavailable');
  }
  const out = new Uint8Array(n);
  g.getRandomValues(out);
  return out;
}

export function generateDeviceKey(): string {
  const bytes = randomBytes(KEY_CHARS);
  const chars = Array.from(bytes, (b) => KEY_ALPHABET[b % KEY_ALPHABET.length]);
  const groups: string[] = [];
  for (let i = 0; i < KEY_CHARS; i += KEY_GROUP) {
    groups.push(chars.slice(i, i + KEY_GROUP).join(''));
  }
  return groups.join('-');
}

export function generateDeviceId(): string {
  const g = globalThis.crypto as Crypto | undefined;
  if (g && 'randomUUID' in g) {
    return g.randomUUID();
  }
  // v4 formatting from random bytes (same fallback dance as the app's
  // X-Device-Id): 16 bytes, version/variant nibbles fixed.
  const b = randomBytes(16);
  b[6] = (b[6]! & 0x0f) | 0x40;
  b[8] = (b[8]! & 0x3f) | 0x80;
  const h = Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

export function generateDeviceIdentity(): DeviceIdentity {
  return { id: generateDeviceId(), key: generateDeviceKey() };
}

export function getDeviceIdentity(): DeviceIdentity | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<DeviceIdentity>;
    if (typeof parsed.id !== 'string' || typeof parsed.key !== 'string') return null;
    return { id: parsed.id, key: parsed.key };
  } catch {
    return null;
  }
}

export function saveDeviceIdentity(identity: DeviceIdentity): void {
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify(identity));
}

export function clearDeviceIdentity(): void {
  window.localStorage.removeItem(STORAGE_KEY);
}

/** UUID shape (8-4-4-4-12 hex) — mirrors the server's is_uuid_email gate. */
export function isDeviceIdShape(id: string): boolean {
  const re = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;
  return re.test(id);
}

/**
 * Parse an id+key pair from user input (backup file fields or paste).
 * Tolerates dash/space/quote noise and lowercase keys; throws on anything
 * that could not be a device identity so the UI can show one clear error.
 */
export function parseDeviceIdentity(id: string, key: string): DeviceIdentity {
  const cleanId = id.trim().toLowerCase();
  const cleanKey = key.trim().replace(/["\s]/g, '').toUpperCase();
  if (!isDeviceIdShape(cleanId)) {
    throw new Error('invalid-device-id');
  }
  const compact = cleanKey.replace(/-/g, '');
  if (compact.length !== KEY_CHARS || [...compact].some((c) => !KEY_ALPHABET.includes(c))) {
    throw new Error('invalid-device-key');
  }
  const groups: string[] = [];
  for (let i = 0; i < KEY_CHARS; i += KEY_GROUP) {
    groups.push(compact.slice(i, i + KEY_GROUP));
  }
  return { id: cleanId, key: groups.join('-') };
}

/** Human-readable backup file body: everything needed to restore, verbatim. */
export function deviceBackupText(identity: DeviceIdentity, serverUrl: string): string {
  return [
    'Readest device identity backup (⑪-P1)',
    `server: ${serverUrl || '(fill in your sync server address)'}`,
    `id: ${identity.id}`,
    `key: ${identity.key}`,
    '',
    'Restore via: sign-in screen → "Use a device identity" → "Import an existing identity".',
    'Anyone holding this pair can read the account — keep the file private.',
  ].join('\n');
}

/**
 * Save the backup as a downloaded file (fetch→blob→anchor-click is only
 * needed for network payloads; this one is local text, but the same anchor
 * idiom keeps the save behavior identical across platforms).
 */
export function downloadDeviceIdentityBackup(identity: DeviceIdentity, serverUrl: string): string {
  const blob = new Blob([deviceBackupText(identity, serverUrl)], {
    type: 'text/plain;charset=utf-8',
  });
  const name = `readest-device-identity-${identity.id.slice(0, 8)}.txt`;
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  URL.revokeObjectURL(url);
  return name;
}
