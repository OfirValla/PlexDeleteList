import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, type Mark } from '../api';
import { formatDate, formatSize, toast } from '../util';
import { AiBadge, Icon, Modal, Poster } from './common';

function targetText(m: Mark): string {
  if (m.scope === 'movie') return 'Movie';
  if (m.scope === 'series') return 'Whole series';
  return m.label;
}

/** What applying this mark will actually do, in plain words. */
function consequence(m: Mark): string {
  if (m.action === 'redownload') return 'Deletes the current files and searches for a new copy';
  switch (m.scope) {
    case 'movie':
      return 'Deletes the files and removes the movie from Radarr';
    case 'series':
      return 'Deletes all files and removes the series from Sonarr';
    case 'season':
      return 'Deletes the season’s files and stops monitoring it';
    default:
      return 'Deletes the file and stops monitoring the episode';
  }
}

export default function Queue() {
  const qc = useQueryClient();
  const { data, isLoading, error } = useQuery({ queryKey: ['marks'], queryFn: api.marks, refetchInterval: 20_000 });
  const [confirming, setConfirming] = useState(false);
  const refresh = () => {
    for (const key of ['media', 'detail', 'marks', 'status']) qc.invalidateQueries({ queryKey: [key] });
  };

  const remove = useMutation({ mutationFn: api.deleteMark, onSuccess: refresh, onError: (e: Error) => toast(e.message) });
  const clear = useMutation({ mutationFn: api.clearMarks, onSuccess: refresh, onError: (e: Error) => toast(e.message) });
  const apply = useMutation({
    mutationFn: () => api.apply(),
    onSuccess: (res) => {
      setConfirming(false);
      refresh();
      if (res.failed > 0) toast(`${res.applied} applied, ${res.failed} failed — see history below`);
      else toast(`${res.applied} action${res.applied === 1 ? '' : 's'} applied`, 'ok');
    },
    onError: (e: Error) => {
      setConfirming(false);
      refresh();
      toast(e.message);
    },
  });

  if (error) return <div className="empty">Could not load the queue: {(error as Error).message}</div>;
  if (isLoading || !data) return <div className="empty">Loading…</div>;

  const { pending, history } = data;
  const deletes = pending.filter((m) => m.action === 'delete');
  const redownloads = pending.length - deletes.length;
  const reclaim = deletes.reduce((sum, m) => sum + m.size_bytes, 0);

  return (
    <div className="queue">
      <div className="queue-head">
        <div>
          <h2>Review queue</h2>
          <p className="muted">
            {pending.length === 0
              ? 'Nothing is marked. Pick items in the library, or let an AI assistant suggest some.'
              : `${deletes.length} to delete (frees ${formatSize(reclaim)}) · ${redownloads} to redownload. Nothing changes until you apply.`}
          </p>
        </div>
        {pending.length > 0 && (
          <div className="queue-actions">
            <button type="button" className="btn" onClick={() => clear.mutate('pending')} disabled={clear.isPending}>
              Clear all
            </button>
            <button type="button" className="btn danger" onClick={() => setConfirming(true)}>
              Apply {pending.length} action{pending.length === 1 ? '' : 's'}
            </button>
          </div>
        )}
      </div>

      <div className="mark-list">
        {pending.map((m) => (
          <MarkRow key={m.id} mark={m} onRemove={() => remove.mutate(m.id)} />
        ))}
      </div>

      {history.length > 0 && (
        <details className="history">
          <summary>
            History ({history.length})
            <button
              type="button"
              className="link"
              onClick={(e) => {
                e.preventDefault();
                clear.mutate('history');
              }}
            >
              Clear history
            </button>
          </summary>
          <div className="mark-list">
            {history.map((m) => (
              <MarkRow key={m.id} mark={m} />
            ))}
          </div>
        </details>
      )}

      {confirming && (
        <Modal onClose={() => !apply.isPending && setConfirming(false)} className="confirm">
          <h2>Apply {pending.length} action{pending.length === 1 ? '' : 's'}?</h2>
          <p>
            {deletes.length > 0 && (
              <>
                <strong>{deletes.length}</strong> item{deletes.length === 1 ? '' : 's'} will be deleted from disk, freeing
                about <strong>{formatSize(reclaim)}</strong>.{' '}
              </>
            )}
            {redownloads > 0 && (
              <>
                <strong>{redownloads}</strong> item{redownloads === 1 ? '' : 's'} will have their files deleted and a new
                search started.{' '}
              </>
            )}
            Deleted files can’t be recovered.
          </p>
          <ul className="confirm-list">
            {pending.slice(0, 12).map((m) => (
              <li key={m.id}>
                <span className={`pill ${m.action}`}>{m.action}</span>
                {m.media_title}
                {m.scope !== 'movie' && m.scope !== 'series' && <span className="muted"> · {m.label}</span>}
              </li>
            ))}
            {pending.length > 12 && <li className="muted">…and {pending.length - 12} more</li>}
          </ul>
          <div className="confirm-actions">
            <button type="button" className="btn" onClick={() => setConfirming(false)} disabled={apply.isPending}>
              Cancel
            </button>
            <button type="button" className="btn danger" onClick={() => apply.mutate()} disabled={apply.isPending}>
              {apply.isPending ? 'Applying…' : 'Yes, apply'}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function MarkRow({ mark, onRemove }: { mark: Mark; onRemove?: () => void }) {
  return (
    <div className={`mark-row ${mark.status}`}>
      <div className="mark-thumb">
        <Poster url={mark.poster_url} title="" />
      </div>
      <div className="mark-main">
        <div className="mark-title">
          {mark.media_title}
          <span className="muted"> · {targetText(mark)}</span>
        </div>
        <div className="mark-sub">
          <span className={`pill ${mark.action}`}>{mark.action}</span>
          {mark.source === 'llm' && <AiBadge />}
          {mark.status === 'pending' ? (
            <span className="muted">{consequence(mark)}</span>
          ) : (
            <span className={mark.status === 'failed' ? 'error-text' : 'muted'}>
              {mark.status === 'failed' ? `Failed: ${mark.error}` : `Applied ${formatDate(mark.executed_at)}`}
            </span>
          )}
        </div>
        {mark.reason && <div className="mark-reason">“{mark.reason}”</div>}
      </div>
      <span className="row-size">{formatSize(mark.size_bytes)}</span>
      {onRemove && (
        <button type="button" className="icon-btn" onClick={onRemove} aria-label="Remove from queue" title="Remove from queue">
          <Icon name="close" />
        </button>
      )}
    </div>
  );
}
