'use client';

// Overview cards for the admin console (管理页可观测性): one read-only
// /api/admin/stats payload rendered as a card grid — account and catalog
// counts, on-disk sizes, blob dedup savings, server version + uptime.
// Self-contained like the rest of src/admin — no upstream imports.

import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from '@/hooks/useTranslation';
import { adminApi, AdminApiError, ServerStats } from './adminApi';
import { humanBytes } from './format';

// Stats move slowly; a one-minute refresh is plenty (the live monitor tab
// polls at 5s — that would be waste here).
const POLL_INTERVAL_MS = 60_000;

function humanDuration(secs?: number | null): string {
  if (secs == null || secs < 0) return '—';
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  if (d > 0) return `${d}d${h}h`;
  if (h > 0) return `${h}h${String(m).padStart(2, '0')}m`;
  if (m > 0) return `${m}m${String(s).padStart(2, '0')}s`;
  return `${s}s`;
}

function StatCard({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className='rounded-lg bg-base-200 p-3'>
      <div className='text-xs opacity-60'>{label}</div>
      <div className='mt-1 text-xl font-semibold'>{value}</div>
      {sub && <div className='mt-0.5 text-xs opacity-60'>{sub}</div>}
    </div>
  );
}

export default function StatsSection() {
  const _ = useTranslation('selfhost');
  const [stats, setStats] = useState<ServerStats | null>(null);
  const [state, setState] = useState<'loading' | 'ok' | 'error'>('loading');
  const [error, setError] = useState('');
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);

  const poll = useCallback(async () => {
    try {
      setStats(await adminApi.getStats());
      setState('ok');
    } catch (e) {
      setError(e instanceof AdminApiError ? e.message : String(e));
      setState('error');
    }
  }, []);

  useEffect(() => {
    poll();
    timer.current = setInterval(poll, POLL_INTERVAL_MS);
    return () => {
      if (timer.current) clearInterval(timer.current);
    };
  }, [poll]);

  return (
    <section className='card bg-base-100 shadow'>
      <div className='card-body gap-3 p-4'>
        <h2 className='card-title text-base'>{_('Overview')}</h2>

        {state === 'error' && (
          <div className='alert alert-error py-2 text-sm'>
            <span>{error}</span>
          </div>
        )}

        {state === 'loading' && <p className='text-sm opacity-70'>{_('Loading…')}</p>}

        {state === 'ok' && stats && (
          <>
            <div className='grid grid-cols-2 gap-3 md:grid-cols-3'>
              <StatCard
                label={_('Users')}
                value={String(stats.users.total)}
                sub={_('{{admins}} admin · {{disabled}} disabled', {
                  admins: stats.users.admins,
                  disabled: stats.users.disabled,
                })}
              />
              <StatCard
                label={_('Synced books')}
                value={String(stats.books.synced)}
                sub={_('across all accounts')}
              />
              <StatCard
                label={_('Catalog books')}
                value={String(stats.books.catalog)}
                sub={_('{{works}} works', { works: stats.books.works })}
              />
              <StatCard
                label={_('Stored files')}
                value={humanBytes(stats.storage.files_bytes)}
                sub={_('per-account copies')}
              />
              <StatCard
                label={_('Blob store')}
                value={humanBytes(stats.storage.blobs_bytes)}
                sub={_('{{count}} blobs', { count: stats.storage.blobs_count })}
              />
              <StatCard
                label={_('Dedup savings')}
                value={humanBytes(stats.storage.dedup_saved_bytes)}
                sub={_('stored once instead of per copy')}
              />
            </div>
            <p className='text-xs opacity-60'>
              {_('sync-server v{{version}} · up {{uptime}} · database {{db}}', {
                version: stats.server.version,
                uptime: humanDuration(stats.server.uptime_secs),
                db: humanBytes(stats.storage.db_bytes),
              })}
            </p>
          </>
        )}
      </div>
    </section>
  );
}
