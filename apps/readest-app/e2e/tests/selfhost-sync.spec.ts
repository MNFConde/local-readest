import { test, expect } from '../fixtures/base';
import { LibraryPage } from '../pages/LibraryPage';
import { SAMPLE_EPUB } from '../fixtures/books';
import { startSyncServer, type SyncServerHandle } from '../selfhost-server';

/**
 * The sync closed loop against a real sync-server: device 1 logs in with the
 * runtime-entered server address + login code and pushes a book; device 2
 * logs in separately and pulls it. Covers /api/auth, the books sync channel
 * and the storage upload path through the actual client UI.
 */
test.describe('self-hosted sync', () => {
  test.describe.configure({ mode: 'serial' });

  let sync: SyncServerHandle;

  test.beforeAll(async () => {
    sync = await startSyncServer();
  });

  test.afterAll(async () => {
    sync?.stop();
  });

  async function login(browser: import('@playwright/test').Browser, label: string) {
    const ctx = await browser.newContext();
    // Suppress the demo-book auto-import so book counts stay deterministic.
    await ctx.addInitScript(() => {
      try {
        window.localStorage.setItem('demoBooksFetched', 'true');
      } catch {
        // ignore
      }
    });
    const page = await ctx.newPage();
    const library = new LibraryPage(page);

    await page.goto('/auth');
    const serverUrl = page.locator('#server-url');
    await serverUrl.waitFor({ state: 'visible' });
    await serverUrl.fill(sync.baseUrl);
    await page.locator('#password').fill(sync.loginCode);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await page.waitForURL(/\/library/, { timeout: 30_000 });

    return { ctx, page, library, label };
  }

  test('a book pushed by device 1 appears on device 2', async ({ browser }) => {
    const device1 = await login(browser, 'device-1');
    await device1.library.goto();
    await expect(device1.library.emptyState).toBeVisible();
    await device1.library.importBook(SAMPLE_EPUB);
    await expect(device1.page.getByRole('button', { name: /Alice/ })).toBeVisible();

    // Device 2 logs in separately and polls: reload, wait for the shelf, and
    // look for the synced book until the pull lands.
    const device2 = await login(browser, 'device-2');
    await device2.library.goto();

    await expect(async () => {
      await device2.page.reload();
      await device2.library.bookshelf.waitFor({ state: 'visible' });
      await expect(device2.page.getByRole('button', { name: /Alice/ })).toBeVisible();
    }).toPass({ timeout: 120_000, intervals: [5_000] });

    await device1.ctx.close();
    await device2.ctx.close();
  });

  test('a wrong login code is rejected with the exact error', async ({ browser }) => {
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    await page.goto('/auth');
    await page.locator('#server-url').fill(sync.baseUrl);
    await page.locator('#password').fill('wrong-code');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.getByText('Invalid login code')).toBeVisible({ timeout: 30_000 });
    await ctx.close();
  });
});
