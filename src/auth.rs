//! Login, token refresh and the on-disk token cache.
//!
//! Credentials come from `EIGHTSLEEP_EMAIL` / `EIGHTSLEEP_PASSWORD`, the
//! config file's `email` and `password`, or a 1Password item. A password
//! login happens only when there is no cached refresh token or the refresh
//! grant fails.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const AUTH_URL: &str = "https://auth-api.8slp.net/v1/tokens";
// The Eight Sleep mobile app's OAuth client. The token endpoint only accepts
// password and refresh grants from known clients.
const CLIENT_ID: &str = "0894c7f33bb94800a03f1f4df13a4f38";
const CLIENT_SECRET: &str = "f0954a3ed5763ba3d06834c73731a32f15f168f47d4f164751275def86db0c76";
const EXPIRY_BUFFER_SECS: u64 = 120;

#[derive(Serialize, Deserialize, Clone)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub user_id: String,
}

impl Token {
    fn fresh(&self) -> bool {
        now() + EXPIRY_BUFFER_SECS < self.expires_at
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `~/.cache/8sleep/<name>`.
fn cache_path(name: &str) -> Result<PathBuf> {
    let dir = dirs::cache_dir().context("no cache directory")?;
    Ok(dir.join("8sleep").join(name))
}

fn load_cache() -> Option<Token> {
    let raw = fs::read(cache_path("token.json").ok()?).ok()?;
    serde_json::from_slice(&raw).ok()
}

pub fn save_cache(tok: &Token) -> Result<()> {
    write_private(&cache_path("token.json")?, &serde_json::to_vec(tok)?)
}

/// Write a file atomically with 0600 permissions.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("cache path has no parent")?;
    fs::create_dir_all(dir)?;
    let name = path.file_name().context("cache path has no file name")?;
    let tmp = dir.join(format!(
        ".{}.{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&tmp)
        .with_context(|| format!("writing {}", tmp.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Return a valid token, refreshing or logging in as needed.
pub fn token(http: &reqwest::blocking::Client, force_refresh: bool) -> Result<Token> {
    let cached = load_cache();
    if let Some(t) = &cached
        && !force_refresh
        && t.fresh()
    {
        return Ok(t.clone());
    }
    if let Some(t) = cached
        && let Ok(new) = refresh(http, &t)
    {
        save_cache(&new)?;
        return Ok(new);
    }
    let new = password_login(http)?;
    save_cache(&new)?;
    Ok(new)
}

fn refresh(http: &reqwest::blocking::Client, old: &Token) -> Result<Token> {
    let body = json!({
        "client_id": CLIENT_ID,
        "client_secret": CLIENT_SECRET,
        "grant_type": "refresh_token",
        "refresh_token": old.refresh_token,
    });
    // The refresh response omits userId, so carry it over.
    token_request(http, &body, Some(&old.user_id))
}

fn password_login(http: &reqwest::blocking::Client) -> Result<Token> {
    let (email, password) = credentials()?;
    login(http, &email, &password)
}

/// Sign in with an email and password. Nothing is cached.
pub fn login(http: &reqwest::blocking::Client, email: &str, password: &str) -> Result<Token> {
    let body = json!({
        "client_id": CLIENT_ID,
        "client_secret": CLIENT_SECRET,
        "grant_type": "password",
        "username": email,
        "password": password,
    });
    token_request(http, &body, None).map_err(|e| {
        if e.to_string().contains("invalid_grant") {
            anyhow!("wrong email or password")
        } else {
            e
        }
    })
}

fn token_request(
    http: &reqwest::blocking::Client,
    body: &Value,
    user_id: Option<&str>,
) -> Result<Token> {
    let resp = http
        .post(AUTH_URL)
        .json(body)
        .send()
        .context("auth request failed")?;
    let status = resp.status();
    let text = resp.text()?;
    if !status.is_success() {
        bail!("auth failed ({status}): {text}");
    }
    let v: Value = serde_json::from_str(&text).context("auth response is not JSON")?;
    let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let user_id = field("userId")
        .or_else(|| user_id.map(str::to_owned))
        .context("auth response has no userId")?;
    let expires_in = v
        .get("expires_in")
        .and_then(Value::as_f64)
        .context("no expires_in")?;
    Ok(Token {
        access_token: field("access_token").context("no access_token")?,
        refresh_token: field("refresh_token").context("no refresh_token")?,
        expires_at: now() + expires_in as u64,
        user_id,
    })
}

/// Email and password, each from the first of: the environment, the
/// config's `email` / `password`, or the 1Password item named by the
/// config's `onepassword` (its `username` and `password` fields).
fn credentials() -> Result<(String, String)> {
    let path = crate::config::path()?;
    let item = crate::config::get("onepassword")?;
    let missing = || {
        anyhow!(
            "no Eight Sleep credentials: run `8sleep setup`, or set `email` and `password`, or `onepassword = \"op://Vault/Item\"`, in {}",
            path.display()
        )
    };
    let field = |env: &str, key: &str, op_field: &str| -> Result<String> {
        if let Ok(v) = std::env::var(env) {
            return Ok(v);
        }
        if let Some(v) = crate::config::get(key)? {
            if key == "password" {
                warn_if_readable(&path);
            }
            return Ok(v);
        }
        match &item {
            Some(item) => op_read(&format!("{}/{op_field}", item.trim_end_matches('/'))),
            None => Err(missing()),
        }
    };
    Ok((
        field("EIGHTSLEEP_EMAIL", "email", "username")?,
        field("EIGHTSLEEP_PASSWORD", "password", "password")?,
    ))
}

/// `op read <reference>` with the 1Password CLI.
fn op_read(reference: &str) -> Result<String> {
    let out = Command::new("op")
        .args(["read", "--no-newline", reference])
        .output()
        .context("running `op`, the 1Password CLI (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "`op read {reference}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// A plaintext password belongs in a file only you can read.
fn warn_if_readable(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = fs::metadata(path)
            && m.permissions().mode() & 0o077 != 0
        {
            eprintln!(
                "warning: {} holds a password but other users can read it; run `chmod 600 {}`",
                path.display(),
                path.display()
            );
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}
