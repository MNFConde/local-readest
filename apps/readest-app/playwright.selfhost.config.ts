import { defineConfig, devices } from '@playwright/test';

/**
 * Self-hosted sync e2e lane.
 *
 * Drives the Next.js production build started with the self-hosted env
 * (`pnpm start-server`, i.e. `.env.server` with `NEXT_PUBLIC_SELFHOSTED=1`)
 * against a real sync-server binary spawned by the spec (see
 * `e2e/selfhost-server.ts`). Two browser contexts act as two devices and
 * verify the sync closed loop end to end.
 *
 *   pnpm build-server && pnpm test:e2e:selfhost
 */
const PORT = 3000;

export default defineConfig({
  testDir: './e2e/tests',
  testMatch: /selfhost.*\.spec\.ts/,
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 2 : 0,
  reporter: [['list']],
  timeout: 180_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        // The dev machine may run a system proxy; the app must reach the
        // local sync-server and the app server directly.
        launchOptions: { args: ['--no-proxy-server'] },
      },
    },
  ],
  webServer: {
    command: 'pnpm start-e2e-selfhost',
    port: PORT,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
