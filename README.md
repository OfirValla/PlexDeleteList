# Plex Delete List

Browse everything in your Plex library and collect a list of what should be deleted or redownloaded.
Includes an MCP server so Claude (or another LLM) can look through the library and suggest what to remove.

**The site never deletes anything.** It only reads from Radarr, Sonarr and Seerr (the backend has no code that
sends them anything but GET requests) and keeps the list in its own SQLite database. You remove the listed
items yourself in Radarr, Sonarr or Plex.

- **Library** – poster grid of movies and series with size on disk; search, filter, sort (largest first by default).
- **Granular marks** – a movie, a whole series, a season, or single episodes; each as *delete* or *redownload*.
- **Delete list** – everything you marked, with the space it would free. AI suggestions arrive here tagged "AI" with a reason.
- **Sources** – Radarr (movies) and Sonarr (series) provide the library. Seerr (Overseerr/Jellyseerr) is optional and adds "requested by".

## How the list clears

At each sync (hourly, or the Sync button) a *delete* entry moves to "Already removed" once its files are gone:
the movie or series left Radarr/Sonarr or has no files, the season has no files, or the episode has no file.
A service that is unreachable is not taken as proof of anything. *Redownload* entries stay until you remove
them, since the site can't tell an old copy from a new one.

## Run with Docker

```sh
docker compose up -d --build
```

Open http://localhost:8080, go to **Settings**, and enter the Radarr/Sonarr (and optionally Seerr) URL and API key.
Data lives in `./data/pdl.db`.

## Run from source

Requires Rust (stable) and Node 20+.

```sh
cd frontend && npm install && npm run build && cd ..
cd backend && cargo run --release        # serves the UI and API on :8080
```

For frontend development, run `cargo run` in `backend/` and `npm run dev` in `frontend/` (Vite proxies `/api` to :8080).

### Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `PDL_BIND` | `0.0.0.0:8080` | Listen address |
| `PDL_DB` | `./data/pdl.db` | SQLite database |
| `PDL_STATIC_DIR` | `./frontend/dist` | Built frontend |
| `PDL_API_KEY` | – | If set, `/api` and `/mcp` require it (`Authorization: Bearer …` or `X-Api-Key`) |
| `PDL_SYNC_INTERVAL_MINUTES` | `60` | Background sync; `0` disables |
| `RADARR_URL`, `RADARR_API_KEY`, `SONARR_URL`, `SONARR_API_KEY`, `SEERR_URL`, `SEERR_API_KEY` | – | Defaults; values saved on the Settings page win |

There is no login by default. The app can't delete media, but anyone who can reach the port can edit the list and
the connection settings, so set `PDL_API_KEY` if that is more than you.

## MCP server

The server speaks MCP at `/mcp` (Streamable HTTP). The same binary can also act as a stdio MCP server that bridges
to it, for clients that only launch local commands.

Claude Code:

```sh
claude mcp add --transport http plex-delete-list http://localhost:8080/mcp
```

Claude Desktop (`claude_desktop_config.json`) or any stdio client:

```json
{
  "mcpServers": {
    "plex-delete-list": {
      "command": "C:\\path\\to\\plex-delete-list.exe",
      "args": ["mcp"],
      "env": { "PDL_URL": "http://localhost:8080" }
    }
  }
}
```

With `PDL_API_KEY` set, add `--header "Authorization: Bearer <key>"` (HTTP) or `PDL_API_KEY` in `env` (stdio).

Tools: `library_stats`, `list_media`, `get_media`, `mark_items`, `list_marks`, `unmark_items`, `sync_library`.
Like the site, they only build the list; none of them can delete or change media.

Example prompt: *"Look at my library and suggest about 200 GB to delete: large, low-rated movies added more than two
years ago, and ended shows. Give a reason for each."*

## Layout

```
backend/src
  main.rs    CLI, router, auth        db.rs     schema and queries
  sync.rs    Radarr/Sonarr/Seerr sync  marks.rs  marking rules
  arr.rs     read-only HTTP client    api.rs    REST handlers
  mcp.rs     MCP tools, HTTP + stdio transports
frontend/src
  App.tsx, components/{Library,MediaDetail,Queue,Settings}.tsx, api.ts, util.ts
```
