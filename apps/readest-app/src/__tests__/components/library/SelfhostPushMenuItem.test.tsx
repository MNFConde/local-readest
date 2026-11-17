/**
 * SelfhostPushMenuItem — the library settings-menu entry into the ⑬ push
 * dialog.
 *
 * Gate contract: visible only for a self-hosted build with a stored token
 * (re-read every render, mirroring AdminConsoleMenuItem); the dialog lists
 * only server-uploaded books, drives createPush with the picked recipient/
 * book/message, and surfaces errors.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';

const { createPush } = vi.hoisted(() => ({
  createPush: vi.fn(),
}));

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (s: string) => s,
}));

vi.mock('@/utils/supabase', () => ({ SELFHOSTED: true }));

vi.mock('@/store/libraryStore', () => ({
  useLibraryStore: {
    getState: () => ({
      library: [
        {
          hash: 'uploaded-1',
          title: 'Uploaded Book',
          uploadedAt: 1700000000000,
          deletedAt: null,
        },
        {
          hash: 'local-1',
          title: 'Local Only Book',
          uploadedAt: null,
          deletedAt: null,
        },
        {
          hash: 'tombstone-1',
          title: 'Deleted Book',
          uploadedAt: 1700000000000,
          deletedAt: 1700000001000,
        },
      ],
    }),
  },
}));

vi.mock('@/services/selfhostPush', () => ({
  SelfhostPushError: class extends Error {
    constructor(message: string) {
      super(message);
    }
  },
  createPush,
}));

import SelfhostPushMenuItem from '@/app/library/components/SelfhostPushMenuItem';
import { SelfhostPushError } from '@/services/selfhostPush';

describe('SelfhostPushMenuItem', () => {
  beforeEach(() => {
    localStorage.clear();
    createPush.mockReset().mockResolvedValue({ pushed: true, id: 1 });
  });
  afterEach(() => {
    cleanup();
    localStorage.clear();
  });

  it('stays hidden without a stored token', () => {
    const { container } = render(<SelfhostPushMenuItem />);
    expect(container.textContent).toBe('');
  });

  it('lists only uploaded books and pushes the picked one', async () => {
    localStorage.setItem('token', 'jwt');
    const onNavigated = vi.fn();
    render(<SelfhostPushMenuItem onNavigated={onNavigated} />);

    fireEvent.click(await screen.findByRole('menuitem', { name: 'Push a book' }));
    expect(onNavigated).toHaveBeenCalled();

    const select = screen.getByRole('combobox') as HTMLSelectElement;
    const options = Array.from(select.options).map((o) => o.value);
    expect(options).toEqual(['uploaded-1']);
    expect(select.value).toBe('uploaded-1');

    fireEvent.change(screen.getByLabelText('Recipient account'), {
      target: { value: 'bob@x.com' },
    });
    fireEvent.change(screen.getByLabelText('Message (optional)'), {
      target: { value: 'worth a read' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Push' }));

    await waitFor(() =>
      expect(createPush).toHaveBeenCalledWith({
        to: 'bob@x.com',
        bookHash: 'uploaded-1',
        message: 'worth a read',
      }),
    );
  });

  it('shows the empty note when nothing is uploaded', async () => {
    localStorage.setItem('token', 'jwt');
    const store = (await import('@/store/libraryStore')) as unknown as {
      useLibraryStore: { getState: () => { library: unknown[] } };
    };
    const original = store.useLibraryStore.getState;
    store.useLibraryStore.getState = () => ({ library: [] });
    try {
      render(<SelfhostPushMenuItem />);
      fireEvent.click(await screen.findByRole('menuitem', { name: 'Push a book' }));
      // Mocked t() returns the key verbatim; the locale mapping is i18n's job.
      expect(screen.getByText('No uploaded books to push')).toBeTruthy();
    } finally {
      store.useLibraryStore.getState = original;
    }
  });

  it('surfaces push errors inside the dialog', async () => {
    localStorage.setItem('token', 'jwt');
    createPush.mockRejectedValue(new SelfhostPushError('no such user', 404));
    render(<SelfhostPushMenuItem />);
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Push a book' }));

    fireEvent.change(screen.getByLabelText('Recipient account'), {
      target: { value: 'ghost@x.com' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Push' }));
    await waitFor(() => expect(screen.getByText('no such user')).toBeTruthy());
  });
});
