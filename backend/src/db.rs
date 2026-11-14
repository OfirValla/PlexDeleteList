use std::path::Path;

use rusqlite::{
    types::{Value as SqlValue, ValueRef},
    Connection, OptionalExtension, Row,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};

pub const SERVICES: [&str; 3] = ["radarr", "sonarr", "seerr"];
pub const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
pub const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%SZ','now')";

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS media (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    kind           TEXT NOT NULL CHECK (kind IN ('movie', 'series')),
    arr_id         INTEGER NOT NULL,
    title          TEXT NOT NULL,
    sort_title     TEXT NOT NULL,
    year           INTEGER,
    overview       TEXT,
    poster_url     TEXT,
    fanart_url     TEXT,
    tmdb_id        INTEGER,
    tvdb_id        INTEGER,
    imdb_id        TEXT,
    size_on_disk   INTEGER NOT NULL DEFAULT 0,
    file_count     INTEGER NOT NULL DEFAULT 0,
    episode_count  INTEGER NOT NULL DEFAULT 0,
    season_count   INTEGER NOT NULL DEFAULT 0,
    status         TEXT,
    added_at       TEXT,
    genres         TEXT NOT NULL DEFAULT '[]',
    rating         REAL,
    path           TEXT,
    quality        TEXT,
    monitored      INTEGER NOT NULL DEFAULT 0,
    runtime        INTEGER,
    network        TEXT,
    file_id        INTEGER,
    requested_by   TEXT,
    seerr_media_id INTEGER,
    synced_at      TEXT NOT NULL,
    UNIQUE (kind, arr_id)
);

CREATE TABLE IF NOT EXISTS seasons (
    media_id            INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    season_number       INTEGER NOT NULL,
    episode_file_count  INTEGER NOT NULL DEFAULT 0,
    episode_count       INTEGER NOT NULL DEFAULT 0,
    total_episode_count INTEGER NOT NULL DEFAULT 0,
    size_on_disk        INTEGER NOT NULL DEFAULT 0,
    monitored           INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (media_id, season_number)
);

CREATE TABLE IF NOT EXISTS marks (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id       INTEGER REFERENCES media(id) ON DELETE SET NULL,
    media_kind     TEXT NOT NULL,
    media_title    TEXT NOT NULL,
    poster_url     TEXT,
    scope          TEXT NOT NULL CHECK (scope IN ('movie', 'series', 'season', 'episode')),
    season_number  INTEGER,
    episode_id     INTEGER,
    episode_number INTEGER,
    label          TEXT NOT NULL,
    action         TEXT NOT NULL CHECK (action IN ('delete', 'redownload')),
    reason         TEXT,
    source         TEXT NOT NULL DEFAULT 'user',
    size_bytes     INTEGER NOT NULL DEFAULT 0,
    status         TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'done', 'failed')),
    error          TEXT,
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
    executed_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_marks_media ON marks (media_id, status);
CREATE INDEX IF NOT EXISTS idx_marks_status ON marks (status);
";

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    if let Some(parent) = Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

// ---------- generic row helpers ----------

pub fn row_to_json(row: &Row) -> rusqlite::Result<Value> {
    let stmt = row.as_ref();
    let mut map = Map::new();
    for i in 0..stmt.column_count() {
        let name = stmt.column_name(i)?.to_string();
        let value = match row.get_ref(i)? {
            ValueRef::Null | ValueRef::Blob(_) => Value::Null,
            ValueRef::Integer(n) => json!(n),
            ValueRef::Real(f) => json!(f),
            ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        };
        map.insert(name, value);
    }
    Ok(Value::Object(map))
}

pub fn query_json(conn: &Connection, sql: &str, params: &[SqlValue]) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), row_to_json)?;
    rows.collect()
}

/// Turn JSON-in-text columns into real JSON and 0/1 into booleans.
fn fix_media(mut v: Value) -> Value {
    for key in ["genres", "marks"] {
        if let Some(text) = v.get(key).and_then(Value::as_str) {
            let parsed = serde_json::from_str::<Value>(text).unwrap_or_else(|_| json!([]));
            v[key] = parsed;
        }
    }
    if let Some(n) = v.get("monitored").and_then(Value::as_i64) {
        v["monitored"] = json!(n != 0);
    }
    v
}

// ---------- settings ----------

/// Value from the settings table, falling back to an environment variable of the same (upper-cased) name.
pub fn get_setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    let stored: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()?;
    Ok(stored
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var(key.to_uppercase()).ok().filter(|v| !v.is_empty())))
}

pub fn set_setting(conn: &Connection, key: &str, value: Option<&str>) -> rusqlite::Result<()> {
    match value {
        Some(v) if !v.is_empty() => conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, v],
        )?,
        _ => conn.execute("DELETE FROM settings WHERE key = ?1", [key])?,
    };
    Ok(())
}

/// (url, api key) for a service when both are configured.
pub fn service(conn: &Connection, name: &str) -> rusqlite::Result<Option<(String, String)>> {
    let url = get_setting(conn, &format!("{name}_url"))?;
    let key = get_setting(conn, &format!("{name}_api_key"))?;
    Ok(url.zip(key))
}

// ---------- media ----------

#[derive(Debug, Default, Deserialize)]
pub struct MediaFilter {
    pub kind: Option<String>,
    pub q: Option<String>,
    pub genre: Option<String>,
    pub requested_by: Option<String>,
    pub min_size_gb: Option<f64>,
    pub max_size_gb: Option<f64>,
    pub added_before: Option<String>,
    pub added_after: Option<String>,
    pub marked: Option<bool>,
    /// Include items that have no files on disk (and so aren't in Plex).
    pub include_missing: Option<bool>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

const PENDING_MARKS: &str = "(SELECT json_group_array(json_object(
        'id', k.id, 'scope', k.scope, 'season_number', k.season_number, 'episode_id', k.episode_id,
        'action', k.action, 'source', k.source, 'reason', k.reason, 'label', k.label))
      FROM marks k WHERE k.media_id = m.id AND k.status = 'pending') AS marks";

/// Returns `{ total, total_size, items }`. `total` ignores limit/offset.
pub fn list_media(conn: &Connection, f: &MediaFilter) -> rusqlite::Result<Value> {
    let mut wh: Vec<&str> = vec!["1 = 1"];
    let mut p: Vec<SqlValue> = Vec::new();

    if let Some(kind) = f.kind.as_deref().filter(|k| *k == "movie" || *k == "series") {
        wh.push("m.kind = ?");
        p.push(kind.to_string().into());
    }
    if let Some(q) = f.q.as_deref().filter(|q| !q.is_empty()) {
        wh.push("m.title LIKE ?");
        p.push(format!("%{q}%").into());
    }
    if let Some(genre) = f.genre.as_deref().filter(|g| !g.is_empty()) {
        wh.push("m.genres LIKE ?");
        p.push(format!("%\"{genre}\"%").into());
    }
    if let Some(who) = f.requested_by.as_deref().filter(|w| !w.is_empty()) {
        wh.push("m.requested_by LIKE ?");
        p.push(format!("%{who}%").into());
    }
    if let Some(min) = f.min_size_gb {
        wh.push("m.size_on_disk >= ?");
        p.push(((min * GIB) as i64).into());
    }
    if let Some(max) = f.max_size_gb {
        wh.push("m.size_on_disk <= ?");
        p.push(((max * GIB) as i64).into());
    }
    if let Some(before) = f.added_before.as_deref().filter(|d| !d.is_empty()) {
        wh.push("m.added_at < ?");
        p.push(before.to_string().into());
    }
    if let Some(after) = f.added_after.as_deref().filter(|d| !d.is_empty()) {
        wh.push("m.added_at >= ?");
        p.push(after.to_string().into());
    }
    match f.marked {
        Some(true) => wh.push("EXISTS (SELECT 1 FROM marks k WHERE k.media_id = m.id AND k.status = 'pending')"),
        Some(false) => wh.push("NOT EXISTS (SELECT 1 FROM marks k WHERE k.media_id = m.id AND k.status = 'pending')"),
        None => {}
    }
    if f.include_missing != Some(true) {
        wh.push("m.file_count > 0");
    }
    let wh = wh.join(" AND ");

    let (column, default_order) = match f.sort.as_deref() {
        Some("title") => ("m.sort_title", "ASC"),
        Some("added") => ("m.added_at", "DESC"),
        Some("year") => ("m.year", "DESC"),
        Some("rating") => ("m.rating", "DESC"),
        _ => ("m.size_on_disk", "DESC"),
    };
    let order = match f.order.as_deref() {
        Some("asc") => "ASC",
        Some("desc") => "DESC",
        _ => default_order,
    };
    let mut sql = format!(
        "SELECT m.*, {PENDING_MARKS} FROM media m WHERE {wh} ORDER BY {column} {order}, m.sort_title ASC"
    );
    if let Some(limit) = f.limit {
        sql.push_str(&format!(" LIMIT {} OFFSET {}", limit.clamp(1, 100_000), f.offset.unwrap_or(0).max(0)));
    }

    let (total, total_size): (i64, i64) = conn.query_row(
        &format!("SELECT COUNT(*), COALESCE(SUM(m.size_on_disk), 0) FROM media m WHERE {wh}"),
        rusqlite::params_from_iter(p.iter()),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let items: Vec<Value> = query_json(conn, &sql, &p)?.into_iter().map(fix_media).collect();
    Ok(json!({ "total": total, "total_size": total_size, "items": items }))
}

pub fn get_media(conn: &Connection, id: i64) -> rusqlite::Result<Option<Value>> {
    let sql = format!("SELECT m.*, {PENDING_MARKS} FROM media m WHERE m.id = ?");
    Ok(query_json(conn, &sql, &[id.into()])?.into_iter().next().map(fix_media))
}

pub fn get_seasons(conn: &Connection, media_id: i64) -> rusqlite::Result<Vec<Value>> {
    let rows = query_json(
        conn,
        "SELECT season_number, episode_file_count, episode_count, total_episode_count, size_on_disk, monitored
         FROM seasons WHERE media_id = ? ORDER BY season_number",
        &[media_id.into()],
    )?;
    Ok(rows.into_iter().map(fix_media).collect())
}

// ---------- marks & status ----------

pub fn list_marks(conn: &Connection) -> rusqlite::Result<Value> {
    let pending = query_json(conn, "SELECT * FROM marks WHERE status = 'pending' ORDER BY id DESC", &[])?;
    let history = query_json(
        conn,
        "SELECT * FROM marks WHERE status <> 'pending' ORDER BY executed_at DESC, id DESC LIMIT 200",
        &[],
    )?;
    Ok(json!({ "pending": pending, "history": history }))
}

pub fn status(conn: &Connection, auth_required: bool) -> rusqlite::Result<Value> {
    let mut services = Map::new();
    for name in SERVICES {
        services.insert(name.to_string(), json!(service(conn, name)?.is_some()));
    }
    let mut library = json!({
        "movie": { "count": 0, "size": 0 },
        "series": { "count": 0, "size": 0 },
    });
    let mut stmt = conn.prepare(
        "SELECT kind, COUNT(*), COALESCE(SUM(size_on_disk), 0) FROM media WHERE file_count > 0 GROUP BY kind",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;
    for row in rows {
        let (kind, count, size) = row?;
        library[kind] = json!({ "count": count, "size": size });
    }
    let (pending, reclaim): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN action = 'delete' THEN size_bytes ELSE 0 END), 0)
         FROM marks WHERE status = 'pending'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(json!({
        "services": services,
        "last_sync": get_setting(conn, "last_sync")?,
        "library": library,
        "pending": { "count": pending, "reclaim_bytes": reclaim },
        "auth_required": auth_required,
    }))
}
