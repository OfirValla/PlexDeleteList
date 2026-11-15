import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, type Mark } from '../api';
import { formatDate, formatSize, toast } from '../util';
import { AiBadge, Icon, Poster } from './common';

function targetText(m: Mark): string {
  if (m.scope === 'movie') return 'Movie';
  if (m.scope === 'series') return 'Whole series';
  return m.label;
}

/**
 * The delete list. This site only collects what should go; it never deletes anything itself.
 * Delete entries move to "Already removed" when a sync sees their files are gone.
 */
export default function Queue() {
  const qc = useQueryClient();
  const { data, isLoading, error } = useQuery({ queryKey: ['marks'], queryFn: api.marks, refetchInterval: 20_000 });
  const refresh = () => {
    for (const key of ['media', 'detail', 'marks', 'status']) qc.invalidateQueries({ queryKey: [key] });
  };

  const remove = useMutation({ mutationFn: api.deleteMark, onSuccess: refresh, onError: (e: Error) => toast(e.message) });
  const clear = useMutation({ mutationFn: api.clearMarks, onSuccess: refresh, onError: (e: Error) => toast(e.message) });

  if (error) return <div className="empty">Could not load the list: {(error as Error).message}</div>;
  if (isLoading || !data) return <div className="empty">Loading…</div>;

  const { pending, history } = data;
  const deletes = pending.filter((m) => m.action === 'delete');
  const redownloads = pending.length - deletes.length;
  const reclaim = deletes.reduce((sum, m) => sum + m.size_bytes, 0);

  return (
    <div className="queue">
      <div className="queue-head">
        <div>
          <h2>Delete list</h2>
          {pending.length === 0 ? (
            <p className="muted">Nothing is on the list. Pick items in the library, or let an AI assistant suggest some.</p>
          ) : (
            <>
              <p>
                {deletes.length} to delete ({formatSize(reclaim)}) · {redownloads} to redownload
              </p>
              <p className="muted">
                This site doesn’t delete anything. Remove these yourself in Radarr, Sonarr or Plex; a delete entry is
                ticked off at the next sync once its files are gone.
              </p>
            </>
          )}
        </div>
        {pending.length > 0 && (
          <div className="queue-actions">
            <button type="button" className="btn" onClick={() => clear.mutate('pending')} disabled={clear.isPending}>
              Clear list
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
            Already removed ({history.length})
            <button
              type="button"
              className="link"
              onClick={(e) => {
                e.preventDefault();
                clear.mutate('history');
              }}
            >
              Clear
            </button>
          </summary>
          <div className="mark-list">
            {history.map((m) => (
              <MarkRow key={m.id} mark={m} />
            ))}
          </div>
        </details>
      )}
    </div>
  );
}

function MarkRow({ mark, onRemove }: { mark: Mark; onRemove?: () => void }) {
  return (
    <div className={`mark-row ${mark.status}`}>
      <div className="mark-thumb">
        <Poster url={mark.poster_url} title="" size="thumb" />
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
            <span className="muted">Added {formatDate(mark.created_at)}</span>
          ) : mark.status === 'failed' ? (
            // Left over from before this site became list-only.
            <span className="error-text">Failed: {mark.error}</span>
          ) : (
            <span className="muted">No longer on disk · {formatDate(mark.executed_at)}</span>
          )}
        </div>
        {mark.reason && <div className="mark-reason">“{mark.reason}”</div>}
      </div>
      <span className="row-size">{formatSize(mark.size_bytes)}</span>
      {onRemove && (
        <button type="button" className="icon-btn" onClick={onRemove} aria-label="Remove from list" title="Remove from list">
          <Icon name="close" />
        </button>
      )}
    </div>
  );
}
