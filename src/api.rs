//! Thin HTTP client over Eight Sleep's two API hosts.

use anyhow::{Context, Result, bail};
use reqwest::Method;
use reqwest::blocking::Client;
use serde_json::Value;

use std::sync::Mutex;

use crate::auth;

pub const CLIENT_API: &str = "https://client-api.8slp.net/v1";
pub const APP_API: &str = "https://app-api.8slp.net/v1";
pub const APP_API_V2: &str = "https://app-api.8slp.net/v2";

/// Shared across threads so independent reads can run concurrently.
pub struct Api {
    http: Client,
    user_id: String,
    access_token: Mutex<String>,
}

impl Api {
    pub fn new() -> Result<Self> {
        // No default content-type: the API edge answers 403 to bodyless GETs
        // that carry one. `.json()` adds it to requests with a body.
        let http = Client::builder()
            .user_agent(concat!("8sleep/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let token = auth::token(&http, false)?;
        Ok(Self {
            http,
            user_id: token.user_id,
            access_token: Mutex::new(token.access_token),
        })
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn get(&self, url: &str) -> Result<Value> {
        self.request(Method::GET, url, None)
    }

    pub fn put(&self, url: &str, body: Value) -> Result<Value> {
        self.request(Method::PUT, url, Some(body))
    }

    pub fn post(&self, url: &str, body: Value) -> Result<Value> {
        self.request(Method::POST, url, Some(body))
    }

    pub fn delete(&self, url: &str) -> Result<Value> {
        self.request(Method::DELETE, url, None)
    }

    fn token(&self) -> String {
        self.access_token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Refresh after a 401, unless another thread already replaced `stale`.
    fn refresh(&self, stale: &str) -> Result<()> {
        let mut cur = self.access_token.lock().unwrap_or_else(|e| e.into_inner());
        if *cur == stale {
            *cur = auth::token(&self.http, true)?.access_token;
        }
        Ok(())
    }

    fn request(&self, method: Method, url: &str, body: Option<Value>) -> Result<Value> {
        let mut retried = false;
        loop {
            let token = self.token();
            let mut req = self
                .http
                .request(method.clone(), url)
                .bearer_auth(&token)
                .header("accept", "application/json");
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req.send().with_context(|| format!("{method} {url}"))?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && !retried {
                self.refresh(&token)?;
                retried = true;
                continue;
            }
            let text = resp.text()?;
            if !status.is_success() {
                let msg = server_message(&text);
                if msg.is_empty() {
                    bail!("{status} ({method} {url})");
                }
                bail!("{msg} ({status}, {method} {url})");
            }
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            return serde_json::from_str(&text)
                .with_context(|| format!("{method} {url}: response is not JSON"));
        }
    }
}

/// The human part of an error body: a bare JSON string, `message`, or a
/// validation `title` with its field errors. Falls back to the raw text.
fn server_message(text: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return text.trim().to_string();
    };
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(m) = v["message"].as_str() {
        return m.to_string();
    }
    if let Some(t) = v["title"].as_str() {
        let fields: Vec<String> = v["errors"]
            .as_object()
            .into_iter()
            .flatten()
            .flat_map(|(_, msgs)| msgs.as_array().cloned().unwrap_or_default())
            .filter_map(|m| m.as_str().map(str::to_owned))
            .collect();
        return if fields.is_empty() {
            t.to_string()
        } else {
            format!("{t} {}", fields.join(" "))
        };
    }
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::server_message;

    #[test]
    fn extracts_server_messages() {
        assert_eq!(
            server_message(r#""Bedtime schedules must be at least 16 hours apart.""#),
            "Bedtime schedules must be at least 16 hours apart."
        );
        assert_eq!(server_message(r#"{"message":"nope"}"#), "nope");
        assert_eq!(
            server_message(
                r#"{"title":"One or more validation errors occurred.","errors":{"Schedules":["The Schedules field is required."]}}"#
            ),
            "One or more validation errors occurred. The Schedules field is required."
        );
        assert_eq!(server_message("<html>404</html>"), "<html>404</html>");
    }
}
