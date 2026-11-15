mod api;
mod arr;
mod db;
mod error;
mod marks;
mod mcp;
mod sync;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::Connection;
use serde_json::json;
use tower_http::{
    compression::CompressionLayer,
    services::{ServeDir, ServeFile},
};

pub struct AppState {
    db: Mutex<Connection>,
    pub http: reqwest::Client,
    pub api_key: Option<String>,
    /// One sync at a time.
    pub sync_lock: tokio::sync::Mutex<()>,
}

impl AppState {
    /// Run a closure with the database. The guard never escapes, so it can't be held across an await.
    pub fn db<T>(&self, f: impl FnOnce(&mut Connection) -> error::Result<T>) -> error::Result<T> {
        let mut guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }
}

pub type Shared = Arc<AppState>;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

const USAGE: &str = "plex-delete-list — collect a list of what to delete from a Plex library.
It only reads from Radarr/Sonarr/Seerr and never deletes or changes anything there.

USAGE:
  plex-delete-list [serve]          Run the web server (default)
  plex-delete-list mcp [--url URL]  Run a stdio MCP server that bridges to a running web server

ENVIRONMENT (serve):
  PDL_BIND                   Listen address              (default 0.0.0.0:8080)
  PDL_DB                     SQLite database path        (default ./data/pdl.db)
  PDL_STATIC_DIR             Built frontend directory    (default ./frontend/dist)
  PDL_API_KEY                If set, /api and /mcp require this key
  PDL_SYNC_INTERVAL_MINUTES  Background sync interval, 0 disables (default 60)
  RADARR_URL, RADARR_API_KEY, SONARR_URL, SONARR_API_KEY, SEERR_URL, SEERR_API_KEY
                             Defaults for the settings page

ENVIRONMENT (mcp):
  PDL_URL                    Web server address          (default http://localhost:8080)
  PDL_API_KEY                Key to send, if the server requires one";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("mcp") => {
            let url = args
                .iter()
                .position(|a| a == "--url")
                .and_then(|i| args.get(i + 1).cloned())
                .unwrap_or_else(|| env_or("PDL_URL", "http://localhost:8080"));
            let key = std::env::var("PDL_API_KEY").ok().filter(|k| !k.is_empty());
            return mcp::stdio_proxy(url, key).await;
        }
        Some("serve") | None => {}
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(other) => {
            eprintln!("Unknown command '{other}'\n\n{USAGE}");
            std::process::exit(2);
        }
    }

    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let db_path = env_or("PDL_DB", "./data/pdl.db");
    let conn = db::open(&db_path)?;
    let state: Shared = Arc::new(AppState {
        db: Mutex::new(conn),
        http: reqwest::Client::builder().timeout(Duration::from_secs(120)).build()?,
        api_key: std::env::var("PDL_API_KEY").ok().filter(|k| !k.is_empty()),
        sync_lock: tokio::sync::Mutex::new(()),
    });

    let interval: u64 = env_or("PDL_SYNC_INTERVAL_MINUTES", "60").parse().unwrap_or(60);
    let bg = state.clone();
    tokio::spawn(async move {
        loop {
            let configured = bg
                .db(|c| Ok(db::service(c, "radarr")?.is_some() || db::service(c, "sonarr")?.is_some()))
                .unwrap_or(false);
            if configured {
                match sync::sync_all(&bg).await {
                    Ok(report) => tracing::info!("background sync: {report}"),
                    Err(e) => tracing::warn!("background sync failed: {e}"),
                }
            }
            if interval == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_secs(interval * 60)).await;
        }
    });

    let static_dir = PathBuf::from(env_or("PDL_STATIC_DIR", "./frontend/dist"));
    let spa = ServeDir::new(&static_dir).fallback(ServeFile::new(static_dir.join("index.html")));

    let app = Router::new()
        .route("/api/status", get(api::status))
        .route("/api/settings", get(api::get_settings).put(api::put_settings))
        .route("/api/settings/test/{service}", post(api::test_service))
        .route("/api/sync", post(api::sync))
        .route("/api/media", get(api::list_media))
        .route("/api/media/{id}", get(api::get_media))
        .route("/api/media/{id}/episodes", get(api::episodes))
        .route("/api/marks", get(api::list_marks).post(api::create_mark).delete(api::clear_marks))
        .route("/api/marks/{id}", delete(api::delete_mark))
        .route("/mcp", post(mcp::http))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state.clone())
        .fallback_service(spa)
        // gzip the library JSON and the frontend bundle; both shrink to under a third.
        .layer(CompressionLayer::new());

    let bind = env_or("PDL_BIND", "0.0.0.0:8080");
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(
        "listening on http://{bind} (db: {db_path}, api key {})",
        if state.api_key.is_some() { "required" } else { "not set" }
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn auth(State(state): State<Shared>, req: Request, next: Next) -> Response {
    if let Some(expected) = &state.api_key {
        let headers = req.headers();
        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let x_key = headers.get("x-api-key").and_then(|v| v.to_str().ok());
        if bearer != Some(expected.as_str()) && x_key != Some(expected.as_str()) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Missing or invalid API key" })),
            )
                .into_response();
        }
    }
    next.run(req).await
}
