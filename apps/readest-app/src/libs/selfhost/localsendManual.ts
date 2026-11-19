import { invoke } from '@tauri-apps/api/core';

/**
 * Manual (host:port) connect for the LocalSend integration — the fallback for
 * networks where multicast discovery is blocked (cross-VLAN, AP isolation).
 * The Rust side probes the peer's `/info` endpoint and feeds it into the
 * discovery store, after which the device behaves exactly like a discovered
 * one (same picker grid, same pairing, same send path).
 */

export interface ManualPeerInfo {
  alias: string;
  deviceModel: string | null;
  deviceType: string | null;
  fingerprint: string;
}

/** LocalSend's well-known default port, used when the address omits one. */
export const LOCALSEND_DEFAULT_PORT = 53317;

export async function connectManualLocalSend(host: string, port: number): Promise<ManualPeerInfo> {
  return invoke<ManualPeerInfo>('localsend_connect_manual', { host, port });
}

/**
 * Parses a user-typed address: `host`, `host:port`, or `[ipv6]:port`. Bare
 * IPv6 (multiple colons) is taken whole. Returns null for anything unusable;
 * a missing port stays null so the caller can apply the default.
 */
export function parseManualTarget(input: string): { host: string; port: number | null } | null {
  const text = input.trim();
  if (!text) return null;
  const bracketed = text.match(/^\[([^\]]+)\](?::(\d+))?$/);
  if (bracketed) {
    return {
      host: bracketed[1] ?? '',
      port: bracketed[2] ? Number(bracketed[2]) : null,
    };
  }
  const firstColon = text.indexOf(':');
  if (firstColon === -1) return { host: text, port: null };
  // More than one colon and no brackets: a bare IPv6 address.
  if (text.indexOf(':', firstColon + 1) !== -1) return { host: text, port: null };
  const host = text.slice(0, firstColon).trim() || '';
  const portText = text.slice(firstColon + 1).trim();
  if (!host) return null;
  const port = Number(portText);
  if (!/^\d+$/.test(portText) || !Number.isInteger(port) || port < 1 || port > 65535) {
    return null;
  }
  return { host, port };
}
