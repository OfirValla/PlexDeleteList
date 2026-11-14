import { useSyncExternalStore } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api, type Action, type MarkRef, type MarkTarget, type Media } from './api';

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

/** Toggle a delete/redownload mark on a movie, series, season or episode. */
export function useMarkToggle() {
  const qc = useQueryClient();
  const refresh = () => {
    for (const key of ['media', 'detail', 'marks', 'status']) {
      qc.invalidateQueries({ queryKey: [key] });
    }
  };
  const mutation = useMutation({
    mutationFn: async (v: { media: Media; target: MarkTarget; action: Action }) => {
      const existing = findMark(v.media.marks, v.target);
      if (existing && existing.action === v.action) {
        await api.deleteMark(existing.id);
      } else {
        await api.createMark({ media_id: v.media.id, ...v.target, action: v.action });
      }
    },
    onSuccess: refresh,
    onError: (e: Error) => {
      toast(e.message);
      refresh();
    },
  });
  return {
    toggle: (media: Media, target: MarkTarget, action: Action) => mutation.mutate({ media, target, action }),
    busy: mutation.isPending,
  };
}
