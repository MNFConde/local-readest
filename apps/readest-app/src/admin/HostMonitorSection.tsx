'use client';

// Server monitor for the self-hosted admin console (ROADMAP 待办⑦): renders
// the metrics snapshot readest-supervisor pushes to the sync server every few
// seconds. Self-contained like the rest of src/admin — no upstream imports.
//
// Degradation is a first-class state, not an error: the supervisor only runs
// in the bundle deployments (and pushing needs ADMIN_TOKEN), so "not
// connected" simply means no supervisor feed — everything else on this page
// keeps working.

import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from '@/hooks/useTranslation';
import { adminApi, AdminApiError, HostServiceStatus, HostSnapshot } from './adminApi';

const POLL_INTERVAL_MS = 5000;

function humanBytes(bytes?: number): string {
  if (!bytes || bytes <= 0) return '—';
  const units = ['B', 'K', 'M', 'G', 'T'];
  let v = bytes;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return u === 0 ? `${bytes}B` : `${v.toFixed(1)}${units[u]}`;
}

function humanDuration(secs?: number | null): string {
  if (secs == null || secs < 0) return '—';
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  if (h > 0) return `${h}h${String(m).padStart(2, '0')}m${String(s).padStart(2, '0')}s`;
  if (m > 0) return `${m}m${String(s).padStart(2, '0')}s`;
  return `${s}s`;
}

function stateBadgeClass(state: string): string {
  if (state === 'running') return 'badge-success';
  if (state === 'restarting') return 'badge-warning';
  if (state === 'circuit open') return 'badge-error';
  if (state === 'stopped') return 'badge-ghost';
  return 'badge-info';
}

function ServiceRow({ service }: { service: HostServiceStatus }) {
  const _ = useTranslation('selfhost');
  const probe = service.probe;
  const probeText = !probe
    ? '—'
    : probe.ok
      ? `${probe.rtt_ms}ms`
      : probe.status === 0
        ? _('no answer')
        : `HTTP ${probe.status}`;
  return (
    <tr>
      <td className='font-mono'>{service.name}</td>
      <td>
        <span className={`badge badge-sm ${stateBadgeClass(service.state)}`}>{service.state}</span>
        {service.circuit_reason && (
          <div className='mt-1 max-w-md text-xs text-error' title={service.circuit_reason}>
            {service.circuit_reason}
          </div>
        )}
      </td>
      <td>{humanDuration(service.uptime_secs)}</td>
      <td>{service.restarts ?? 0}</td>
      <td>{service.pid ?? '—'}</td>
      <td>{humanBytes(service.memory_bytes)}</td>
      <td>{(service.cpu_percent ?? 0).toFixed(1)}%</td>
      <td>{probeText}</td>
    </tr>
  );
}

export default function HostMonitorSection() {
  const _ = useTranslation('selfhost');
  const [snapshot, setSnapshot] = useState<HostSnapshot | null>(null);
  const [ageSecs, setAgeSecs] = useState<number | null>(null);
  const [state, setState] = useState<'loading' | 'connected' | 'disconnected' | 'error'>('loading');
  const [error, setError] = useState('');
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);

  const poll = useCallback(async () => {
    try {
      const res = await adminApi.getHostSnapshot();
      if (res.available && res.snapshot) {
        setSnapshot(res.snapshot);
        setAgeSecs(res.age_secs ?? null);
        setState('connected');
      } else {
        setSnapshot(null);
        setState('disconnected');
      }
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

  const supervisor = snapshot?.supervisor;
  const host = snapshot?.host;
  const services = snapshot?.services ?? [];

  return (
    <section className='card bg-base-100 shadow'>
      <div className='card-body gap-3 p-4'>
        <h2 className='card-title text-base'>{_('Server Monitor')}</h2>

        {state === 'error' && (
          <div className='alert alert-error py-2 text-sm'>
            <span>{error}</span>
          </div>
        )}

        {state !== 'connected' && (
          <div className='alert py-2 text-sm opacity-70'>
            <span>
              {state === 'loading'
                ? _('Loading…')
                : _(
                    'Supervisor not connected — run readest-supervisor (bundle deployments) and set ADMIN_TOKEN to feed this panel.',
                  )}
            </span>
          </div>
        )}

        {state === 'connected' && snapshot && (
          <>
            <p className='text-sm opacity-70'>
              {_(
                'Updated {{age}}s ago · supervisor v{{version}} ({{mode}}) · {{os}} · {{cores}} cores · {{mem}} RAM',
                {
                  age: ageSecs ?? '–',
                  version: supervisor?.version ?? '?',
                  mode: supervisor?.headless ? _('headless') : _('dashboard'),
                  os: host?.os ?? '?',
                  cores: host?.cores ?? '?',
                  mem: humanBytes(host?.total_memory_bytes),
                },
              )}
            </p>
            <div className='overflow-x-auto'>
              <table className='table-zebra table table-sm'>
                <thead>
                  <tr>
                    <th>{_('Service')}</th>
                    <th>{_('State')}</th>
                    <th>{_('Uptime')}</th>
                    <th>{_('Restarts')}</th>
                    <th>PID</th>
                    <th>{_('Memory')}</th>
                    <th>CPU</th>
                    <th>{_('Latency')}</th>
                  </tr>
                </thead>
                <tbody>
                  {services.map((s) => (
                    <ServiceRow key={s.name} service={s} />
                  ))}
                </tbody>
              </table>
            </div>
            <p className='text-xs opacity-60'>
              {_(
                'Restart policy: exponential backoff (1s/4s/16s), circuit breaker after 5 rapid failures. Metrics arrive every 5s.',
              )}
            </p>
          </>
        )}
      </div>
    </section>
  );
}
