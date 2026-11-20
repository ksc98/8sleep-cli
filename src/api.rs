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
                bail!("{method} {url} failed ({status}): {text}");
            }
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            return serde_json::from_str(&text)
                .with_context(|| format!("{method} {url}: response is not JSON"));
        }
    }
}
