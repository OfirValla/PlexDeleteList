import { useSyncExternalStore } from 'react';
import { useMutation, useQueryClient, type QueryClient } from '@tanstack/react-query';
import { api, type Action, type MarkRef, type MarkTarget, type Media, type Season } from './api';

/**
 * Radarr and Sonarr hand out full-resolution artwork (around 1 MB per movie poster). Ask the image CDN
 * for a size that fits where it's shown instead.
 */
export function sizedImage(url: string | null, use: 'poster' | 'thumb' | 'fanart'): string | null {
  if (!url) return null;
  if (url.includes('image.tmdb.org/t/p/original/')) {
    const size = use === 'fanart' ? 'w1280' : use === 'thumb' ? 'w154' : 'w342';
    return url.replace('/t/p/original/', `/t/p/${size}/`);
  }
  // TVDB keeps a 340px-wide "_t" thumbnail next to each image. That's too small for a backdrop.
  if (url.includes('artworks.thetvdb.com/') && use !== 'fanart' && !/_t\.\w+$/.test(url)) {
    return url.replace(/(\.\w+)$/, '_t$1');
  }
  return url;
}

export function formatSize(bytes: number): string {
  if (!bytes) return '0 GB';
  const gb = bytes / 1024 ** 3;
  if (gb >= 1024) return `${(gb / 1024).toFixed(2)} TB`;
  if (gb >= 100) return `${gb.toFixed(0)} GB`;
  if (gb >= 1) return `${gb.toFixed(1)} GB`;
  return `${Math.max(1, Math.round(bytes / 1024 ** 2))} MB`;
}

export function formatDate(iso: string | null): string {
  if (!iso) return '—';
  const d = new Date(iso);
  return isNaN(d.getTime()) ? '—' : d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

export function timeAgo(iso: string | null): string {
  if (!iso) return 'never';
  const minutes = Math.round((Date.now() - new Date(iso).getTime()) / 60000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes} min ago`;
  if (minutes < 60 * 24) return `${Math.round(minutes / 60)} h ago`;
  return `${Math.round(minutes / 1440)} d ago`;
}

export function seasonLabel(n: number): string {
  return n === 0 ? 'Specials' : `Season ${n}`;
}

// ---------- toasts ----------

export interface Toast {
  id: number;
  text: string;
  kind: 'error' | 'ok';
}

let toasts: Toast[] = [];
let nextToast = 1;
const listeners = new Set<() => void>();

export function toast(text: string, kind: Toast['kind'] = 'error') {
  const id = nextToast++;
  toasts = [...toasts, { id, text, kind }];
  listeners.forEach((l) => l());
  setTimeout(() => {
    toasts = toasts.filter((t) => t.id !== id);
    listeners.forEach((l) => l());
  }, 6000);
}

export function useToasts(): Toast[] {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => toasts,
  );
}

// ---------- marks ----------

export function findMark(marks: MarkRef[], target: MarkTarget): MarkRef | undefined {
  return marks.find((m) => {
    if (m.scope !== target.scope) return false;
    if (target.scope === 'season') return m.season_number === target.season_number;
    if (target.scope === 'episode') return m.episode_id === target.episode_id;
    return true;
  });
}

/** Same rule as the server: a new mark replaces any mark on the same target or on something inside it. */
function withMark(marks: MarkRef[], mark: MarkRef): MarkRef[] {
  const kept = marks.filter((m) => {
    if (mark.scope === 'season') return m.season_number !== mark.season_number;
    if (mark.scope === 'episode') return m.episode_id !== mark.episode_id;
    return false;
  });
  return [...kept, mark];
}

/** Change one item's marks in the cached library list and in its cached detail. */
function patchMarks(qc: QueryClient, mediaId: number, change: (marks: MarkRef[]) => MarkRef[]) {
  qc.setQueryData<{ total: number; total_size: number; items: Media[] }>(
    ['media'],
    (old) => old && { ...old, items: old.items.map((m) => (m.id === mediaId ? { ...m, marks: change(m.marks) } : m)) },
  );
  qc.setQueryData<{ media: Media; seasons: Season[] }>(
    ['detail', mediaId],
    (old) => old && { ...old, media: { ...old.media, marks: change(old.media.marks) } },
  );
}

let nextTempId = -1;

/**
 * Toggle a delete/redownload mark on a movie, series, season or episode.
 *
 * The change is shown immediately and the server call confirms it in the background, so a click never
 * waits on the network or on reloading the whole library. If the server refuses, the change is undone.
 */
export function useMarkToggle() {
  const qc = useQueryClient();
  const mutation = useMutation({
    mutationFn: (v: { media: Media; target: MarkTarget; action: Action; existing?: MarkRef; tempId: number }) =>
      v.existing?.action === v.action
        ? api.deleteMark(v.existing.id).then(() => null)
        : api.createMark({ media_id: v.media.id, ...v.target, action: v.action }),
    onMutate: async (v) => {
      // An in-flight refetch would otherwise overwrite the change with the old state.
      await Promise.all([
        qc.cancelQueries({ queryKey: ['media'] }),
        qc.cancelQueries({ queryKey: ['detail', v.media.id] }),
      ]);
      if (v.existing?.action === v.action) {
        const id = v.existing.id;
        patchMarks(qc, v.media.id, (marks) => marks.filter((m) => m.id !== id));
      } else {
        const placeholder: MarkRef = {
          id: v.tempId,
          scope: v.target.scope,
          season_number: v.target.season_number ?? null,
          episode_id: v.target.episode_id ?? null,
          action: v.action,
          source: 'user',
          reason: null,
          label: '',
        };
        patchMarks(qc, v.media.id, (marks) => withMark(marks, placeholder));
      }
    },
    onSuccess: (saved, v) => {
      if (!saved) return;
      const { id, scope, season_number, episode_id, action, source, reason, label } = saved;
      const real: MarkRef = { id, scope, season_number, episode_id, action, source, reason, label };
      patchMarks(qc, v.media.id, (marks) => marks.map((m) => (m.id === v.tempId ? real : m)));
    },
    onError: (e: Error, v) => {
      toast(e.message);
      // Put this item's marks back as they were, then ask the server for the truth.
      patchMarks(qc, v.media.id, () => v.media.marks);
      qc.invalidateQueries({ queryKey: ['media'] });
      qc.invalidateQueries({ queryKey: ['detail', v.media.id] });
    },
    onSettled: () => {
      qc.invalidateQueries({ queryKey: ['marks'] });
      qc.invalidateQueries({ queryKey: ['status'] });
    },
  });
  return {
    toggle: (media: Media, target: MarkTarget, action: Action) => {
      const existing = findMark(media.marks, target);
      // A negative id is a mark that is still being saved; wait for its real id.
      if (existing && existing.id < 0) return;
      mutation.mutate({ media, target, action, existing, tempId: nextTempId-- });
    },
    busy: mutation.isPending,
  };
}
