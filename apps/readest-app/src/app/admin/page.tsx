'use client';

// The single mount point of the admin console (project constraint: all admin
// UI lives in the self-contained src/admin/ subtree; this file is the only
// thing the upstream app router sees, keeping merge surface at zero).
// Reachable at /admin on any platform (web, desktop, Android) for accounts
// whose self-hosted JWT carries the admin role.

import AdminApp from '@/admin/AdminApp';

export default function AdminPage() {
  return <AdminApp />;
}
