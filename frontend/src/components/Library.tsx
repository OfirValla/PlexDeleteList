import { useDeferredValue, useEffect, useMemo, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api, type Kind, type Media } from '../api';
import { findMark, formatSize, useMarkToggle } from '../util';
import { AiBadge, Icon, MarkButtons, Poster } from './common';
import MediaDetail from './MediaDetail';

type Sort = 'size' | 'added_new' | 'added_old' | 'title' | 'year' | 'rating';
const PAGE = 120;

const sorters: Record<Sort, (a: Media, b: Media) => number> = {
  size: (a, b) => b.size_on_disk - a.size_on_disk,
  added_new: (a, b) => (b.added_at ?? '').localeCompare(a.added_at ?? ''),
  added_old: (a, b) => (a.added_at ?? '9').localeCompare(b.added_at ?? '9'),
  title: (a, b) => a.sort_title.localeCompare(b.sort_title),
  year: (a, b) => (b.year ?? 0) - (a.year ?? 0),
  rating: (a, b) => (b.rating ?? 0) - (a.rating ?? 0),
};

export default function Library({ configured }: { configured: boolean }) {
  const { data, isLoading, error } = useQuery({ queryKey: ['media'], queryFn: api.media, refetchInterval: 60_000 });
  const [search, setSearch] = useState('');
  const [kind, setKind] = useState<Kind | 'all'>('all');
  const [sort, setSort] = useState<Sort>('size');
  const [markedOnly, setMarkedOnly] = useState(false);
  const [visible, setVisible] = useState(PAGE);
  const [openId, setOpenId] = useState<number | null>(null);
  const query = useDeferredValue(search.trim().toLowerCase());
  const sentinel = useRef<HTMLDivElement>(null);

  const items = useMemo(() => {
    const list = (data?.items ?? []).filter(
      (m) =>
        (kind === 'all' || m.kind === kind) &&
        (!markedOnly || m.marks.length > 0) &&
        (!query || m.title.toLowerCase().includes(query)),
    );
    return list.sort(sorters[sort]);
  }, [data, kind, markedOnly, query, sort]);

  useEffect(() => setVisible(PAGE), [kind, markedOnly, query, sort]);

  // Render the grid in chunks as the user scrolls; a few thousand posters at once is too heavy.
  useEffect(() => {
    const el = sentinel.current;
    if (!el) return;
    const observer = new IntersectionObserver(
      (entries) => entries[0].isIntersecting && setVisible((v) => v + PAGE),
      { rootMargin: '800px' },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [items.length, visible]);

  const totalSize = useMemo(() => items.reduce((sum, m) => sum + m.size_on_disk, 0), [items]);

  return (
    <>
      <div className="toolbar">
        <label className="search">
          <Icon name="search" />
          <input
            type="search"
            placeholder="Search titles"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            aria-label="Search titles"
          />
        </label>
        <div className="segmented" role="group" aria-label="Type">
          {(['all', 'movie', 'series'] as const).map((k) => (
            <button key={k} type="button" className={kind === k ? 'on' : ''} onClick={() => setKind(k)}>
              {k === 'all' ? 'All' : k === 'movie' ? 'Movies' : 'Series'}
            </button>
          ))}
        </div>
        <select value={sort} onChange={(e) => setSort(e.target.value as Sort)} aria-label="Sort by">
          <option value="size">Largest first</option>
          <option value="added_old">Oldest added</option>
          <option value="added_new">Recently added</option>
          <option value="title">Title</option>
          <option value="year">Newest release</option>
          <option value="rating">Highest rated</option>
        </select>
        <label className="check">
          <input type="checkbox" checked={markedOnly} onChange={(e) => setMarkedOnly(e.target.checked)} />
          Marked only
        </label>
        <span className="toolbar-summary">
          {items.length.toLocaleString()} items · {formatSize(totalSize)}
        </span>
      </div>

      {error ? (
        <div className="empty">Could not load the library: {(error as Error).message}</div>
      ) : isLoading ? (
        <div className="empty">Loading library…</div>
      ) : items.length === 0 ? (
        <div className="empty">
          {data?.items.length
            ? 'Nothing matches these filters.'
            : configured
              ? 'The library is empty. Press Sync to pull it from Radarr and Sonarr.'
              : 'Connect Radarr and/or Sonarr in Settings to load your library.'}
        </div>
      ) : (
        <div className="grid">
          {items.slice(0, visible).map((m) => (
            <Card key={m.id} media={m} onOpen={() => setOpenId(m.id)} />
          ))}
        </div>
      )}
      <div ref={sentinel} />

      {openId !== null && <MediaDetail id={openId} onClose={() => setOpenId(null)} />}
    </>
  );
}

function Card({ media, onOpen }: { media: Media; onOpen: () => void }) {
  const { toggle, busy } = useMarkToggle();
  const whole = findMark(media.marks, { scope: media.kind });
  const partial = whole ? 0 : media.marks.length;
  const fromAi = media.marks.some((m) => m.source === 'llm');

  return (
    <article className={`card ${whole ? `marked ${whole.action}` : partial ? 'marked partial' : ''}`}>
      <button type="button" className="card-poster" onClick={onOpen} aria-label={`Open ${media.title}`}>
        <Poster url={media.poster_url} title={media.title} />
        <span className="card-size">{formatSize(media.size_on_disk)}</span>
        {(whole || partial > 0) && (
          <span className="card-flag">
            {whole ? (whole.action === 'delete' ? 'Delete' : 'Redownload') : `${partial} part${partial > 1 ? 's' : ''} marked`}
            {fromAi && <AiBadge />}
          </span>
        )}
      </button>
      <div className="card-actions">
        <MarkButtons
          active={whole?.action ?? null}
          disabled={busy}
          onToggle={(action) => toggle(media, { scope: media.kind }, action)}
        />
      </div>
      <div className="card-meta">
        <div className="card-title" title={media.title}>
          {media.title}
        </div>
        <div className="card-sub">
          {media.year ?? '—'} ·{' '}
          {media.kind === 'movie' ? (media.quality ?? 'Movie') : `${media.season_count} season${media.season_count === 1 ? '' : 's'}`}
        </div>
      </div>
    </article>
  );
}
