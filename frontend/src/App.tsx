import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, setApiKey } from './api';
import { formatSize, timeAgo, toast, useToasts } from './util';
import { Icon } from './components/common';
import Library from './components/Library';
import Queue from './components/Queue';
import Settings from './components/Settings';

type Tab = 'library' | 'queue' | 'settings';

export default function App() {
  const qc = useQueryClient();
  const [tab, setTab] = useState<Tab>('library');
  const status = useQuery({ queryKey: ['status'], queryFn: api.status, refetchInterval: 20_000 });
  const toasts = useToasts();

  const sync = useMutation({
    mutationFn: api.sync,
    onSuccess: (res) => {
      qc.invalidateQueries();
      if (res.errors.length) toast(res.errors.join('\n'));
      else toast('Library synced', 'ok');
    },
    onError: (e: Error) => toast(e.message),
  });

  if (status.error instanceof ApiError && status.error.status === 401) {
    return <KeyPrompt onSaved={() => qc.invalidateQueries()} />;
  }

  const s = status.data;
  const configured = !!s && (s.services.radarr || s.services.sonarr);
  const pending = s?.pending.count ?? 0;

  return (
    <>
      <header className="topbar">
        <div className="brand">
          <span className="logo">
            <Icon name="trash" size={16} />
          </span>
          Plex Delete List
        </div>
        <nav>
          <button type="button" className={tab === 'library' ? 'on' : ''} onClick={() => setTab('library')}>
            Library
          </button>
          <button type="button" className={tab === 'queue' ? 'on' : ''} onClick={() => setTab('queue')}>
            Delete list {pending > 0 && <span className="count">{pending}</span>}
          </button>
          <button type="button" className={tab === 'settings' ? 'on' : ''} onClick={() => setTab('settings')}>
            Settings
          </button>
        </nav>
        <div className="topbar-right">
          {s && (
            <span className="muted stats">
              {s.library.movie.count.toLocaleString()} movies · {s.library.series.count.toLocaleString()} series ·{' '}
              {formatSize(s.library.movie.size + s.library.series.size)}
            </span>
          )}
          <button
            type="button"
            className="btn"
            onClick={() => sync.mutate()}
            disabled={sync.isPending || !configured}
            title={s ? `Last synced ${timeAgo(s.last_sync)}` : undefined}
          >
            <span className={sync.isPending ? 'spin' : ''}>
              <Icon name="refresh" />
            </span>
            {sync.isPending ? 'Syncing…' : 'Sync'}
          </button>
        </div>
      </header>

      <main>
        {status.error && <div className="banner error">{(status.error as Error).message}</div>}
        {s && !configured && tab !== 'settings' && (
          <div className="banner">
            Radarr and Sonarr aren’t connected yet.{' '}
            <button type="button" className="link" onClick={() => setTab('settings')}>
              Open settings
            </button>
          </div>
        )}
        {tab === 'library' && <Library configured={configured} />}
        {tab === 'queue' && <Queue />}
        {tab === 'settings' && <Settings />}
      </main>

      <div className="toasts" role="status" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast ${t.kind}`}>
            {t.text}
          </div>
        ))}
      </div>
    </>
  );
}

function KeyPrompt({ onSaved }: { onSaved: () => void }) {
  const [key, setKey] = useState('');
  return (
    <form
      className="key-prompt"
      onSubmit={(e) => {
        e.preventDefault();
        setApiKey(key.trim());
        onSaved();
      }}
    >
      <h1>Plex Delete List</h1>
      <p className="muted">This server requires an API key (the PDL_API_KEY it was started with).</p>
      <input type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder="API key" autoFocus />
      <button type="submit" className="btn primary" disabled={!key.trim()}>
        Continue
      </button>
    </form>
  );
}
