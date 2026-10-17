#!/usr/bin/env node
/**
 * Assemble the Windows self-host server bundle (ROADMAP 待办⑥+⑦ / 方案 B):
 *
 *   readest-supervisor.exe   THE entry point: spawns and supervises both
 *                            services (backoff restarts + circuit breaker,
 *                            TUI dashboard, both children die with it)
 *   sync-server.exe          sync/auth/OPDS/admin API + data      → port 8080
 *   web/                     Next.js standalone server (web reader, /admin)
 *                            — plain `node`, no pnpm, no source tree → port 3000
 *   start.bat / stop.bat     open the supervisor dashboard / stop everything
 *
 * Inputs (all must exist before this script runs):
 *   sync-server/target/release/sync-server.exe
 *       cargo build --release   (run inside sync-server/)
 *   sync-server/supervisor/target/release/readest-supervisor.exe
 *       cargo build --release   (run inside sync-server/supervisor/)
 *   apps/readest-app/.next/standalone/apps/readest-app/server.js
 *   apps/readest-app/.next/static/
 *   apps/readest-app/public/
 *       BUILD_STANDALONE=true NEXT_PUBLIC_SELFHOSTED=1 pnpm build-web
 *       (the self-host flags are compile-time inlined: the bundle's web
 *        reader logs in against the operator's sync server)
 *
 * Output: build/selfhost-server/ and, with --zip,
 *         build/readest-selfhost-server_<version>_x86_64-pc-windows-msvc.zip
 *         (the same name the release-selfhost pipeline attaches to releases).
 */

import { execSync, spawnSync } from 'node:child_process';
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const appDir = path.join(root, 'apps', 'readest-app');
const outDir = path.join(root, 'build', 'selfhost-server');
const zipIt = process.argv.includes('--zip');

const inputs = {
  serverExe: path.join(root, 'sync-server', 'target', 'release', 'sync-server.exe'),
  supervisorExe: path.join(root, 'sync-server', 'supervisor', 'target', 'release', 'readest-supervisor.exe'),
  standalone: path.join(appDir, '.next', 'standalone'),
  staticDir: path.join(appDir, '.next', 'static'),
  publicDir: path.join(appDir, 'public'),
};

const die = (msg) => {
  console.error(`package-server-bundle: ${msg}`);
  process.exit(1);
};

for (const [name, p] of Object.entries(inputs)) {
  if (!existsSync(p)) {
    die(`missing ${name}: ${p}\n` +
      '  build them first:\n' +
      '    cd sync-server && cargo build --release\n' +
      '    cd sync-server/supervisor && cargo build --release\n' +
      '    cd apps/readest-app && NEXT_PUBLIC_SELFHOSTED=1 BUILD_STANDALONE=true pnpm build-web');
  }
}
if (!existsSync(path.join(inputs.standalone, 'apps', 'readest-app', 'server.js'))) {
  // The standalone tree is monorepo-rooted (outputFileTracingRoot); an
  // app-rooted layout means a future Next.js changed the shape — stop rather
  // than ship a bundle whose start.bat points at a missing entrypoint.
  die('unexpected standalone layout: apps/readest-app/server.js not found under .next/standalone');
}

const { version } = JSON.parse(readFileSync(path.join(appDir, 'package.json'), 'utf8'));

console.log(`Assembling self-host server bundle (v${version}) into ${outDir}`);
// Phased logging + full-stack die(): this script runs unattended in CI, and a
// phase that dies silently (exit 127 with no output, as on the 0.12.14-2026101712
// runner) is undebuggable — every step below must be attributable in the log.
const step = (name, fn) => {
  console.log(`[bundle] ${name}...`);
  try {
    fn();
  } catch (error) {
    die(`${name} failed: ${error?.stack ?? error}`);
  }
};

step('clear previous output', () => {
  try {
    rmSync(outDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 300 });
  } catch {
    // A previous start.bat still running from this directory holds the dir
    // lock; say so instead of leaking a bare EPERM.
    die(`${outDir} is locked — stop the running bundle first (stop.bat in it)`);
  }
  mkdirSync(outDir, { recursive: true });
});

// The web server: standalone tree + the traced static/public assets Next does
// not include. Same layout the Docker image assembles (see Dockerfile).
//
// The standalone tree is full of pnpm symlinks/junctions into the build
// machine's store, and the copy must materialize them as real files (the
// release zip cannot carry links). Two native tools already failed on the
// CI runner: node cpSync(dereference) crashed the process (silent exit 127,
// twice), and robocopy /MT failed with exit 9 while losing its own error
// output. bsdtar with -h (follow links) through a temp archive is the third
// option: iterative traversal, no console-handle quirks, full stderr.
const runOrDie = (name, exe, args) => {
  const result = spawnSync(exe, args, { encoding: 'utf8' });
  if (result.error) die(`${name} failed to start: ${result.error.stack}`);
  if (result.status !== 0) {
    die(`${name} exited ${result.status}\n--- stdout tail ---\n` +
      (result.stdout ?? '').split('\n').slice(-20).join('\n') +
      '\n--- stderr tail ---\n' +
      (result.stderr ?? '').split('\n').slice(-20).join('\n'));
  }
};
step('copy standalone web tree (bsdtar -h materializes pnpm links)', () => {
  const bsdtar = path.join(process.env.WINDIR ?? 'C:\\Windows', 'System32', 'tar.exe');
  const webDir = path.join(outDir, 'web');
  const tmpTar = path.join(root, 'build', 'web-tree.tar');
  rmSync(tmpTar, { force: true });
  runOrDie('bsdtar create', bsdtar, ['-h', '-cf', tmpTar, '-C', inputs.standalone, '.']);
  mkdirSync(webDir, { recursive: true });
  runOrDie('bsdtar extract', bsdtar, ['-xf', tmpTar, '-C', webDir]);
  rmSync(tmpTar, { force: true });
});
step('copy static assets', () =>
  cpSync(inputs.staticDir, path.join(outDir, 'web', 'apps', 'readest-app', '.next', 'static'), {
    recursive: true,
  }),
);
step('copy public assets', () =>
  cpSync(inputs.publicDir, path.join(outDir, 'web', 'apps', 'readest-app', 'public'), {
    recursive: true,
  }),
);

// Flatten the store into a plain npm-style `node_modules` so the Node
// resolver can walk its way to every transitive dep from anywhere in the
// tree — and so the release zip holds real files only: Explorer/tar cannot
// recreate junctions without admin or Developer Mode, so any link shipped
// in the zip dangles for most users. First store entry wins on version
// collisions, like npm hoisting.
//
// Read from the SOURCE standalone tree, never from the extracted copy:
// bsdtar -h materializes symlinks but ships junction-style reparse points
// verbatim, and on the CI runner the extracted .pnpm then contains entries
// stat can't even touch (EPERM) — which is also what natively crashed the
// earlier cpSync(dereference) pass. On the source side every link resolves
// against the build machine, so flatten is a plain walk of real content.
const srcStore = path.join(inputs.standalone, 'node_modules', '.pnpm');
const flatRoot = path.join(outDir, 'web', 'node_modules');
if (!existsSync(srcStore)) {
  die(`no .pnpm store under ${inputs.standalone}/node_modules — pnpm's standalone ` +
    'node_modules layout changed; the flatten step would ship a tree that cannot resolve');
}
const flattenPackageDir = (from, to) => {
  // Scoped names hit this twice: the `@scope` dir is merged per child so a
  // second store entry contributing another package under the same scope
  // still lands.
  if (path.basename(to).startsWith('@')) {
    mkdirSync(to, { recursive: true });
    for (const sub of readdirSync(from)) {
      const subTo = path.join(to, sub);
      if (existsSync(subTo)) continue;
      cpSync(path.join(from, sub), subTo, { recursive: true, dereference: true });
    }
    return;
  }
  if (existsSync(to)) return;
  cpSync(from, to, { recursive: true, dereference: true });
};
step('flatten .pnpm store into plain node_modules (from source)', () => {
  rmSync(flatRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 300 });
  mkdirSync(flatRoot, { recursive: true });
  for (const storeEntry of readdirSync(srcStore)) {
    // `storeEntry` is either `<name>@<version>[<peer-hash>]` or the store's
    // own hoisted-fallback `node_modules` directory.
    const pkgsDir =
      storeEntry === 'node_modules'
        ? path.join(srcStore, storeEntry)
        : path.join(srcStore, storeEntry, 'node_modules');
    let names;
    try {
      names = readdirSync(pkgsDir);
    } catch {
      continue;
    }
    for (const name of names) {
      if (name === '.bin') continue;
      const from = path.join(pkgsDir, name);
      let stat;
      try {
        stat = statSync(from);
      } catch {
        continue; // unreadable even at the source: nothing sane to ship
      }
      if (!stat.isDirectory()) continue;
      flattenPackageDir(from, path.join(flatRoot, name));
    }
  }
});
// The extracted copy's app-scoped node_modules is links/reparse points into
// the (now deleted) .pnpm tree. Rebuild it from the flat root, restricted to
// the packages the standalone's app scope actually listed.
step('rebuild app-scoped node_modules as real dirs', () => {
  const appNm = path.join(outDir, 'web', 'apps', 'readest-app', 'node_modules');
  rmSync(appNm, { recursive: true, force: true, maxRetries: 5, retryDelay: 300 });
  mkdirSync(appNm, { recursive: true });
  const srcAppNm = path.join(inputs.standalone, 'apps', 'readest-app', 'node_modules');
  for (const name of readdirSync(srcAppNm)) {
    if (name === '.bin' || name === '.pnpm') continue;
    const flat = path.join(flatRoot, name);
    if (existsSync(flat)) {
      cpSync(flat, path.join(appNm, name), { recursive: true, dereference: true });
    } else {
      // Package listed at app scope but not hoisted to the flat root — take
      // it straight from the source, dereferencing against the build machine.
      cpSync(path.join(srcAppNm, name), path.join(appNm, name), {
        recursive: true,
        dereference: true,
      });
    }
  }
});
step('copy exes', () => {
  copyFileSync(inputs.serverExe, path.join(outDir, 'sync-server.exe'));
  copyFileSync(inputs.supervisorExe, path.join(outDir, 'readest-supervisor.exe'));
});

const CRLF = (s) => s.replace(/\n/g, '\r\n') + '\r\n';

const startBat = CRLF(`@echo off
rem Readest self-host server (ROADMAP ⑦): readest-supervisor starts BOTH
rem services (sync server :8080 + web reader :3000) and supervises them —
rem crash-restarts with backoff, live dashboard, and both services stop the
rem moment the supervisor itself stops. Requires Node.js (>= 20) on PATH.
setlocal
cd /d "%~dp0"
start "Readest Server" readest-supervisor.exe
echo.
echo   Web reader     :  http://localhost:3000/
echo   Admin console  :  http://localhost:3000/admin   (admin account required)
echo   Sync server    :  http://localhost:8080/api/health
echo   Data directory :  %~dp0data
echo.
echo   The dashboard runs in its own window ("Readest Server"): live status,
echo   latency and memory of both services; press q there to stop everything.
echo   Headless mode (services / task scheduler):  readest-supervisor.exe --headless
echo   First admin account (run here once):
echo       sync-server.exe add-user ^<email^> --admin
echo   Desktop/Android clients use  http://<this-pc-ip>:8080  as server address.
echo.
endlocal
`);

const stopBat = CRLF(`@echo off
rem The supervisor owns both services (Windows Job Object): killing it takes
rem them down with it. The two extra kills below only catch services that
rem were started manually outside the supervisor.
taskkill /IM readest-supervisor.exe /F >nul 2>&1
taskkill /IM sync-server.exe /F >nul 2>&1
powershell -NoProfile -Command "Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'node.exe' -and $_.CommandLine -like '*readest-app\\server.js*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }" >nul 2>&1
echo Readest self-host server stopped.
`);

const readme = CRLF(`Readest Self-Host Server ${version} (Windows x64)
==========================================

What is inside
  readest-supervisor.exe  THE entry point: starts and supervises both
                    services. Dashboard TUI by default (--headless for
                    services); restarts crashed services with backoff
                    (1s/4s/16s, gives up after 5 rapid failures and says
                    why); both services stop together with it.
  sync-server.exe   Sync/auth/OPDS/admin API + book storage (port 8080).
  web\\              Web reader + admin console, standalone Next.js server
                    (port 3000). Needs Node.js >= 20 on PATH; no pnpm needed.
  start.bat         Opens the supervisor dashboard. stop.bat stops everything.
  logs\\             Per-service log files (also shown in the dashboard).
  data\\             Created on first start: SQLite DB + book files.

First run
  1. Double-click start.bat (the dashboard window opens).
  2. Open a terminal in this folder and create the admin account:
       sync-server.exe add-user you@example.com --admin
     (--admin marks the account as administrator; plain users can be created
     from the admin console or without the flag.)
  3. Web reader + admin console:  http://localhost:3000  /  http://localhost:3000/admin
     Login there with the server address  http://localhost:8080  plus the
     account and password from step 2. The admin console's "Server Monitor"
     tab shows live service status when ADMIN_TOKEN is set (see below).
  4. Desktop/Android apps: set the sync server address to
     http://<this-pc-ip>:8080 (same account). LAN access requires a firewall
     rule allowing inbound TCP 8080 and 3000.

Configuration (same .env file as the sync server, beside the exes)
  LISTEN_ADDR   Sync server bind address   (default 0.0.0.0:8080)
  PORT / HOSTNAME  Web server bind         (default 3000 / 0.0.0.0)
  DATA_DIR      Data directory             (default .\\data)
  QUOTA_BYTES   Per-user storage quota     (default 2 GiB)
  ADMIN_TOKEN   Break-glass API token for /api/admin; ALSO enables the
                supervisor's metrics feed for the admin console's
                Server Monitor tab (pushed locally every 5s)
  READEST_NODE  Path to the node executable (default: node.exe beside the
                exes if present, else PATH)

Run as a service (optional)
  Use the supervisor's headless mode, not the dashboard:
    NSSM:  nssm install ReadestServer "<path>\\readest-supervisor.exe" --headless
    schtasks /Create /TN ReadestServer /SC ONLOGON /TR "<path>\\readest-supervisor.exe --headless"
  (The supervisor keeps both services alive; whatever restarts the supervisor
  itself covers the last gap.)

Updating
  Replace readest-supervisor.exe, sync-server.exe and web\\ with the new
  release's copies; data\\ is compatible across releases and migrations run
  automatically at boot.
`);

writeFileSync(path.join(outDir, 'start.bat'), startBat);
writeFileSync(path.join(outDir, 'stop.bat'), stopBat);
writeFileSync(path.join(outDir, 'README.txt'), readme);

let zipPath = null;
if (zipIt) {
  const archiveName = `readest-selfhost-server_${version}_x86_64-pc-windows-msvc.zip`;
  zipPath = path.join(root, 'build', archiveName);
  rmSync(zipPath, { force: true });
  // Windows ships bsdtar (which `-a` turns into a zip by extension) in
  // System32, but a Git-Bash PATH usually has GNU tar first — and GNU tar
  // misreads `D:\...` as a remote host ("Cannot connect to D"). Pin the
  // absolute path; fall back to PowerShell where System32 tar is absent.
  const bsdtar = path.join(process.env.WINDIR ?? 'C:\\Windows', 'System32', 'tar.exe');
  const cmd = existsSync(bsdtar)
    ? `"${bsdtar}" -a -c -f "${zipPath}" -C "${path.dirname(outDir)}" "${path.basename(outDir)}"`
    : `powershell -NoProfile -Command "Compress-Archive -Path '${outDir}' -DestinationPath '${zipPath}' -Force"`;
  step(`zip bundle via ${existsSync(bsdtar) ? 'bsdtar' : 'Compress-Archive'}`, () => {
    try {
      execSync(cmd, { stdio: 'inherit' });
    } catch (error) {
      die(`zip command failed (status ${error?.status}): ${error?.stack ?? error}`);
    }
    if (!existsSync(zipPath)) die(`zip command reported success but ${zipPath} is missing`);
  });
}

console.log('Bundle ready:');
console.log(`  ${outDir}`);
if (zipPath) console.log(`  ${zipPath}`);
