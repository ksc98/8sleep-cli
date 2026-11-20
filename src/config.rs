//! `~/.config/8sleep/config.toml` (or `$XDG_CONFIG_HOME/8sleep/config.toml`).
//!
//! ```toml
//! device_id = "793e54a4..."   # the Pod; filled in on first run
//! ```
//!
//! Edits keep the file's comments and other keys (`toml_edit`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use toml_edit::DocumentMut;

pub fn path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => dirs::home_dir()
            .context("no home directory")?
            .join(".config"),
    };
    Ok(base.join("8sleep").join("config.toml"))
}

fn load() -> Result<DocumentMut> {
    let path = path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => text
            .parse()
            .with_context(|| format!("{} is not valid TOML", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// A string setting, if present.
pub fn get(key: &str) -> Result<Option<String>> {
    Ok(load()?.get(key).and_then(|v| v.as_str()).map(str::to_owned))
}

/// Set a string setting, creating the file if needed.
pub fn set(key: &str, value: &str) -> Result<()> {
    let mut doc = load()?;
    doc[key] = toml_edit::value(value);
    let path = path()?;
    crate::auth::write_private(&path, doc.to_string().as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}
