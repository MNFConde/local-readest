import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { Book } from '@/types/book';

// SELFHOSTED is a build-time constant read at module eval — flip the env
// before the component (and its supabase util) are imported.
vi.stubEnv('NEXT_PUBLIC_SELFHOSTED', '1');

const peekLibraryBooks = vi.fn();
const generateCoverImageUrl = vi.fn(async () => 'blob:cover');
const saveLibraryBooks = vi.fn(async () => {});
const fakeAppService = {
  peekLibraryBooks,
  generateCoverImageUrl,
  saveLibraryBooks,
};

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (key: string) => key,
}));
vi.mock('@/context/EnvContext', () => ({
  useEnv: () => ({
    envConfig: { getAppService: async () => fakeAppService },
    appService: fakeAppService,
  }),
}));
vi.mock('@/context/AuthContext', () => ({
  useAuth: () => ({ user: { id: 'uid-a', email: 'a@test.local' }, token: 'jwt' }),
}));
vi.mock('@/utils/event', () => ({
  eventDispatcher: { dispatch: vi.fn() },
}));

const { default: GuestShelfImport } = await import('@/app/library/components/GuestShelfImport');
const { useLibraryStore } = await import('@/store/libraryStore');
const { eventDispatcher } = await import('@/utils/event');

const book = (hash: string, title: string): Book =>
  ({ hash, title, author: 'Author', format: 'EPUB', createdAt: 1, updatedAt: 1 }) as Book;

describe('GuestShelfImport', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useLibraryStore.setState({
      library: [book('owned1', 'Owned Book')],
      libraryLoaded: true,
    });
  });

  afterEach(cleanup);

  test('offers only guest-shelf books this account does not have yet', async () => {
    peekLibraryBooks.mockResolvedValue([
      book('guest1', 'Guest Book'),
      book('owned1', 'Already Here'),
      { ...book('gone1', 'Deleted There'), deletedAt: 123 },
    ]);
    render(<GuestShelfImport />);
    await waitFor(() => {
      expect(screen.getByText(/guest shelf has {{count}} book/)).toBeTruthy();
    });
    fireEvent.click(screen.getByRole('button', { name: 'Import' }));
    await waitFor(() => {
      expect(screen.getByText('Import from Guest Shelf')).toBeTruthy();
    });
    expect(screen.getByText('Guest Book')).toBeTruthy();
    expect(screen.queryByText('Already Here')).toBeNull();
    expect(screen.queryByText('Deleted There')).toBeNull();
  });

  test('renders nothing when the guest shelf has nothing to import', async () => {
    peekLibraryBooks.mockResolvedValue([book('owned1', 'Already Here')]);
    const { container } = render(<GuestShelfImport />);
    await waitFor(() => {
      expect(peekLibraryBooks).toHaveBeenCalledWith(null);
    });
    expect(container.innerHTML).toBe('');
  });

  test('importing merges the selection into the account shelf', async () => {
    peekLibraryBooks.mockResolvedValue([book('guest1', 'Guest Book'), book('guest2', 'Other')]);
    render(<GuestShelfImport />);
    await screen.findByText(/guest shelf has {{count}} book/);
    fireEvent.click(screen.getByRole('button', { name: 'Import' }));
    await screen.findByText('Import from Guest Shelf');

    // Toggle one book off and back on, then import the full selection.
    const checkboxes = screen.getAllByRole('checkbox') as HTMLInputElement[];
    expect(checkboxes).toHaveLength(2);
    expect(checkboxes.every((box) => box.checked)).toBe(true);
    fireEvent.click(checkboxes[1]!);
    expect(checkboxes[1]!.checked).toBe(false);
    fireEvent.click(checkboxes[1]!);
    expect(checkboxes[1]!.checked).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: /Import {{count}} book/ }));

    await waitFor(() => {
      expect(saveLibraryBooks).toHaveBeenCalledTimes(1);
    });
    const library = useLibraryStore.getState().library;
    expect(library.map((b) => b.hash).sort()).toEqual(['guest1', 'guest2', 'owned1']);
    expect(eventDispatcher.dispatch).toHaveBeenCalledWith(
      'toast',
      expect.objectContaining({ type: 'success' }),
    );
    // The banner is gone once everything importable is imported.
    await waitFor(() => {
      expect(screen.queryByText(/guest shelf has/)).toBeNull();
    });
  });
});
