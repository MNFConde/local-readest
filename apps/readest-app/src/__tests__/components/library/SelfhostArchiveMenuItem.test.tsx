/**
 * SelfhostArchiveMenuItem — the library settings-menu entry into the ⑫
 * archive transfer dialog.
 *
 * Gate contract: visible only for a self-hosted build with a stored token
 * (re-read every render, mirroring AdminConsoleMenuItem); the dialog drives
 * exportArchive/importArchive and surfaces the import summary or error.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';

const { exportArchive, importArchive } = vi.hoisted(() => ({
  exportArchive: vi.fn(),
  importArchive: vi.fn(),
}));

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (s: string) => s,
}));

vi.mock('@/utils/supabase', () => ({ SELFHOSTED: true }));

vi.mock('@/services/selfhostArchive', () => ({
  SelfhostArchiveError: class extends Error {
    constructor(message: string) {
      super(message);
    }
  },
  exportArchive,
  importArchive,
}));

import SelfhostArchiveMenuItem from '@/app/library/components/SelfhostArchiveMenuItem';
import { SelfhostArchiveError } from '@/services/selfhostArchive';

describe('SelfhostArchiveMenuItem', () => {
  beforeEach(() => {
    localStorage.clear();
    exportArchive.mockReset().mockResolvedValue('readest-archive.tar');
    importArchive.mockReset().mockResolvedValue({ books: 2, filesLinked: 1 });
  });
  afterEach(() => {
    cleanup();
    localStorage.clear();
  });

  it('stays hidden without a stored token', () => {
    const { container } = render(<SelfhostArchiveMenuItem />);
    expect(container.textContent).toBe('');
  });

  it('opens the dialog and exports with the selected scope', async () => {
    localStorage.setItem('token', 'jwt');
    const onNavigated = vi.fn();
    render(<SelfhostArchiveMenuItem onNavigated={onNavigated} />);

    fireEvent.click(await screen.findByRole('menuitem', { name: 'Archive Transfer' }));
    expect(onNavigated).toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Export archive' })).toBeTruthy();

    // Default is the full scope; the metadata radio switches the call.
    fireEvent.click(screen.getByRole('button', { name: 'Export archive' }));
    await waitFor(() => expect(exportArchive).toHaveBeenCalledWith('full'));

    fireEvent.click(screen.getByLabelText('Metadata only (records, no book files)'));
    fireEvent.click(screen.getByRole('button', { name: 'Export archive' }));
    await waitFor(() => expect(exportArchive).toHaveBeenCalledWith('metadata'));
  });

  it('imports a picked tar and shows the summary', async () => {
    localStorage.setItem('token', 'jwt');
    render(<SelfhostArchiveMenuItem />);
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Archive Transfer' }));

    const file = new File(['tar-bytes'], 'readest-archive.tar', { type: 'application/x-tar' });
    const input = document.querySelector('input[type=file]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [file] } });
    fireEvent.click(screen.getByRole('button', { name: 'Import archive' }));

    await waitFor(() => expect(importArchive).toHaveBeenCalledWith(file));
    // The mocked t() returns keys verbatim, so the summary line is the raw
    // key — the numbers are covered by the service test.
    expect(screen.getByText('Imported {{books}} book(s), linked {{files}} file(s).')).toBeTruthy();
  });

  it('surfaces export errors inside the dialog', async () => {
    localStorage.setItem('token', 'jwt');
    exportArchive.mockRejectedValue(new SelfhostArchiveError('Cannot reach the sync server', 0));
    render(<SelfhostArchiveMenuItem />);
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Archive Transfer' }));

    fireEvent.click(screen.getByRole('button', { name: 'Export archive' }));
    await waitFor(() => expect(screen.getByText('Cannot reach the sync server')).toBeTruthy());
  });
});
