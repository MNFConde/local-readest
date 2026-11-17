/**
 * SelfhostInboxMenuItem — the library settings-menu entry into the ⑬
 * recommended-inbox dialog.
 *
 * Gate contract: visible only for a self-hosted build with a stored token;
 * the pending count badges the menu item (fetched on mount / dialog open,
 * no polling); decisions drive acceptPush/declinePush and refresh the list.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';

const { acceptPush, declinePush, listInbox } = vi.hoisted(() => ({
  acceptPush: vi.fn(),
  declinePush: vi.fn(),
  listInbox: vi.fn(),
}));

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (s: string) => s,
}));

vi.mock('@/utils/supabase', () => ({ SELFHOSTED: true }));

vi.mock('@/services/selfhostPush', () => ({
  SelfhostPushError: class extends Error {
    constructor(message: string) {
      super(message);
    }
  },
  acceptPush,
  declinePush,
  listInbox,
}));

import SelfhostInboxMenuItem from '@/app/library/components/SelfhostInboxMenuItem';
import { SelfhostPushError } from '@/services/selfhostPush';

const ITEM = {
  id: 1,
  from: 'alice@x.com',
  bookHash: 'h1',
  title: 'Some Book',
  author: 'Someone',
  message: 'worth a read',
  createdAt: '2026-11-18T00:00:00Z',
};

describe('SelfhostInboxMenuItem', () => {
  beforeEach(() => {
    localStorage.clear();
    acceptPush.mockReset().mockResolvedValue({ accepted: true, filesLinked: 2, filesSkipped: 0 });
    declinePush.mockReset().mockResolvedValue({ declined: 1 });
    listInbox.mockReset().mockResolvedValue([ITEM]);
  });
  afterEach(() => {
    cleanup();
    localStorage.clear();
  });

  it('stays hidden without a stored token', () => {
    const { container } = render(<SelfhostInboxMenuItem />);
    expect(container.textContent).toBe('');
  });

  it('badges the menu item with the pending count and lists the offer', async () => {
    localStorage.setItem('token', 'jwt');
    render(<SelfhostInboxMenuItem />);

    fireEvent.click(await screen.findByRole('menuitem', { name: 'Recommended inbox (1)' }));
    expect(screen.getByText('Some Book')).toBeTruthy();
    // Mocked t() returns keys verbatim (no interpolation).
    expect(screen.getByText('From {{user}}')).toBeTruthy();
    expect(screen.getByText('worth a read')).toBeTruthy();
    await waitFor(() => expect(listInbox).toHaveBeenCalled());
  });

  it('accepting an offer calls acceptPush, shows the summary, and refreshes', async () => {
    localStorage.setItem('token', 'jwt');
    render(<SelfhostInboxMenuItem />);
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Recommended inbox (1)' }));

    fireEvent.click(screen.getByRole('button', { name: 'Accept' }));
    await waitFor(() => expect(acceptPush).toHaveBeenCalledWith(1));
    // The mocked t() returns keys verbatim; the number mapping is covered by
    // the service test.
    expect(screen.getByText('Accepted, linked {{files}} file(s).')).toBeTruthy();
    await waitFor(() => expect(listInbox.mock.calls.length).toBeGreaterThanOrEqual(2));
  });

  it('declining an offer calls declinePush without accepting', async () => {
    localStorage.setItem('token', 'jwt');
    render(<SelfhostInboxMenuItem />);
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Recommended inbox (1)' }));

    fireEvent.click(screen.getByRole('button', { name: 'Decline' }));
    await waitFor(() => expect(declinePush).toHaveBeenCalledWith(1));
    expect(acceptPush).not.toHaveBeenCalled();
  });

  it('shows the empty state when nothing is pending', async () => {
    localStorage.setItem('token', 'jwt');
    listInbox.mockResolvedValue([]);
    render(<SelfhostInboxMenuItem />);
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Recommended inbox' }));

    expect(screen.getByText('No pending recommendations')).toBeTruthy();
  });

  it('surfaces fetch errors inside the dialog', async () => {
    localStorage.setItem('token', 'jwt');
    listInbox.mockRejectedValue(new SelfhostPushError('Cannot reach the sync server', 0));
    render(<SelfhostInboxMenuItem />);
    // The badge fetch error only renders inside the dialog; opening it
    // triggers a fresh fetch that rejects again.
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Recommended inbox' }));

    await waitFor(() => expect(screen.getByText('Cannot reach the sync server')).toBeTruthy());
  });
});
