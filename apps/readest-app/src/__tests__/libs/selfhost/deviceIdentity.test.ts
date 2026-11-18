/**
 * deviceIdentity — ⑪-P1 local {id, key} pair: generation shapes, persistence,
 * import normalization, backup text. The server-side contract (UUID email,
 * >=16-char secret) is mirrored here so a generated pair always provisions.
 */
import { afterEach, describe, expect, it } from 'vitest';
import {
  clearDeviceIdentity,
  deviceBackupText,
  downloadDeviceIdentityBackup,
  generateDeviceIdentity,
  generateDeviceKey,
  getDeviceIdentity,
  parseDeviceIdentity,
  saveDeviceIdentity,
} from '@/libs/selfhost/deviceIdentity';

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const KEY_RE = /^[0-9A-Z]{5}(-[0-9A-Z]{5}){4}$/;

describe('deviceIdentity', () => {
  afterEach(() => {
    clearDeviceIdentity();
  });

  it('generates a UUID id and a 5x5 base32 key', () => {
    for (let i = 0; i < 20; i++) {
      const { id, key } = generateDeviceIdentity();
      expect(id).toMatch(UUID_RE);
      expect(key).toMatch(KEY_RE);
      // Crockford alphabet: no I/L/O/U anywhere.
      expect(key.replace(/-/g, '')).not.toMatch(/[ILOU]/);
    }
    expect(generateDeviceKey()).not.toBe(generateDeviceKey());
  });

  it('persists and reloads the pair', () => {
    expect(getDeviceIdentity()).toBeNull();
    const identity = {
      id: '01234567-89ab-4cde-8f01-23456789abcd',
      key: 'ABCDE-FGHJK-MNPQR-STVWX-YZ012',
    };
    saveDeviceIdentity(identity);
    expect(getDeviceIdentity()).toEqual(identity);
    clearDeviceIdentity();
    expect(getDeviceIdentity()).toBeNull();
  });

  it('normalizes an imported pair: case, dashes and whitespace', () => {
    const parsed = parseDeviceIdentity(
      '  01234567-89AB-4CDE-8F01-23456789ABCD ',
      ' abcde fghjk mnpqr stvwx yz012 ',
    );
    expect(parsed.id).toBe('01234567-89AB-4CDE-8F01-23456789abcd'.toLowerCase());
    expect(parsed.key).toBe('ABCDE-FGHJK-MNPQR-STVWX-YZ012');
  });

  it('rejects malformed ids and keys', () => {
    const key = 'ABCDE-FGHJK-MNPQR-STVWX-YZ012';
    expect(() => parseDeviceIdentity('not-a-uuid', key)).toThrow('invalid-device-id');
    expect(() => parseDeviceIdentity('01234567-89ab-4cde-8f01-23456789abcd', 'short')).toThrow(
      'invalid-device-key',
    );
    // I/L/O/U are not in the Crockford alphabet.
    expect(() =>
      parseDeviceIdentity('01234567-89ab-4cde-8f01-23456789abcd', 'ILOUI-LOUIL-OUILO-UILOU-ILOUI'),
    ).toThrow('invalid-device-key');
  });

  it('backup text carries server, id and key', () => {
    const identity = {
      id: '01234567-89ab-4cde-8f01-23456789abcd',
      key: 'ABCDE-FGHJK-MNPQR-STVWX-YZ012',
    };
    const text = deviceBackupText(identity, 'http://127.0.0.1:8080');
    expect(text).toContain('server: http://127.0.0.1:8080');
    expect(text).toContain(`id: ${identity.id}`);
    expect(text).toContain(`key: ${identity.key}`);
  });

  it('backup download uses the anchor idiom', () => {
    const identity = {
      id: '01234567-89ab-4cde-8f01-23456789abcd',
      key: 'ABCDE-FGHJK-MNPQR-STVWX-YZ012',
    };
    const clicks: string[] = [];
    const click = HTMLAnchorElement.prototype.click;
    HTMLAnchorElement.prototype.click = function () {
      clicks.push(this.download);
    };
    try {
      const name = downloadDeviceIdentityBackup(identity, 'http://127.0.0.1:8080');
      expect(clicks).toEqual([name]);
      expect(name).toBe('readest-device-identity-01234567.txt');
    } finally {
      HTMLAnchorElement.prototype.click = click;
    }
  });
});
