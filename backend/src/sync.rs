//! Pulls the library from Radarr/Sonarr (and request info from Seerr) into SQLite.

use std::collections::{BTreeSet, HashMap};

use rusqlite::{named_params, Transaction};
use serde_json::{json, Value};

use crate::{
    arr::{image, Client},
    db,
    error::Result,
    AppState,
};

pub async fn sync_all(state: &AppState) -> Result<Value> {
    let _guard = state.work_lock.lock().await;
    sync_inner(state).await
}

/// Caller must hold `work_lock`.
pub async fn sync_inner(state: &AppState) -> Result<Value> {
    let radarr = Client::for_service(state, "radarr")?;
    let sonarr = Client::for_service(state, "sonarr")?;
    let seerr = Client::for_service(state, "seerr")?;

    let (movies, series, requests) = tokio::join!(
        async {
            match &radarr {
                Some(c) => Some(c.get("/movie", &[]).await),
                None => None,
            }
        },
        async {
            match &sonarr {
                Some(c) => Some(c.get("/series", &[]).await),
                None => None,
            }
        },
        async {
            match &seerr {
                Some(c) => Some(seerr_requests(c).await),
                None => None,
            }
        },
    );

    let mut errors: Vec<String> = Vec::new();
    let mut unwrap = |r: Option<Result<Value>>| match r {
        Some(Ok(v)) => Some(v),
        Some(Err(e)) => {
            errors.push(e.message);
            None
        }
        None => None,
    };
    let movies = unwrap(movies);
    let series = unwrap(series);
    let requests = match requests {
        Some(Ok(v)) => Some(v),
        Some(Err(e)) => {
            errors.push(e.message);
            None
        }
        None => None,
    };

    let movie_count = movies.as_ref().and_then(Value::as_array).map(Vec::len);
    let series_count = series.as_ref().and_then(Value::as_array).map(Vec::len);

    state.db(|conn| {
        let tx = conn.transaction()?;
        let now: String = tx.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| r.get(0))?;

        if let Some(list) = movies.as_ref().and_then(Value::as_array) {
            for m in list {
                upsert(&tx, &movie_row(m), &now)?;
            }
        }
        // A kind is replaced wholesale when its service answered, and dropped when the service is unconfigured.
        if movies.is_some() || radarr.is_none() {
            tx.execute("DELETE FROM media WHERE kind = 'movie' AND synced_at <> ?1", [&now])?;
        }

        if let Some(list) = series.as_ref().and_then(Value::as_array) {
            for s in list {
                let id = upsert(&tx, &series_row(s), &now)?;
                tx.execute("DELETE FROM seasons WHERE media_id = ?1", [id])?;
                for season in s["seasons"].as_array().into_iter().flatten() {
                    let st = &season["statistics"];
                    tx.execute(
                        "INSERT OR REPLACE INTO seasons
                           (media_id, season_number, episode_file_count, episode_count, total_episode_count, size_on_disk, monitored)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![
                            id,
                            season["seasonNumber"].as_i64().unwrap_or(0),
                            st["episodeFileCount"].as_i64().unwrap_or(0),
                            st["episodeCount"].as_i64().unwrap_or(0),
                            st["totalEpisodeCount"].as_i64().unwrap_or(0),
                            st["sizeOnDisk"].as_i64().unwrap_or(0),
                            season["monitored"].as_bool().unwrap_or(false),
                        ],
                    )?;
                }
            }
        }
        if series.is_some() || sonarr.is_none() {
            tx.execute("DELETE FROM media WHERE kind = 'series' AND synced_at <> ?1", [&now])?;
        }

        // Pending marks whose item disappeared can never be applied.
        tx.execute("DELETE FROM marks WHERE status = 'pending' AND media_id IS NULL", [])?;

        if let Some(map) = &requests {
            tx.execute("UPDATE media SET requested_by = NULL, seerr_media_id = NULL", [])?;
            for ((kind, id_column, external_id), (seerr_media_id, names)) in map {
                tx.execute(
                    &format!(
                        "UPDATE media SET requested_by = ?1, seerr_media_id = ?2
                         WHERE kind = ?3 AND {id_column} = ?4 AND requested_by IS NULL"
                    ),
                    rusqlite::params![
                        names.iter().cloned().collect::<Vec<_>>().join(", "),
                        seerr_media_id,
                        kind,
                        external_id
                    ],
                )?;
            }
        }

        db::set_setting(&tx, "last_sync", Some(&now))?;
        tx.commit()?;
        Ok(())
    })?;

    Ok(json!({
        "movies": movie_count,
        "series": series_count,
        "seerr_requests": requests.as_ref().map(HashMap::len),
        "errors": errors,
    }))
}

struct MediaRow {
    kind: &'static str,
    arr_id: i64,
    title: String,
    sort_title: String,
    year: Option<i64>,
    overview: Option<String>,
    poster_url: Option<String>,
    fanart_url: Option<String>,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
    size_on_disk: i64,
    file_count: i64,
    episode_count: i64,
    season_count: i64,
    status: Option<String>,
    added_at: Option<String>,
    genres: String,
    rating: Option<f64>,
    path: Option<String>,
    quality: Option<String>,
    monitored: bool,
    runtime: Option<i64>,
    network: Option<String>,
    file_id: Option<i64>,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn positive(v: &Value) -> Option<i64> {
    v.as_i64().filter(|n| *n > 0)
}

fn common(kind: &'static str, v: &Value) -> MediaRow {
    let title = text(&v["title"]).unwrap_or_else(|| "Untitled".into());
    MediaRow {
        kind,
        arr_id: v["id"].as_i64().unwrap_or(0),
        sort_title: text(&v["sortTitle"]).unwrap_or_else(|| title.to_lowercase()),
        title,
        year: positive(&v["year"]),
        overview: text(&v["overview"]),
        poster_url: image(v, "poster"),
        fanart_url: image(v, "fanart"),
        tmdb_id: positive(&v["tmdbId"]),
        tvdb_id: positive(&v["tvdbId"]),
        imdb_id: text(&v["imdbId"]),
        size_on_disk: 0,
        file_count: 0,
        episode_count: 0,
        season_count: 0,
        status: text(&v["status"]),
        added_at: text(&v["added"]),
        genres: v["genres"].as_array().map(|_| v["genres"].to_string()).unwrap_or_else(|| "[]".into()),
        rating: None,
        path: text(&v["path"]),
        quality: None,
        monitored: v["monitored"].as_bool().unwrap_or(false),
        runtime: positive(&v["runtime"]),
        network: None,
        file_id: None,
    }
}

fn movie_row(m: &Value) -> MediaRow {
    let has_file = m["hasFile"].as_bool().unwrap_or(false);
    MediaRow {
        size_on_disk: m["sizeOnDisk"].as_i64().or(m["movieFile"]["size"].as_i64()).unwrap_or(0),
        file_count: has_file as i64,
        rating: m["ratings"]["imdb"]["value"]
            .as_f64()
            .or(m["ratings"]["tmdb"]["value"].as_f64())
            .filter(|r| *r > 0.0),
        quality: text(&m["movieFile"]["quality"]["quality"]["name"]),
        network: text(&m["studio"]),
        file_id: positive(&m["movieFile"]["id"]).or(positive(&m["movieFileId"])),
        ..common("movie", m)
    }
}

fn series_row(s: &Value) -> MediaRow {
    let st = &s["statistics"];
    MediaRow {
        size_on_disk: st["sizeOnDisk"].as_i64().unwrap_or(0),
        file_count: st["episodeFileCount"].as_i64().unwrap_or(0),
        episode_count: st["episodeCount"].as_i64().unwrap_or(0),
        season_count: st["seasonCount"].as_i64().unwrap_or(0),
        rating: s["ratings"]["value"].as_f64().filter(|r| *r > 0.0),
        network: text(&s["network"]),
        ..common("series", s)
    }
}

/// Insert or update by (kind, arr_id), keeping our own id stable so marks stay attached.
fn upsert(tx: &Transaction, m: &MediaRow, now: &str) -> rusqlite::Result<i64> {
    tx.query_row(
        "INSERT INTO media
           (kind, arr_id, title, sort_title, year, overview, poster_url, fanart_url, tmdb_id, tvdb_id, imdb_id,
            size_on_disk, file_count, episode_count, season_count, status, added_at, genres, rating, path,
            quality, monitored, runtime, network, file_id, synced_at)
         VALUES
           (:kind, :arr_id, :title, :sort_title, :year, :overview, :poster_url, :fanart_url, :tmdb_id, :tvdb_id, :imdb_id,
            :size_on_disk, :file_count, :episode_count, :season_count, :status, :added_at, :genres, :rating, :path,
            :quality, :monitored, :runtime, :network, :file_id, :synced_at)
         ON CONFLICT (kind, arr_id) DO UPDATE SET
            title = excluded.title, sort_title = excluded.sort_title, year = excluded.year,
            overview = excluded.overview, poster_url = excluded.poster_url, fanart_url = excluded.fanart_url,
            tmdb_id = excluded.tmdb_id, tvdb_id = excluded.tvdb_id, imdb_id = excluded.imdb_id,
            size_on_disk = excluded.size_on_disk, file_count = excluded.file_count,
            episode_count = excluded.episode_count, season_count = excluded.season_count,
            status = excluded.status, added_at = excluded.added_at, genres = excluded.genres,
            rating = excluded.rating, path = excluded.path, quality = excluded.quality,
            monitored = excluded.monitored, runtime = excluded.runtime, network = excluded.network,
            file_id = excluded.file_id, synced_at = excluded.synced_at
         RETURNING id",
        named_params! {
            ":kind": m.kind, ":arr_id": m.arr_id, ":title": m.title, ":sort_title": m.sort_title,
            ":year": m.year, ":overview": m.overview, ":poster_url": m.poster_url, ":fanart_url": m.fanart_url,
            ":tmdb_id": m.tmdb_id, ":tvdb_id": m.tvdb_id, ":imdb_id": m.imdb_id,
            ":size_on_disk": m.size_on_disk, ":file_count": m.file_count, ":episode_count": m.episode_count,
            ":season_count": m.season_count, ":status": m.status, ":added_at": m.added_at, ":genres": m.genres,
            ":rating": m.rating, ":path": m.path, ":quality": m.quality, ":monitored": m.monitored,
            ":runtime": m.runtime, ":network": m.network, ":file_id": m.file_id, ":synced_at": now,
        },
        |r| r.get(0),
    )
}

/// (kind, id column, external id) -> (Seerr media id, requester names)
type RequestMap = HashMap<(&'static str, &'static str, i64), (i64, BTreeSet<String>)>;

async fn seerr_requests(client: &Client) -> Result<RequestMap> {
    const TAKE: usize = 100;
    let mut map: RequestMap = HashMap::new();
    for page in 0..200 {
        let res = client
            .get(
                "/request",
                &[("take", TAKE.to_string()), ("skip", (page * TAKE).to_string()), ("filter", "all".into())],
            )
            .await?;
        let results = res["results"].as_array().cloned().unwrap_or_default();
        for r in &results {
            let media = &r["media"];
            let Some(seerr_media_id) = media["id"].as_i64() else { continue };
            let user = &r["requestedBy"];
            let name = text(&user["displayName"])
                .or_else(|| text(&user["username"]))
                .or_else(|| text(&user["plexUsername"]))
                .or_else(|| text(&user["email"]));
            let mut keys: Vec<(&'static str, &'static str, i64)> = Vec::new();
            if media["mediaType"] == "movie" {
                if let Some(id) = positive(&media["tmdbId"]) {
                    keys.push(("movie", "tmdb_id", id));
                }
            } else {
                if let Some(id) = positive(&media["tvdbId"]) {
                    keys.push(("series", "tvdb_id", id));
                }
                if let Some(id) = positive(&media["tmdbId"]) {
                    keys.push(("series", "tmdb_id", id));
                }
            }
            for key in keys {
                let entry = map.entry(key).or_insert_with(|| (seerr_media_id, BTreeSet::new()));
                if let Some(name) = &name {
                    entry.1.insert(name.clone());
                }
            }
        }
        if results.len() < TAKE {
            break;
        }
    }
    Ok(map)
}
