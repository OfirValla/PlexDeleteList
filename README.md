# Plex Delete List

Browse everything in your Plex library, pick what to delete or redownload, and apply it through Radarr and Sonarr.
Includes an MCP server so Claude (or another LLM) can look through the library and suggest what to remove.

- **Library** – poster grid of movies and series with size on disk; search, filter, sort (largest first by default).
- **Granular marks** – a movie, a whole series, a season, or single episodes; each as *delete* or *redownload*.
- **Review queue** – marks do nothing until you press **Apply** and confirm. AI suggestions arrive here tagged "AI" with a reason.
- **Sources** – Radarr (movies) and Sonarr (series) provide the library and do the work. Seerr (Overseerr/Jellyseerr) is optional: it adds "requested by" and gets cleared when a whole item is deleted.

## What applying a mark does

| Target | Delete | Redownload |
| --- | --- | --- |
| Movie | Removes the movie and its files from Radarr | Deletes the file, monitors the movie, starts a search |
| Series | Removes the series and its files from Sonarr | Deletes all episode files, starts a series search |
| Season | Unmonitors the season, deletes its files | Deletes its files, monitors it, starts a season search |
| Episode | Unmonitors the episode, deletes its file | Deletes its file, monitors it, starts an episode search |

Plex notices the removals on its next library scan (or immediately if Radarr/Sonarr have a Plex "Connect" notification).
Redownload does not blocklist the old release, so the same release can be grabbed again if it still scores best.

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

There is no login by default. This app can delete media, so set `PDL_API_KEY` if anyone else can reach the port.

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
There is deliberately no tool that applies marks: the LLM proposes, you confirm in the UI.

Example prompt: *"Look at my library and suggest about 200 GB to delete: large, low-rated movies added more than two
years ago, and ended shows. Give a reason for each."*

## Layout

```
backend/src
  main.rs    CLI, router, auth        db.rs     schema and queries
  sync.rs    Radarr/Sonarr/Seerr sync  marks.rs  marking rules and apply
  arr.rs     HTTP client              api.rs    REST handlers
  mcp.rs     MCP tools, HTTP + stdio transports
frontend/src
  App.tsx, components/{Library,MediaDetail,Queue,Settings}.tsx, api.ts, util.ts
```
