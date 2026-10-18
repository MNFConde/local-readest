// Shared human-readable formatters for the admin console (fork-owned file,
// no upstream imports). Kept tiny on purpose — every consumer renders
// server-provided numbers only.

export function humanBytes(bytes?: number | null): string {
  if (!bytes || bytes <= 0) return '—';
  const units = ['B', 'K', 'M', 'G', 'T'];
  let v = bytes;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return u === 0 ? `${bytes}B` : `${v.toFixed(1)}${units[u]}`;
}
