import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(HERE, '..', '..', '..');
const SERVER_DIR = path.join(REPO_ROOT, 'sync-server');

const LOGIN_CODE = 'e2e-code-0123456789abcdef';
// Random per-run port so a stale server from a crashed previous run can
// never serve stale data to this run.
const PORT = 20000 + Math.floor(Math.random() * 20000);

export interface SyncServerHandle {
  baseUrl: string;
  loginCode: string;
  stop: () => void;
}

function serverBinary(): string {
  const exe = process.platform === 'win32' ? 'sync-server.exe' : 'sync-server';
  const release = path.join(SERVER_DIR, 'target', 'release', exe);
  if (existsSync(release)) return release;
  const debug = path.join(SERVER_DIR, 'target', 'debug', exe);
  if (existsSync(debug)) return debug;
  // First run on a fresh checkout: build it (release keeps single-exe parity).
  spawnSync('cargo', ['build', '--release'], { cwd: SERVER_DIR, stdio: 'inherit', shell: true });
  if (!existsSync(release)) {
    throw new Error(`sync-server binary not found at ${release} after cargo build`);
  }
  return release;
}

async function waitForHealth(baseUrl: string, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`${baseUrl}/api/health`);
      if (res.ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error(`sync-server did not become healthy at ${baseUrl}`);
}

/**
 * Spawn a throwaway sync-server on 127.0.0.1:<PORT> with a temp data dir and
 * a single seeded account, for the selfhost e2e lane. `stop()` kills it and
 * removes the data dir.
 */
export async function startSyncServer(): Promise<SyncServerHandle> {
  const bin = serverBinary();
  const dataDir = mkdtempSync(path.join(os.tmpdir(), 'sync-server-e2e-'));
  const env = {
    ...process.env,
    JWT_SECRET: 'e2e-jwt-secret-0123456789abcdef',
    DATA_DIR: dataDir,
    LISTEN_ADDR: `127.0.0.1:${PORT}`,
  };

  // Seed the account against the same data dir the server will use.
  spawnSync(bin, ['add-user', 'e2e@readest.test', '--code', LOGIN_CODE], {
    env,
    stdio: 'inherit',
    shell: true,
  });

  const child = spawn(bin, ['serve'], { env, stdio: 'inherit', shell: true });
  const baseUrl = `http://127.0.0.1:${PORT}`;
  await waitForHealth(baseUrl);

  return {
    baseUrl,
    loginCode: LOGIN_CODE,
    stop: () => {
      child.kill();
      // Windows keeps file handles alive briefly after kill; best-effort cleanup.
      // Data dir is intentionally kept for post-mortem debugging; the OS temp
      // dir sweep will clean these up.
      void dataDir;
    },
  };
}
