//! `~/.config/8sleep/config.toml` (or `$XDG_CONFIG_HOME/8sleep/config.toml`).
//!
//! ```toml
//! email = "you@example.com"
//! password = "..."
//! # or, instead of email and password, a 1Password item:
//! # onepassword = "op://Personal/EightSleep"
//! device_id = "..."   # the Pod; filled in by `8sleep setup` or the first run
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
    edit(&[(key, Some(value))])
}

/// Set (`Some`) or remove (`None`) several settings in one write.
pub fn edit(changes: &[(&str, Option<&str>)]) -> Result<()> {
    let mut doc = load()?;
    for (key, value) in changes {
        match value {
            Some(v) => doc[*key] = toml_edit::value(*v),
            None => remove(&mut doc, key),
        }
    }
    let path = path()?;
    crate::auth::write_private(&path, doc.to_string().as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

/// Remove a key, handing the comments above it to the key that follows.
fn remove(doc: &mut DocumentMut, key: &str) {
    let table = doc.as_table_mut();
    let prefix = table
        .key(key)
        .and_then(|k| k.leaf_decor().prefix())
        .and_then(|p| p.as_str())
        .map(str::to_owned);
    let next = table
        .iter()
        .map(|(k, _)| k.to_owned())
        .skip_while(|k| k != key)
        .nth(1);
    table.remove(key);
    if let (Some(prefix), Some(next)) = (prefix, next)
        && let Some(mut k) = table.key_mut(&next)
    {
        let own = k
            .leaf_decor()
            .prefix()
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_owned();
        k.leaf_decor_mut().set_prefix(format!("{prefix}{own}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_a_key_keeps_its_comment() {
        let mut doc: DocumentMut = "# mine\nonepassword = \"op://x\"\ndevice_id = \"d\"\n"
            .parse()
            .unwrap();
        remove(&mut doc, "onepassword");
        doc["email"] = toml_edit::value("e");
        assert_eq!(
            doc.to_string(),
            "# mine\ndevice_id = \"d\"\nemail = \"e\"\n"
        );
    }
}
