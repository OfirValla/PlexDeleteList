//! Thin HTTP client for Radarr, Sonarr and Seerr (Overseerr/Jellyseerr). All three accept `X-Api-Key`.

use reqwest::Method;
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

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut req = self
            .http
            .request(method.clone(), format!("{}{}", self.base, path))
            .header("X-Api-Key", &self.key)
            .query(query);
        if let Some(body) = body {
            req = req.json(body);
        }
        let res = req
            .send()
            .await
            .map_err(|e| AppError::upstream(format!("{}: {}", self.label, e.without_url())))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            let snippet: String = text.chars().take(300).collect();
            return Err(AppError::upstream(format!(
                "{} {method} {path} returned {status}: {snippet}",
                self.label
            )));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.request(Method::GET, path, query, None).await
    }
    pub async fn delete(&self, path: &str, query: &[(&str, String)], body: Option<&Value>) -> Result<Value> {
        self.request(Method::DELETE, path, query, body).await
    }
    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.request(Method::POST, path, &[], Some(body)).await
    }
    pub async fn put(&self, path: &str, body: &Value) -> Result<Value> {
        self.request(Method::PUT, path, &[], Some(body)).await
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
