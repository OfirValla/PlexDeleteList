import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, type Service, type SettingsUpdate } from '../api';
import { toast } from '../util';

const services: { id: Service; name: string; hint: string; placeholder: string }[] = [
  { id: 'radarr', name: 'Radarr', hint: 'Movies. Settings → General → API Key.', placeholder: 'http://localhost:7878' },
  { id: 'sonarr', name: 'Sonarr', hint: 'TV series. Settings → General → API Key.', placeholder: 'http://localhost:8989' },
  {
    id: 'seerr',
    name: 'Seerr (optional)',
    hint: 'Overseerr / Jellyseerr / Seerr. Shows who requested each item and clears it there when deleted.',
    placeholder: 'http://localhost:5055',
  },
];

type Form = Record<Service, { url: string; api_key: string }>;
type TestResult = { ok: boolean; version?: string; error?: string };

export default function Settings() {
  const qc = useQueryClient();
  const { data } = useQuery({ queryKey: ['settings'], queryFn: api.settings });
  const [form, setForm] = useState<Form | null>(null);
  const [tests, setTests] = useState<Partial<Record<Service, TestResult>>>({});

  useEffect(() => {
    if (data && !form) {
      setForm({
        radarr: { url: data.radarr.url ?? '', api_key: '' },
        sonarr: { url: data.sonarr.url ?? '', api_key: '' },
        seerr: { url: data.seerr.url ?? '', api_key: '' },
      });
    }
  }, [data, form]);

  const save = useMutation({
    mutationFn: async (update: SettingsUpdate) => {
      const saved = await api.saveSettings(update);
      // Check every service that now has both a URL and a key, so a typo shows up right away.
      const results: Partial<Record<Service, TestResult>> = {};
      await Promise.all(
        services
          .filter((s) => saved[s.id].url && saved[s.id].has_key)
          .map(async (s) => {
            results[s.id] = await api.testService(s.id);
          }),
      );
      return { saved, results };
    },
    onSuccess: ({ saved, results }) => {
      qc.setQueryData(['settings'], saved);
      setTests(results);
      setForm((f) => f && { radarr: { ...f.radarr, api_key: '' }, sonarr: { ...f.sonarr, api_key: '' }, seerr: { ...f.seerr, api_key: '' } });
      qc.invalidateQueries({ queryKey: ['status'] });
      const bad = Object.values(results).some((r) => !r.ok);
      if (bad) toast('Saved, but a connection test failed');
      else {
        toast('Settings saved', 'ok');
        api.sync().then(
          () => qc.invalidateQueries(),
          (e: Error) => toast(e.message),
        );
      }
    },
    onError: (e: Error) => toast(e.message),
  });

  if (!form || !data) return <div className="empty">Loading…</div>;
  const mcpUrl = `${location.origin}/mcp`;

  return (
    <div className="settings">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          save.mutate(form);
        }}
      >
        <h2>Connections</h2>
        {services.map((s) => {
          const test = tests[s.id];
          return (
            <fieldset key={s.id} className="service">
              <legend>{s.name}</legend>
              <p className="muted">{s.hint}</p>
              <div className="fields">
                <label>
                  URL
                  <input
                    type="url"
                    placeholder={s.placeholder}
                    value={form[s.id].url}
                    onChange={(e) => setForm({ ...form, [s.id]: { ...form[s.id], url: e.target.value } })}
                  />
                </label>
                <label>
                  API key
                  <input
                    type="password"
                    autoComplete="off"
                    placeholder={data[s.id].has_key ? 'Saved — leave blank to keep' : 'Paste API key'}
                    value={form[s.id].api_key}
                    onChange={(e) => setForm({ ...form, [s.id]: { ...form[s.id], api_key: e.target.value } })}
                  />
                </label>
              </div>
              {test && (
                <p className={test.ok ? 'ok-text' : 'error-text'}>
                  {test.ok ? `Connected${test.version ? ` (v${test.version})` : ''}` : test.error}
                </p>
              )}
            </fieldset>
          );
        })}
        <button type="submit" className="btn primary" disabled={save.isPending}>
          {save.isPending ? 'Saving…' : 'Save and test'}
        </button>
      </form>

      <section className="mcp">
        <h2>AI assistant (MCP)</h2>
        <p className="muted">
          Connect Claude or another MCP client so it can browse the library and suggest what to remove. Suggestions land in
          the review queue tagged “AI”; only you can apply them.
        </p>
        <h3>Claude Code</h3>
        <pre>claude mcp add --transport http plex-delete-list {mcpUrl}</pre>
        <h3>Claude Desktop and other stdio clients</h3>
        <pre>
          {JSON.stringify(
            {
              mcpServers: {
                'plex-delete-list': {
                  command: 'plex-delete-list',
                  args: ['mcp'],
                  env: { PDL_URL: location.origin },
                },
              },
            },
            null,
            2,
          )}
        </pre>
        <p className="muted">
          If the server was started with <code>PDL_API_KEY</code>, also pass it: a{' '}
          <code>Authorization: Bearer …</code> header for HTTP, or <code>PDL_API_KEY</code> in <code>env</code> for stdio.
        </p>
      </section>
    </div>
  );
}
