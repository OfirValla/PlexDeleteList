export type Kind = 'movie' | 'series';
export type Scope = 'movie' | 'series' | 'season' | 'episode';
export type Action = 'delete' | 'redownload';

export interface MarkRef {
  id: number;
  scope: Scope;
  season_number: number | null;
  episode_id: number | null;
  action: Action;
  source: 'user' | 'llm';
  reason: string | null;
  label: string;
}

export interface Media {
  id: number;
  kind: Kind;
  title: string;
  sort_title: string;
  year: number | null;
  overview: string | null;
  poster_url: string | null;
  fanart_url: string | null;
  imdb_id: string | null;
  size_on_disk: number;
  file_count: number;
  episode_count: number;
  season_count: number;
  status: string | null;
  added_at: string | null;
  genres: string[];
  rating: number | null;
  path: string | null;
  quality: string | null;
  monitored: boolean;
  runtime: number | null;
  network: string | null;
  requested_by: string | null;
  marks: MarkRef[];
}

export interface Season {
  season_number: number;
  episode_file_count: number;
  episode_count: number;
  total_episode_count: number;
  size_on_disk: number;
  monitored: boolean;
}

export interface Episode {
  id: number;
  season_number: number;
  episode_number: number;
  title: string | null;
  air_date: string | null;
  has_file: boolean;
  size: number;
  quality: string | null;
  monitored: boolean;
}

export interface Mark {
  id: number;
  media_id: number | null;
  media_kind: Kind;
  media_title: string;
  poster_url: string | null;
  scope: Scope;
  season_number: number | null;
  episode_id: number | null;
  label: string;
  action: Action;
  reason: string | null;
  source: 'user' | 'llm';
  size_bytes: number;
  status: 'pending' | 'done' | 'failed';
  error: string | null;
  created_at: string;
  executed_at: string | null;
}

export type Service = 'radarr' | 'sonarr' | 'seerr';

export interface Status {
  services: Record<Service, boolean>;
  last_sync: string | null;
  library: Record<Kind, { count: number; size: number }>;
  pending: { count: number; reclaim_bytes: number };
  auth_required: boolean;
}

export type Settings = Record<Service, { url: string | null; has_key: boolean }>;
export type SettingsUpdate = Partial<Record<Service, { url: string; api_key: string }>>;

export interface MarkTarget {
  scope: Scope;
  season_number?: number;
  episode_id?: number;
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

const KEY_STORAGE = 'pdl_api_key';

export function getApiKey(): string {
  try {
    return localStorage.getItem(KEY_STORAGE) ?? '';
  } catch {
    return '';
  }
}

export function setApiKey(key: string) {
  try {
    localStorage.setItem(KEY_STORAGE, key);
  } catch {
    /* storage unavailable: the key just won't persist */
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  const key = getApiKey();
  if (key) headers['Authorization'] = `Bearer ${key}`;
  if (body !== undefined) headers['Content-Type'] = 'application/json';

  let res: Response;
  try {
    res = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  } catch {
    throw new ApiError(0, 'Cannot reach the server');
  }
  const text = await res.text();
  let data: unknown = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    /* non-JSON error body */
  }
  if (!res.ok) {
    const message = (data as { error?: string } | null)?.error ?? `Request failed (${res.status})`;
    throw new ApiError(res.status, message);
  }
  return data as T;
}

export const api = {
  status: () => request<Status>('GET', '/api/status'),
  settings: () => request<Settings>('GET', '/api/settings'),
  saveSettings: (update: SettingsUpdate) => request<Settings>('PUT', '/api/settings', update),
  testService: (service: Service) =>
    request<{ ok: boolean; version?: string; error?: string }>('POST', `/api/settings/test/${service}`),
  sync: () => request<{ errors: string[] }>('POST', '/api/sync'),
  media: () => request<{ total: number; total_size: number; items: Media[] }>('GET', '/api/media'),
  detail: (id: number) => request<{ media: Media; seasons: Season[] }>('GET', `/api/media/${id}`),
  episodes: (id: number, season: number) => request<Episode[]>('GET', `/api/media/${id}/episodes?season=${season}`),
  marks: () => request<{ pending: Mark[]; history: Mark[] }>('GET', '/api/marks'),
  createMark: (mark: MarkTarget & { media_id: number; action: Action }) => request<Mark>('POST', '/api/marks', mark),
  deleteMark: (id: number) => request<{ removed: number }>('DELETE', `/api/marks/${id}`),
  clearMarks: (which: 'pending' | 'history') => request<{ removed: number }>('DELETE', `/api/marks?which=${which}`),};
