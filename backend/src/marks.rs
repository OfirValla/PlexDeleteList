//! The review queue: marking items for delete/redownload, and applying those marks through Radarr/Sonarr.

use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    arr::Client,
    db::{self, NOW},
    error::{AppError, Result},
    sync, AppState,
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
    let id = input.media_id;
    let media = state
        .db(|c| Ok(db::get_media(c, id)?))?
        .ok_or_else(|| AppError::not_found(format!("Media {id} not found")))?;
    let kind = media["kind"].as_str().unwrap_or_default().to_string();
    let title = media["title"].as_str().unwrap_or_default().to_string();
    let scope = input.scope.clone().unwrap_or_else(|| kind.clone());

    let (label, size, season_number, episode_id, episode_number) = match (kind.as_str(), scope.as_str()) {
        ("movie", "movie") | ("series", "series") => {
            (title.clone(), media["size_on_disk"].as_i64().unwrap_or(0), None, None, None)
        }
        ("series", "season") => {
            let n = input
                .season_number
                .ok_or_else(|| AppError::bad_request("season_number is required for scope 'season'"))?;
            let size = state
                .db(|c| {
                    Ok(c.query_row(
                        "SELECT size_on_disk FROM seasons WHERE media_id = ?1 AND season_number = ?2",
                        params![id, n],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()?)
                })?
                .ok_or_else(|| AppError::not_found(format!("'{title}' has no season {n}")))?;
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
            if input.action == "delete" && ep["hasFile"] != true {
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

/// Execute pending marks (all, or just `ids`) against Radarr/Sonarr, then resync.
pub async fn apply(state: &AppState, ids: Option<Vec<i64>>) -> Result<Value> {
    let _guard = state.work_lock.lock().await;
    let mut pending = state.db(|c| {
        Ok(db::query_json(
            c,
            "SELECT k.*, m.arr_id, m.seerr_media_id FROM marks k
             LEFT JOIN media m ON m.id = k.media_id
             WHERE k.status = 'pending' ORDER BY k.id",
            &[],
        )?)
    })?;
    if let Some(ids) = &ids {
        pending.retain(|m| m["id"].as_i64().is_some_and(|id| ids.contains(&id)));
    }

    let mut results = Vec::new();
    let (mut applied, mut failed) = (0, 0);
    for mark in &pending {
        let mark_id = mark["id"].as_i64().unwrap_or(0);
        let outcome = execute(state, mark).await;
        let error = outcome.as_ref().err().map(|e| e.message.clone());
        state.db(|c| {
            c.execute(
                &format!("UPDATE marks SET status = ?1, error = ?2, executed_at = {NOW} WHERE id = ?3"),
                params![if error.is_none() { "done" } else { "failed" }, error, mark_id],
            )?;
            let whole = matches!(mark["scope"].as_str(), Some("movie" | "series"));
            if error.is_none() && whole && mark["action"] == "delete" {
                c.execute("DELETE FROM media WHERE id = ?1", params![mark["media_id"].as_i64()])?;
            }
            Ok(())
        })?;
        match &error {
            None => applied += 1,
            Some(e) => {
                failed += 1;
                tracing::warn!("mark {mark_id} ({}) failed: {e}", mark["label"]);
            }
        }
        results.push(json!({
            "id": mark_id,
            "media_title": mark["media_title"],
            "label": mark["label"],
            "action": mark["action"],
            "status": if error.is_none() { "done" } else { "failed" },
            "error": error,
        }));
    }

    let sync_report = if pending.is_empty() {
        Value::Null
    } else {
        sync::sync_inner(state).await.unwrap_or_else(|e| json!({ "errors": [e.message] }))
    };
    Ok(json!({ "applied": applied, "failed": failed, "results": results, "sync": sync_report }))
}

async fn execute(state: &AppState, mark: &Value) -> Result<()> {
    let arr_id = mark["arr_id"]
        .as_i64()
        .ok_or_else(|| AppError::not_found("Item is no longer in the library"))?;
    let redownload = mark["action"] == "redownload";
    let season = mark["season_number"].as_i64();

    match mark["scope"].as_str().unwrap_or_default() {
        "movie" => {
            let radarr = Client::require(state, "radarr")?;
            if redownload {
                let mut movie = radarr.get(&format!("/movie/{arr_id}"), &[]).await?;
                let file_id = movie["movieFile"]["id"].as_i64().or(movie["movieFileId"].as_i64()).filter(|n| *n > 0);
                if movie["monitored"] != true {
                    movie["monitored"] = json!(true);
                    radarr.put(&format!("/movie/{arr_id}"), &movie).await?;
                }
                if let Some(file_id) = file_id {
                    radarr.delete(&format!("/moviefile/{file_id}"), &[], None).await?;
                }
                radarr.post("/command", &json!({ "name": "MoviesSearch", "movieIds": [arr_id] })).await?;
            } else {
                radarr
                    .delete(
                        &format!("/movie/{arr_id}"),
                        &[("deleteFiles", "true".into()), ("addImportExclusion", "false".into())],
                        None,
                    )
                    .await?;
                forget_in_seerr(state, mark).await;
            }
        }
        "series" => {
            let sonarr = Client::require(state, "sonarr")?;
            if redownload {
                set_season_monitoring(&sonarr, arr_id, None, true).await?;
                delete_episode_files(&sonarr, arr_id, None).await?;
                sonarr.post("/command", &json!({ "name": "SeriesSearch", "seriesId": arr_id })).await?;
            } else {
                sonarr
                    .delete(
                        &format!("/series/{arr_id}"),
                        &[("deleteFiles", "true".into()), ("addImportListExclusion", "false".into())],
                        None,
                    )
                    .await?;
                forget_in_seerr(state, mark).await;
            }
        }
        "season" => {
            let sonarr = Client::require(state, "sonarr")?;
            let n = season.ok_or_else(|| AppError::bad_request("Mark has no season number"))?;
            // Unmonitor before deleting, otherwise Sonarr would just grab the season again.
            set_season_monitoring(&sonarr, arr_id, Some(n), redownload).await?;
            delete_episode_files(&sonarr, arr_id, Some(n)).await?;
            if redownload {
                sonarr
                    .post("/command", &json!({ "name": "SeasonSearch", "seriesId": arr_id, "seasonNumber": n }))
                    .await?;
            }
        }
        "episode" => {
            let sonarr = Client::require(state, "sonarr")?;
            let eid = mark["episode_id"]
                .as_i64()
                .ok_or_else(|| AppError::bad_request("Mark has no episode id"))?;
            let episode = sonarr.get(&format!("/episode/{eid}"), &[]).await?;
            sonarr
                .put("/episode/monitor", &json!({ "episodeIds": [eid], "monitored": redownload }))
                .await?;
            if let Some(file_id) = episode["episodeFileId"].as_i64().filter(|n| *n > 0) {
                sonarr.delete(&format!("/episodefile/{file_id}"), &[], None).await?;
            }
            if redownload {
                sonarr.post("/command", &json!({ "name": "EpisodeSearch", "episodeIds": [eid] })).await?;
            }
        }
        other => return Err(AppError::bad_request(format!("Unknown scope '{other}'"))),
    }
    Ok(())
}

/// Set the monitored flag of one season (or, with `season = None`, make sure the series itself is monitored).
async fn set_season_monitoring(sonarr: &Client, series_id: i64, season: Option<i64>, monitored: bool) -> Result<()> {
    let mut series = sonarr.get(&format!("/series/{series_id}"), &[]).await?;
    let mut changed = false;
    if monitored && series["monitored"] != true {
        series["monitored"] = json!(true);
        changed = true;
    }
    if let (Some(n), Some(seasons)) = (season, series["seasons"].as_array_mut()) {
        for s in seasons.iter_mut().filter(|s| s["seasonNumber"].as_i64() == Some(n)) {
            if s["monitored"].as_bool() != Some(monitored) {
                s["monitored"] = json!(monitored);
                changed = true;
            }
        }
    }
    if changed {
        sonarr.put(&format!("/series/{series_id}"), &series).await?;
    }
    Ok(())
}

async fn delete_episode_files(sonarr: &Client, series_id: i64, season: Option<i64>) -> Result<()> {
    let files = sonarr.get("/episodefile", &[("seriesId", series_id.to_string())]).await?;
    let ids: Vec<i64> = files
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| season.is_none() || f["seasonNumber"].as_i64() == season)
        .filter_map(|f| f["id"].as_i64())
        .collect();
    if !ids.is_empty() {
        sonarr
            .delete("/episodefile/bulk", &[], Some(&json!({ "episodeFileIds": ids })))
            .await?;
    }
    Ok(())
}

/// Best effort: clear the item in Seerr so it shows as requestable again.
async fn forget_in_seerr(state: &AppState, mark: &Value) {
    let Some(media_id) = mark["seerr_media_id"].as_i64() else { return };
    let Ok(Some(seerr)) = Client::for_service(state, "seerr") else { return };
    if let Err(e) = seerr.delete(&format!("/media/{media_id}"), &[], None).await {
        tracing::warn!("could not clear '{}' in Seerr: {e}", mark["media_title"]);
    }
}
