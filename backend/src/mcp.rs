//! Model Context Protocol server.
//!
//! The tool logic lives behind `POST /mcp` (Streamable HTTP, plain JSON responses). `plex-delete-list mcp`
//! is a stdio transport that forwards each JSON-RPC message to that endpoint, so both transports share one
//! implementation.
//!
//! By design there is no tool that applies marks: an LLM can propose deletions, a human confirms them in the UI.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    db::{self, MediaFilter, GIB},
    error::{AppError, Result},
    marks::{self, NewMark},
    sync, AppState, Shared,
};

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Plex Delete List helps the user free disk space in their Plex library. \
You can browse the library (movies and TV series, synced from Radarr/Sonarr) and mark items for deletion \
or redownload. Marks go into a review queue; nothing is deleted until the user confirms in the web UI, \
and you cannot apply marks yourself. Typical flow: library_stats, then list_media (sorted by size, \
filtered by age/genre/requester), get_media for season or episode detail, then mark_items with a short \
reason for each item so the user understands why it was suggested. Sizes are in GiB.";

fn tools() -> Value {
    json!([
        {
            "name": "library_stats",
            "description": "Overview of the library: item counts and disk usage for movies and series, pending marks, configured services and last sync time.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_media",
            "description": "List movies and/or series that have files on disk, with size, age, rating, genres, who requested them and any pending marks. Paginated; `total` is the full match count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["movie", "series"], "description": "Omit for both" },
                    "q": { "type": "string", "description": "Title contains" },
                    "genre": { "type": "string", "description": "Exact genre name, e.g. 'Comedy'" },
                    "requested_by": { "type": "string", "description": "Requester name contains (needs Seerr)" },
                    "min_size_gb": { "type": "number" },
                    "max_size_gb": { "type": "number" },
                    "added_before": { "type": "string", "description": "ISO date, e.g. 2024-01-01" },
                    "added_after": { "type": "string", "description": "ISO date" },
                    "marked": { "type": "boolean", "description": "true: only items with pending marks; false: only unmarked" },
                    "sort": { "type": "string", "enum": ["size", "added", "title", "year", "rating"], "description": "Default: size" },
                    "order": { "type": "string", "enum": ["asc", "desc"] },
                    "limit": { "type": "integer", "description": "Default 50, max 500" },
                    "offset": { "type": "integer" }
                }
            }
        },
        {
            "name": "get_media",
            "description": "Full detail for one item: overview, path, quality and, for series, per-season sizes. Pass season_number to also list that season's episodes (with the episode ids needed to mark single episodes).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "media_id": { "type": "integer" },
                    "season_number": { "type": "integer", "description": "Series only: include this season's episodes" }
                },
                "required": ["media_id"]
            }
        },
        {
            "name": "mark_items",
            "description": "Add items to the user's review queue. action 'delete' removes the files (and the movie/series from Radarr/Sonarr when the whole item is marked); 'redownload' deletes the current files and searches for a new copy. Nothing happens until the user approves in the web UI. Marking a whole item replaces narrower marks on it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "media_id": { "type": "integer" },
                                "scope": { "type": "string", "enum": ["movie", "series", "season", "episode"], "description": "Default: the whole movie/series" },
                                "season_number": { "type": "integer", "description": "Required for scope 'season'" },
                                "episode_id": { "type": "integer", "description": "Required for scope 'episode' (from get_media)" },
                                "action": { "type": "string", "enum": ["delete", "redownload"] },
                                "reason": { "type": "string", "description": "Shown to the user next to the mark" }
                            },
                            "required": ["media_id", "action"]
                        }
                    }
                },
                "required": ["items"]
            }
        },
        {
            "name": "list_marks",
            "description": "The review queue: pending marks (from the user and from you) and recently applied or failed ones.",
            "inputSchema": {
                "type": "object",
                "properties": { "include_history": { "type": "boolean", "description": "Also return applied/failed marks" } }
            }
        },
        {
            "name": "unmark_items",
            "description": "Remove pending marks from the review queue by mark id.",
            "inputSchema": {
                "type": "object",
                "properties": { "mark_ids": { "type": "array", "items": { "type": "integer" } } },
                "required": ["mark_ids"]
            }
        },
        {
            "name": "sync_library",
            "description": "Refresh the library from Radarr/Sonarr/Seerr now. It also syncs automatically in the background.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn gb(bytes: &Value) -> f64 {
    (bytes.as_i64().unwrap_or(0) as f64 / GIB * 100.0).round() / 100.0
}

fn date(v: &Value) -> Value {
    v.as_str().map(|s| json!(s.chars().take(10).collect::<String>())).unwrap_or(Value::Null)
}

/// A trimmed row for listings, to keep tool results small.
fn compact(m: &Value) -> Value {
    let mut row = json!({
        "media_id": m["id"],
        "kind": m["kind"],
        "title": m["title"],
        "year": m["year"],
        "size_gb": gb(&m["size_on_disk"]),
        "added": date(&m["added_at"]),
        "rating": m["rating"],
        "genres": m["genres"],
        "status": m["status"],
    });
    if m["kind"] == "series" {
        row["seasons"] = m["season_count"].clone();
        row["episodes_on_disk"] = m["file_count"].clone();
    } else {
        row["quality"] = m["quality"].clone();
    }
    if !m["requested_by"].is_null() {
        row["requested_by"] = m["requested_by"].clone();
    }
    if m["marks"].as_array().is_some_and(|a| !a.is_empty()) {
        row["pending_marks"] = m["marks"].clone();
    }
    row
}

fn compact_mark(k: &Value) -> Value {
    json!({
        "mark_id": k["id"],
        "media_id": k["media_id"],
        "title": k["media_title"],
        "scope": k["scope"],
        "target": k["label"],
        "action": k["action"],
        "reason": k["reason"],
        "source": k["source"],
        "size_gb": gb(&k["size_bytes"]),
        "status": k["status"],
        "error": k["error"],
    })
}

async fn call_tool(state: &AppState, name: &str, args: Value) -> Result<Value> {
    let bad_args = |e: serde_json::Error| AppError::bad_request(format!("Invalid arguments: {e}"));
    match name {
        "library_stats" => {
            let s = state.db(|c| Ok(db::status(c, false)?))?;
            Ok(json!({
                "movies": { "count": s["library"]["movie"]["count"], "size_gb": gb(&s["library"]["movie"]["size"]) },
                "series": { "count": s["library"]["series"]["count"], "size_gb": gb(&s["library"]["series"]["size"]) },
                "pending_marks": { "count": s["pending"]["count"], "reclaim_gb": gb(&s["pending"]["reclaim_bytes"]) },
                "services": s["services"],
                "last_sync": s["last_sync"],
            }))
        }
        "list_media" => {
            let mut filter: MediaFilter = serde_json::from_value(args).map_err(bad_args)?;
            filter.limit = Some(filter.limit.unwrap_or(50).clamp(1, 500));
            filter.include_missing = None;
            let res = state.db(|c| Ok(db::list_media(c, &filter)?))?;
            let items: Vec<Value> = res["items"].as_array().into_iter().flatten().map(compact).collect();
            Ok(json!({
                "total": res["total"],
                "total_size_gb": gb(&res["total_size"]),
                "offset": filter.offset.unwrap_or(0),
                "returned": items.len(),
                "items": items,
            }))
        }
        "get_media" => {
            let id = args["media_id"]
                .as_i64()
                .ok_or_else(|| AppError::bad_request("media_id is required"))?;
            let (media, seasons) = state.db(|c| {
                let media = db::get_media(c, id)?.ok_or_else(|| AppError::not_found(format!("Media {id} not found")))?;
                Ok((media, db::get_seasons(c, id)?))
            })?;
            let mut out = compact(&media);
            for key in ["overview", "path", "network", "runtime", "monitored", "imdb_id"] {
                out[key] = media[key].clone();
            }
            if media["kind"] == "series" {
                out["season_list"] = seasons
                    .iter()
                    .map(|s| {
                        json!({
                            "season_number": s["season_number"],
                            "episodes_on_disk": s["episode_file_count"],
                            "episodes_total": s["total_episode_count"],
                            "size_gb": gb(&s["size_on_disk"]),
                            "monitored": s["monitored"],
                        })
                    })
                    .collect();
                if let Some(n) = args["season_number"].as_i64() {
                    out["episodes"] = marks::episodes(state, id, Some(n))
                        .await?
                        .iter()
                        .map(|e| {
                            json!({
                                "episode_id": e["id"],
                                "episode": e["episode_number"],
                                "title": e["title"],
                                "air_date": e["air_date"],
                                "has_file": e["has_file"],
                                "size_gb": gb(&e["size"]),
                                "quality": e["quality"],
                            })
                        })
                        .collect();
                }
            }
            Ok(out)
        }
        "mark_items" => {
            let items: Vec<NewMark> = serde_json::from_value(args["items"].clone()).map_err(bad_args)?;
            let mut results = Vec::new();
            for item in items {
                let media_id = item.media_id;
                results.push(match marks::create(state, item, "llm").await {
                    Ok(mark) => json!({ "ok": true, "mark": compact_mark(&mark) }),
                    Err(e) => json!({ "ok": false, "media_id": media_id, "error": e.message }),
                });
            }
            Ok(json!({
                "results": results,
                "note": "Marks are pending. The user must review and apply them in the web UI.",
            }))
        }
        "list_marks" => {
            let all = state.db(|c| Ok(db::list_marks(c)?))?;
            let map = |v: &Value| -> Vec<Value> { v.as_array().into_iter().flatten().map(compact_mark).collect() };
            let mut out = json!({ "pending": map(&all["pending"]) });
            if args["include_history"] == true {
                out["history"] = json!(map(&all["history"]));
            }
            Ok(out)
        }
        "unmark_items" => {
            let ids: Vec<i64> = serde_json::from_value(args["mark_ids"].clone()).map_err(bad_args)?;
            Ok(json!({ "removed": marks::remove(state, &ids)? }))
        }
        "sync_library" => sync::sync_all(state).await,
        other => Err(AppError::not_found(format!("Unknown tool '{other}'"))),
    }
}

/// Handle one JSON-RPC message. Notifications (no id) produce no response.
pub async fn handle(state: &AppState, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned().filter(|id| !id.is_null())?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match msg["method"].as_str().unwrap_or_default() {
        "initialize" => {
            let requested = params["protocolVersion"].as_str().unwrap_or_default();
            let version = if PROTOCOL_VERSIONS.contains(&requested) { requested } else { PROTOCOL_VERSIONS[0] };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "plex-delete-list", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            // Tool failures are reported in-band so the model can read and react to them.
            Ok(match call_tool(state, name, args).await {
                Ok(v) => json!({
                    "content": [{ "type": "text", "text": v.to_string() }],
                    "isError": false,
                }),
                Err(e) => json!({ "content": [{ "type": "text", "text": e.message }], "isError": true }),
            })
        }
        method => Err(json!({ "code": -32601, "message": format!("Method not found: {method}") })),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

/// `POST /mcp`
pub async fn http(State(state): State<Shared>, Json(body): Json<Value>) -> Response {
    let reply = match body {
        Value::Array(batch) => {
            let mut replies = Vec::new();
            for msg in batch {
                if let Some(r) = handle(&state, msg).await {
                    replies.push(r);
                }
            }
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        msg => handle(&state, msg).await,
    };
    match reply {
        Some(reply) => Json(reply).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// `plex-delete-list mcp`: newline-delimited JSON-RPC on stdin/stdout, forwarded to a running server.
pub async fn stdio_proxy(url: String, key: Option<String>) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let endpoint = format!("{}/mcp", url.trim_end_matches('/'));
    eprintln!("plex-delete-list mcp: bridging stdio to {endpoint}");
    let http = reqwest::Client::new();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let id = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|v| v.get("id").cloned())
            .filter(|id| !id.is_null());
        let mut req = http
            .post(&endpoint)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(line.to_string());
        if let Some(key) = &key {
            req = req.bearer_auth(key);
        }
        let outcome: std::result::Result<Option<Value>, String> = match req.send().await {
            Ok(res) if res.status() == StatusCode::ACCEPTED => Ok(None),
            Ok(res) if res.status().is_success() => res.json::<Value>().await.map(Some).map_err(|e| e.to_string()),
            Ok(res) => Err(format!("Plex Delete List server at {url} returned {}", res.status())),
            Err(e) => Err(format!("Cannot reach Plex Delete List server at {url}: {e}")),
        };
        let reply = match outcome {
            Ok(reply) => reply,
            Err(message) => {
                eprintln!("plex-delete-list mcp: {message}");
                id.map(|id| json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } }))
            }
        };
        if let Some(reply) = reply {
            stdout.write_all(format!("{reply}\n").as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
