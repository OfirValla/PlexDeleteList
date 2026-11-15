//! The delete list: marking items for delete/redownload.
//!
//! Marks are only a record of what the user should remove. Nothing here (or anywhere in this app) acts on
//! them; `sync` ticks a delete mark off once it sees the files are gone.

use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    arr::Client,
    db,
    error::{AppError, Result},
    AppState,
};

#[derive(Debug, Deserialize)]
pub struct NewMark {
    pub media_id: i64,
    /// movie | series | season | episode. Defaults to the whole item.
    pub scope: Option<String>,
    pub season_number: Option<i64>,
    /// Sonarr episode id, as returned by the episodes listing.
    pub episode_id: Option<i64>,
    /// delete | redownload
    pub action: String,
    pub reason: Option<String>,
}

fn season_label(n: i64) -> String {
    if n == 0 {
        "Specials".into()
    } else {
        format!("Season {n}")
    }
}

fn episode_json(e: &Value) -> Value {
    let file = &e["episodeFile"];
    json!({
        "id": e["id"],
        "season_number": e["seasonNumber"],
        "episode_number": e["episodeNumber"],
        "title": e["title"],
        "air_date": e["airDate"],
        "has_file": e["hasFile"].as_bool().unwrap_or(false),
        "file_id": e["episodeFileId"].as_i64().filter(|n| *n > 0),
        "size": file["size"].as_i64().unwrap_or(0),
        "quality": file["quality"]["quality"]["name"],
        "monitored": e["monitored"].as_bool().unwrap_or(false),
    })
}

/// Episodes of a series, live from Sonarr (they aren't cached locally).
pub async fn episodes(state: &AppState, media_id: i64, season: Option<i64>) -> Result<Vec<Value>> {
    let media = state
        .db(|c| Ok(db::get_media(c, media_id)?))?
        .ok_or_else(|| AppError::not_found("Media not found"))?;
    if media["kind"] != "series" {
        return Err(AppError::bad_request("Only series have episodes"));
    }
    let sonarr = Client::require(state, "sonarr")?;
    let mut query = vec![
        ("seriesId", media["arr_id"].to_string()),
        ("includeEpisodeFile", "true".to_string()),
    ];
    if let Some(n) = season {
        query.push(("seasonNumber", n.to_string()));
    }
    let res = sonarr.get("/episode", &query).await?;
    let mut list: Vec<Value> = res.as_array().into_iter().flatten().map(episode_json).collect();
    list.sort_by_key(|e| (e["season_number"].as_i64(), e["episode_number"].as_i64()));
    Ok(list)
}

/// Add a pending mark. A broader mark replaces narrower ones; a narrower mark under a broader one is rejected.
pub async fn create(state: &AppState, input: NewMark, source: &str) -> Result<Value> {
    if !matches!(input.action.as_str(), "delete" | "redownload") {
        return Err(AppError::bad_request("action must be 'delete' or 'redownload'"));
    }
    let delete = input.action == "delete";
    let id = input.media_id;
    let media = state
        .db(|c| Ok(db::get_media(c, id)?))?
        .ok_or_else(|| AppError::not_found(format!("Media {id} not found")))?;
    let kind = media["kind"].as_str().unwrap_or_default().to_string();
    let title = media["title"].as_str().unwrap_or_default().to_string();
    let scope = input.scope.clone().unwrap_or_else(|| kind.clone());

    // A delete mark needs files to point at: sync ticks it off as soon as there are none.
    let (label, size, season_number, episode_id, episode_number) = match (kind.as_str(), scope.as_str()) {
        ("movie", "movie") | ("series", "series") => {
            if delete && media["file_count"].as_i64().unwrap_or(0) == 0 {
                return Err(AppError::bad_request(format!("'{title}' has no files to delete")));
            }
            (title.clone(), media["size_on_disk"].as_i64().unwrap_or(0), None, None, None)
        }
        ("series", "season") => {
            let n = input
                .season_number
                .ok_or_else(|| AppError::bad_request("season_number is required for scope 'season'"))?;
            let (size, files) = state
                .db(|c| {
                    Ok(c.query_row(
                        "SELECT size_on_disk, episode_file_count FROM seasons
                         WHERE media_id = ?1 AND season_number = ?2",
                        params![id, n],
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                    )
                    .optional()?)
                })?
                .ok_or_else(|| AppError::not_found(format!("'{title}' has no season {n}")))?;
            if delete && files == 0 {
                return Err(AppError::bad_request(format!(
                    "{} of '{title}' has no files to delete",
                    season_label(n)
                )));
            }
            (season_label(n), size, Some(n), None, None)
        }
        ("series", "episode") => {
            let eid = input
                .episode_id
                .ok_or_else(|| AppError::bad_request("episode_id is required for scope 'episode'"))?;
            let sonarr = Client::require(state, "sonarr")?;
            let ep = sonarr.get(&format!("/episode/{eid}"), &[]).await?;
            if ep["seriesId"].as_i64() != media["arr_id"].as_i64() {
                return Err(AppError::bad_request(format!("Episode {eid} does not belong to '{title}'")));
            }
            if delete && ep["hasFile"] != true {
                return Err(AppError::bad_request("Episode has no file to delete"));
            }
            let s = ep["seasonNumber"].as_i64().unwrap_or(0);
            let e = ep["episodeNumber"].as_i64().unwrap_or(0);
            let label = format!("S{s:02}E{e:02} · {}", ep["title"].as_str().unwrap_or("Episode"));
            (label, ep["episodeFile"]["size"].as_i64().unwrap_or(0), Some(s), Some(eid), Some(e))
        }
        _ => return Err(AppError::bad_request(format!("scope '{scope}' is not valid for a {kind}"))),
    };

    let reason = input.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
    state.db(|c| {
        let tx = c.transaction()?;
        let covered_by: Option<String> = match scope.as_str() {
            "season" => tx
                .query_row(
                    "SELECT scope FROM marks WHERE media_id = ?1 AND status = 'pending' AND scope = 'series' LIMIT 1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?,
            "episode" => tx
                .query_row(
                    "SELECT scope FROM marks WHERE media_id = ?1 AND status = 'pending'
                       AND (scope = 'series' OR (scope = 'season' AND season_number = ?2)) LIMIT 1",
                    params![id, season_number],
                    |r| r.get(0),
                )
                .optional()?,
            _ => None,
        };
        if let Some(broader) = covered_by {
            return Err(AppError::conflict(format!(
                "'{title}' already has a pending mark on the whole {broader}; remove it first"
            )));
        }
        match scope.as_str() {
            "season" => tx.execute(
                "DELETE FROM marks WHERE media_id = ?1 AND status = 'pending' AND season_number = ?2",
                params![id, season_number],
            )?,
            "episode" => tx.execute(
                "DELETE FROM marks WHERE media_id = ?1 AND status = 'pending' AND episode_id = ?2",
                params![id, episode_id],
            )?,
            _ => tx.execute("DELETE FROM marks WHERE media_id = ?1 AND status = 'pending'", params![id])?,
        };
        tx.execute(
            "INSERT INTO marks (media_id, media_kind, media_title, poster_url, scope, season_number, episode_id,
                                episode_number, label, action, reason, source, size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                kind,
                title,
                media["poster_url"].as_str(),
                scope,
                season_number,
                episode_id,
                episode_number,
                label,
                input.action,
                reason,
                source,
                size
            ],
        )?;
        let mark_id = tx.last_insert_rowid();
        let row = db::query_json(&tx, "SELECT * FROM marks WHERE id = ?", &[mark_id.into()])?
            .into_iter()
            .next()
            .unwrap_or(Value::Null);
        tx.commit()?;
        Ok(row)
    })
}

/// Remove pending marks by id. Returns how many were removed.
pub fn remove(state: &AppState, ids: &[i64]) -> Result<usize> {
    state.db(|c| {
        let mut removed = 0;
        for id in ids {
            removed += c.execute("DELETE FROM marks WHERE id = ?1 AND status = 'pending'", [id])?;
        }
        Ok(removed)
    })
}
