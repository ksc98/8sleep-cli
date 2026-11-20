//! Thin HTTP client over Eight Sleep's two API hosts.

use anyhow::{Context, Result, bail};
use reqwest::Method;
use reqwest::blocking::Client;
use serde_json::Value;

use crate::auth::{self, Token};

pub const CLIENT_API: &str = "https://client-api.8slp.net/v1";
pub const APP_API: &str = "https://app-api.8slp.net/v1";

pub struct Api {
    http: Client,
    token: Token,
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
        Ok(Self { http, token })
    }

    pub fn user_id(&self) -> &str {
        &self.token.user_id
    }

    pub fn get(&mut self, url: &str) -> Result<Value> {
        self.request(Method::GET, url, None)
    }

    pub fn put(&mut self, url: &str, body: Value) -> Result<Value> {
        self.request(Method::PUT, url, Some(body))
    }

    pub fn post(&mut self, url: &str, body: Value) -> Result<Value> {
        self.request(Method::POST, url, Some(body))
    }

    fn request(&mut self, method: Method, url: &str, body: Option<Value>) -> Result<Value> {
        let mut retried = false;
        loop {
            let mut req = self
                .http
                .request(method.clone(), url)
                .bearer_auth(&self.token.access_token)
                .header("accept", "application/json");
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req.send().with_context(|| format!("{method} {url}"))?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && !retried {
                self.token = auth::token(&self.http, true)?;
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
