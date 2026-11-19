/**
 * ⑪-P3 manual LocalSend connect — address parsing and the invoke wrapper.
 * Contract: `host`, `host:port`, and `[ipv6]:port` all parse; bare IPv6 stays
 * whole; garbage (empty, non-numeric port, out-of-range port) is rejected;
 * the wrapper passes (host, port) straight to the Rust command.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

import {
  LOCALSEND_DEFAULT_PORT,
  connectManualLocalSend,
  parseManualTarget,
} from '@/libs/selfhost/localsendManual';

describe('parseManualTarget', () => {
  it('parses host with explicit port', () => {
    expect(parseManualTarget('192.168.1.7:53317')).toEqual({ host: '192.168.1.7', port: 53317 });
  });

  it('parses host without port as port null', () => {
    expect(parseManualTarget('my-laptop.local')).toEqual({ host: 'my-laptop.local', port: null });
  });

  it('parses bracketed IPv6 with and without port', () => {
    expect(parseManualTarget('[fe80::1%25en0]:1234')).toEqual({
      host: 'fe80::1%25en0',
      port: 1234,
    });
    expect(parseManualTarget('[fe80::1]')).toEqual({ host: 'fe80::1', port: null });
  });

  it('keeps a bare IPv6 address whole', () => {
    expect(parseManualTarget('fe80::1')).toEqual({ host: 'fe80::1', port: null });
  });

  it('trims surrounding whitespace', () => {
    expect(parseManualTarget('  10.0.0.2:48  ')).toEqual({ host: '10.0.0.2', port: 48 });
  });

  it('rejects empty and malformed input', () => {
    expect(parseManualTarget('')).toBeNull();
    expect(parseManualTarget('   ')).toBeNull();
    expect(parseManualTarget(':53317')).toBeNull();
    expect(parseManualTarget('host:')).toBeNull();
    expect(parseManualTarget('host:notaport')).toBeNull();
    expect(parseManualTarget('host:0')).toBeNull();
    expect(parseManualTarget('host:99999')).toBeNull();
  });
});

describe('connectManualLocalSend', () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it('invokes the Rust command with host and port', async () => {
    invokeMock.mockResolvedValueOnce({
      alias: 'Desk',
      deviceModel: 'Windows',
      deviceType: 'desktop',
      fingerprint: 'fp',
    });
    const peer = await connectManualLocalSend('192.168.1.7', 53317);
    expect(invokeMock).toHaveBeenCalledWith('localsend_connect_manual', {
      host: '192.168.1.7',
      port: 53317,
    });
    expect(peer.alias).toBe('Desk');
  });

  it('exposes the LocalSend default port constant', () => {
    expect(LOCALSEND_DEFAULT_PORT).toBe(53317);
  });
});
