import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api, type Media, type Season } from '../api';
import { findMark, formatDate, formatSize, seasonLabel, sizedImage, useMarkToggle } from '../util';
import { AiBadge, Icon, MarkButtons, Modal, Poster } from './common';

export default function MediaDetail({ id, onClose }: { id: number; onClose: () => void }) {
  const { data, error } = useQuery({ queryKey: ['detail', id], queryFn: () => api.detail(id) });
  const { toggle, busy } = useMarkToggle();
  const media = data?.media;

  return (
    <Modal onClose={onClose} className="detail">
      <button type="button" className="icon-btn modal-close" onClick={onClose} aria-label="Close">
        <Icon name="close" size={18} />
      </button>
      {error ? (
        <div className="empty">{(error as Error).message}</div>
      ) : !media ? (
        <div className="empty">Loading…</div>
      ) : (
        <>
          <div
            className="detail-hero"
            style={media.fanart_url ? { backgroundImage: `url(${sizedImage(media.fanart_url, 'fanart')})` } : undefined}
          >
            <div className="detail-hero-inner">
              <div className="detail-poster">
                <Poster url={media.poster_url} title={media.title} />
              </div>
              <div className="detail-head">
                <h2>
                  {media.title} {media.year && <span className="muted">({media.year})</span>}
                </h2>
                <div className="chips">
                  <span className="chip strong">{formatSize(media.size_on_disk)}</span>
                  <span className="chip">{media.kind === 'movie' ? 'Movie' : 'Series'}</span>
                  {media.rating && <span className="chip">★ {media.rating.toFixed(1)}</span>}
                  {media.quality && <span className="chip">{media.quality}</span>}
                  {media.kind === 'series' && <span className="chip">{media.file_count} episodes on disk</span>}
                  {media.status && <span className="chip">{media.status}</span>}
                  {media.network && <span className="chip">{media.network}</span>}
                </div>
                <MarkButtons
                  labels
                  active={findMark(media.marks, { scope: media.kind })?.action ?? null}
                  disabled={busy}
                  onToggle={(action) => toggle(media, { scope: media.kind }, action)}
                />
              </div>
            </div>
          </div>

          <div className="detail-body">
            <MarkNotes media={media} />
            {media.overview && <p className="overview">{media.overview}</p>}
            <dl className="facts">
              <div>
                <dt>Added</dt>
                <dd>{formatDate(media.added_at)}</dd>
              </div>
              {media.requested_by && (
                <div>
                  <dt>Requested by</dt>
                  <dd>{media.requested_by}</dd>
                </div>
              )}
              {media.genres.length > 0 && (
                <div>
                  <dt>Genres</dt>
                  <dd>{media.genres.join(', ')}</dd>
                </div>
              )}
              {media.path && (
                <div>
                  <dt>Path</dt>
                  <dd className="mono">{media.path}</dd>
                </div>
              )}
            </dl>

            {media.kind === 'series' && (
              <section>
                <h3>Seasons</h3>
                <div className="seasons">
                  {data.seasons.map((s) => (
                    <SeasonRow key={s.season_number} media={media} season={s} />
                  ))}
                </div>
              </section>
            )}
          </div>
        </>
      )}
    </Modal>
  );
}

/** Reasons attached to marks on this item (mostly from the AI assistant). */
function MarkNotes({ media }: { media: Media }) {
  const notes = media.marks.filter((m) => m.reason);
  if (notes.length === 0) return null;
  return (
    <ul className="notes">
      {notes.map((m) => (
        <li key={m.id}>
          {m.source === 'llm' && <AiBadge />}
          <span>
            <strong>
              {m.action === 'delete' ? 'Delete' : 'Redownload'}
              {m.scope === 'season' || m.scope === 'episode' ? ` ${m.label}` : ''}:
            </strong>{' '}
            {m.reason}
          </span>
        </li>
      ))}
    </ul>
  );
}

function SeasonRow({ media, season }: { media: Media; season: Season }) {
  const [open, setOpen] = useState(false);
  const { toggle, busy } = useMarkToggle();
  const n = season.season_number;
  const seriesMark = findMark(media.marks, { scope: 'series' });
  const seasonMark = findMark(media.marks, { scope: 'season', season_number: n });
  const episodeMarks = media.marks.filter((m) => m.scope === 'episode' && m.season_number === n).length;
  const hasFiles = season.episode_file_count > 0;

  return (
    <div className={`season ${open ? 'open' : ''}`}>
      <div className="season-row">
        <button type="button" className="season-toggle" onClick={() => setOpen(!open)} aria-expanded={open}>
          <span className="chevron">
            <Icon name="chevron" />
          </span>
          <span className="season-name">{seasonLabel(n)}</span>
          <span className="muted">
            {season.episode_file_count}/{season.total_episode_count} episodes
            {episodeMarks > 0 && ` · ${episodeMarks} marked`}
          </span>
        </button>
        <span className="row-size">{formatSize(season.size_on_disk)}</span>
        <MarkButtons
          active={seasonMark?.action ?? null}
          inherited={seriesMark?.action}
          disabled={busy || (!hasFiles && !seasonMark)}
          onToggle={(action) => toggle(media, { scope: 'season', season_number: n }, action)}
        />
      </div>
      {open && <Episodes media={media} season={n} inherited={seriesMark?.action ?? seasonMark?.action ?? null} />}
    </div>
  );
}

function Episodes({ media, season, inherited }: { media: Media; season: number; inherited: 'delete' | 'redownload' | null }) {
  const { data, error, isLoading } = useQuery({
    queryKey: ['episodes', media.id, season],
    queryFn: () => api.episodes(media.id, season),
  });
  const { toggle, busy } = useMarkToggle();

  if (isLoading) return <div className="episodes muted pad">Loading episodes…</div>;
  if (error) return <div className="episodes muted pad">{(error as Error).message}</div>;
  return (
    <div className="episodes">
      {data?.map((e) => {
        const mark = findMark(media.marks, { scope: 'episode', episode_id: e.id });
        return (
          <div key={e.id} className={`episode ${e.has_file ? '' : 'missing'}`}>
            <span className="ep-num">E{String(e.episode_number).padStart(2, '0')}</span>
            <span className="ep-title">
              {e.title ?? 'Episode'}
              <span className="muted"> · {e.has_file ? (e.quality ?? 'on disk') : 'no file'}</span>
            </span>
            <span className="row-size">{e.has_file ? formatSize(e.size) : '—'}</span>
            <MarkButtons
              active={mark?.action ?? null}
              inherited={inherited}
              disabled={busy || (!e.has_file && !mark)}
              onToggle={(action) => toggle(media, { scope: 'episode', season_number: season, episode_id: e.id }, action)}
            />
          </div>
        );
      })}
    </div>
  );
}
