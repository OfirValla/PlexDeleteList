//! Read-only HTTP client for Radarr, Sonarr and Seerr (Overseerr/Jellyseerr). All three accept `X-Api-Key`.
//!
//! Only GET exists here, on purpose: this app collects a list of what should be deleted and must never be
//! able to delete or change anything in those services itself.

use serde_json::Value;

use crate::{
    db,
    error::{AppError, Result},
    AppState,
};

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    key: String,
    label: &'static str,
}

fn label(name: &str) -> &'static str {
    match name {
        "radarr" => "Radarr",
        "sonarr" => "Sonarr",
        _ => "Seerr",
    }
}

impl Client {
    pub fn for_service(state: &AppState, name: &str) -> Result<Option<Client>> {
        let cfg = state.db(|c| Ok(db::service(c, name)?))?;
        Ok(cfg.map(|(url, key)| Client {
            http: state.http.clone(),
            base: format!(
                "{}/api/{}",
                url.trim().trim_end_matches('/'),
                if name == "seerr" { "v1" } else { "v3" }
            ),
            key,
            label: label(name),
        }))
    }

    pub fn require(state: &AppState, name: &str) -> Result<Client> {
        Self::for_service(state, name)?
            .ok_or_else(|| AppError::bad_request(format!("{} is not configured", label(name))))
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let res = self
            .http
            .get(format!("{}{}", self.base, path))
            .header("X-Api-Key", &self.key)
            .query(query)
            .send()
            .await
            .map_err(|e| AppError::upstream(format!("{}: {}", self.label, e.without_url())))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            let snippet: String = text.chars().take(300).collect();
            return Err(AppError::upstream(format!(
                "{} GET {path} returned {status}: {snippet}",
                self.label
            )));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }
}

/// Remote (TMDB/TVDB) URL of an image of the given cover type, as reported by Radarr/Sonarr.
pub fn image(item: &Value, cover_type: &str) -> Option<String> {
    item["images"]
        .as_array()?
        .iter()
        .find(|i| i["coverType"] == cover_type)
        .and_then(|i| i["remoteUrl"].as_str())
        .filter(|u| u.starts_with("http"))
        .map(str::to_string)
}
