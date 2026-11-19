import React, { useState } from 'react';
import { useTranslation } from '@/hooks/useTranslation';
import {
  LOCALSEND_DEFAULT_PORT,
  connectManualLocalSend,
  parseManualTarget,
} from './localsendManual';

const errorText = (err: unknown) => (err instanceof Error ? err.message : String(err));

/**
 * Manual connect row for the nearby-send picker (fork, ⑪-P3): peers that
 * multicast discovery cannot reach are dialed by host:port once, then behave
 * like discovered devices in the grid above. Collapsed by default so the
 * picker stays identical to the discovery-only layout until asked for.
 */
const ManualConnectRow: React.FC = () => {
  const _ = useTranslation('selfhost');
  const [expanded, setExpanded] = useState(false);
  const [value, setValue] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [connected, setConnected] = useState<string | null>(null);

  const connect = async () => {
    const target = parseManualTarget(value);
    if (!target) {
      setError(_('Enter a valid address like 192.168.1.7:53317'));
      return;
    }
    setBusy(true);
    setError(null);
    setConnected(null);
    try {
      const peer = await connectManualLocalSend(target.host, target.port ?? LOCALSEND_DEFAULT_PORT);
      setConnected(peer.alias);
      setValue('');
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  if (!expanded) {
    return (
      <div className='border-base-content/10 flex justify-center border-t pt-2'>
        <button
          type='button'
          className='text-base-content/50 btn btn-ghost btn-xs'
          onClick={() => setExpanded(true)}
        >
          {_('Add device manually')}
        </button>
      </div>
    );
  }

  return (
    <div className='border-base-content/10 flex flex-col gap-1 border-t pt-2'>
      <div className='flex items-center gap-2'>
        <input
          type='text'
          className='input input-bordered input-sm h-8 grow'
          placeholder={_('Host or IP, e.g. 192.168.1.7:53317')}
          value={value}
          disabled={busy}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void connect();
          }}
        />
        <button
          type='button'
          className='btn btn-neutral btn-sm h-8'
          disabled={busy || !value.trim()}
          onClick={() => void connect()}
        >
          {_('Connect')}
        </button>
        <button
          type='button'
          className='btn btn-ghost btn-sm h-8 px-2'
          aria-label={_('Collapse manual connect')}
          onClick={() => {
            setExpanded(false);
            setError(null);
            setConnected(null);
          }}
        >
          ✕
        </button>
      </div>
      {error && <span className='text-error px-1 text-xs'>{error}</span>}
      {connected && (
        <span className='text-success px-1 text-xs'>
          {_('Connected to {{alias}} — pick it from the list above', { alias: connected })}
        </span>
      )}
    </div>
  );
};

export default ManualConnectRow;
