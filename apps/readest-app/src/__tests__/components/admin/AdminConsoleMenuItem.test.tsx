/**
 * AdminConsoleMenuItem — the library settings-menu entry into /admin.
 *
 * Gate contract: visible only for a self-hosted build whose stored JWT carries
 * the admin role; the token is re-read on every render so a login that happens
 * after the menu mounted still surfaces the entry (and a logout hides it).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';

const { mockPush } = vi.hoisted(() => ({ mockPush: vi.fn() }));

vi.mock('next/navigation', () => ({
  useRouter: () => ({ push: mockPush }),
}));

vi.mock('@/hooks/useTranslation', () => ({
  useTranslation: () => (s: string) => s,
}));

vi.mock('@/utils/supabase', () => ({ SELFHOSTED: true }));

import AdminConsoleMenuItem from '@/admin/AdminConsoleMenuItem';

const b64url = (s: string) => Buffer.from(s, 'utf-8').toString('base64');
const mkJwt = (payload: object) =>
  b64url('{"alg":"HS256","typ":"JWT"}') + '.' + b64url(JSON.stringify(payload)) + '.sig';
const adminJwt = mkJwt({ sub: 'a', email: 'a@x', plan: 'pro', role: 'admin', exp: 9999999999 });
const userJwt = mkJwt({ sub: 'u', email: 'u@x', plan: 'pro', role: 'user', exp: 9999999999 });

describe('AdminConsoleMenuItem', () => {
  beforeEach(() => {
    localStorage.clear();
    mockPush.mockClear();
  });
  afterEach(() => {
    cleanup();
    localStorage.clear();
  });

  it('renders for an admin token and navigates to /admin on click', async () => {
    localStorage.setItem('token', adminJwt);
    const onNavigated = vi.fn();
    render(<AdminConsoleMenuItem onNavigated={onNavigated} />);

    const item = await screen.findByRole('menuitem', { name: 'Admin Console' });
    fireEvent.click(item);
    expect(mockPush).toHaveBeenCalledWith('/admin');
    expect(onNavigated).toHaveBeenCalled();
  });

  it('stays hidden for a plain-user token', () => {
    localStorage.setItem('token', userJwt);
    const { container } = render(<AdminConsoleMenuItem />);
    expect(container.textContent).toBe('');
  });

  it('stays hidden without a stored token', () => {
    const { container } = render(<AdminConsoleMenuItem />);
    expect(container.textContent).toBe('');
  });

  it('surfaces the entry when an admin signs in after the menu mounted', async () => {
    const { rerender } = render(<AdminConsoleMenuItem />);
    expect(screen.queryByText('Admin Console')).toBeNull();

    localStorage.setItem('token', adminJwt);
    rerender(<AdminConsoleMenuItem />);
    await waitFor(() =>
      expect(screen.getByRole('menuitem', { name: 'Admin Console' })).toBeTruthy(),
    );
  });

  it('hides the entry again after the token is cleared (logout)', async () => {
    localStorage.setItem('token', adminJwt);
    const { rerender } = render(<AdminConsoleMenuItem />);
    await screen.findByRole('menuitem', { name: 'Admin Console' });

    localStorage.removeItem('token');
    rerender(<AdminConsoleMenuItem />);
    await waitFor(() => expect(screen.queryByText('Admin Console')).toBeNull());
  });
});
