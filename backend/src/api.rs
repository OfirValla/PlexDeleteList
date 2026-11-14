use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    arr::Client,
    db::{self, MediaFilter, SERVICES},
    error::{AppError, Result},
    marks::{self, NewMark},
    sync, Shared,
};

pub async fn status(State(st): State<Shared>) -> Result<Json<Value>> {
    let auth_required = st.api_key.is_some();
    Ok(Json(st.db(|c| Ok(db::status(c, auth_required)?))?))
}

pub async fn get_settings(State(st): State<Shared>) -> Result<Json<Value>> {
    let settings = st.db(|c| {
        let mut out = Map::new();
        for name in SERVICES {
            out.insert(
                name.to_string(),
                json!({
                    "url": db::get_setting(c, &format!("{name}_url"))?,
                    // The key itself never leaves the server.
                    "has_key": db::get_setting(c, &format!("{name}_api_key"))?.is_some(),
                }),
            );
        }
        Ok(Value::Object(out))
    })?;
    Ok(Json(settings))
}

/// Body: `{ "<service>": { "url": "...", "api_key": "..." } }`. An empty/missing api_key keeps the stored one.
pub async fn put_settings(
    State(st): State<Shared>,
    Json(body): Json<HashMap<String, HashMap<String, Option<String>>>>,
) -> Result<Json<Value>> {
    st.db(|c| {
        for name in SERVICES {
            let Some(cfg) = body.get(name) else { continue };
            if let Some(url) = cfg.get("url") {
                db::set_setting(c, &format!("{name}_url"), url.as_deref().map(str::trim))?;
            }
            if let Some(Some(key)) = cfg.get("api_key") {
                if !key.trim().is_empty() {
                    db::set_setting(c, &format!("{name}_api_key"), Some(key.trim()))?;
                }
            }
        }
        Ok(())
    })?;
    get_settings(State(st)).await
}

pub async fn test_service(State(st): State<Shared>, Path(service): Path<String>) -> Result<Json<Value>> {
    if !SERVICES.contains(&service.as_str()) {
        return Err(AppError::not_found("Unknown service"));
    }
    let path = if service == "seerr" { "/settings/about" } else { "/system/status" };
    let outcome = match Client::require(&st, &service) {
        Ok(client) => client.get(path, &[]).await,
        Err(e) => Err(e),
    };
    Ok(Json(match outcome {
        Ok(v) => json!({ "ok": true, "version": v["version"] }),
        Err(e) => json!({ "ok": false, "error": e.message }),
    }))
}

pub async fn sync(State(st): State<Shared>) -> Result<Json<Value>> {
    Ok(Json(sync::sync_all(&st).await?))
}

pub async fn list_media(State(st): State<Shared>, Query(filter): Query<MediaFilter>) -> Result<Json<Value>> {
    Ok(Json(st.db(|c| Ok(db::list_media(c, &filter)?))?))
}

pub async fn get_media(State(st): State<Shared>, Path(id): Path<i64>) -> Result<Json<Value>> {
    let detail = st.db(|c| {
        let media = db::get_media(c, id)?.ok_or_else(|| AppError::not_found("Media not found"))?;
        let seasons = db::get_seasons(c, id)?;
        Ok(json!({ "media": media, "seasons": seasons }))
    })?;
    Ok(Json(detail))
}

#[derive(Deserialize)]
pub struct EpisodesQuery {
    season: Option<i64>,
}

pub async fn episodes(
    State(st): State<Shared>,
    Path(id): Path<i64>,
    Query(q): Query<EpisodesQuery>,
) -> Result<Json<Value>> {
    Ok(Json(Value::Array(marks::episodes(&st, id, q.season).await?)))
}

pub async fn list_marks(State(st): State<Shared>) -> Result<Json<Value>> {
    Ok(Json(st.db(|c| Ok(db::list_marks(c)?))?))
}

pub async fn create_mark(State(st): State<Shared>, Json(input): Json<NewMark>) -> Result<Json<Value>> {
    Ok(Json(marks::create(&st, input, "user").await?))
}

pub async fn delete_mark(State(st): State<Shared>, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(json!({ "removed": marks::remove(&st, &[id])? })))
}

#[derive(Deserialize)]
pub struct ClearQuery {
    /// "pending" (default) or "history"
    which: Option<String>,
}

pub async fn clear_marks(State(st): State<Shared>, Query(q): Query<ClearQuery>) -> Result<Json<Value>> {
    let sql = if q.which.as_deref() == Some("history") {
        "DELETE FROM marks WHERE status <> 'pending'"
    } else {
        "DELETE FROM marks WHERE status = 'pending'"
    };
    Ok(Json(json!({ "removed": st.db(|c| Ok(c.execute(sql, [])?))? })))
}

#[derive(Deserialize)]
pub struct ApplyRequest {
    ids: Option<Vec<i64>>,
}

pub async fn apply(State(st): State<Shared>, Json(req): Json<ApplyRequest>) -> Result<Json<Value>> {
    Ok(Json(marks::apply(&st, req.ids).await?))
}
