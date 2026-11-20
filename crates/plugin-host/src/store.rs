//! Where plugins live on disk, and each plugin's settings and secrets.
//!
//! `<config>/ShapeCraft/plugins/<id>/`: `plugin.toml`, `plugin.cwasm` (the precompiled component), `settings.json`,
//! `secrets.json` (readable by this user only), `results/` (pictures the plugin made). Secrets are kept in a file
//! like the report sign-in rather than the system keychain: the app is unsigned, so macOS would ask for the
//! keychain password again after every update.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The folder all plugins live in (`SHAPECRAFT_PLUGINS` overrides it, for tests and scratch runs).
pub fn root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SHAPECRAFT_PLUGINS") {
        return Some(PathBuf::from(p));
    }
    Some(dirs::config_dir()?.join("ShapeCraft").join("plugins"))
}

/// A JSON object of strings kept in one file, written whole each time (small: a few keys).
#[derive(Debug)]
pub struct KeyFile {
    path: PathBuf,
    private: bool,
}

impl KeyFile {
    pub fn settings(dir: &Path) -> Self {
        Self { path: dir.join("settings.json"), private: false }
    }

    pub fn secrets(dir: &Path) -> Self {
        Self { path: dir.join("secrets.json"), private: true }
    }

    pub fn read_all(&self) -> BTreeMap<String, String> {
        let Ok(text) = std::fs::read_to_string(&self.path) else { return BTreeMap::new() };
        match serde_json::from_str::<BTreeMap<String, Value>>(&text) {
            Ok(m) => m.into_iter().filter_map(|(k, v)| Some((k, v.as_str()?.to_string()))).collect(),
            Err(_) => BTreeMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.read_all().remove(key)
    }

    pub fn set(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        if key.is_empty() || key.len() > 200 {
            return Err("names are 1-200 characters".into());
        }
        if value.is_some_and(|v| v.len() > 1 << 20) {
            return Err("that value is too big to keep (1 MB at most)".into());
        }
        let mut all = self.read_all();
        match value {
            Some(v) => {
                all.insert(key.to_string(), v.to_string());
            }
            None => {
                all.remove(key);
            }
        }
        if all.len() > 500 {
            return Err("too many saved values".into());
        }
        let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
        write_file(&self.path, text.as_bytes(), self.private)
    }
}

/// Writes through a `.part` file so a crash never leaves half a file; `private` makes it readable by this user only.
pub fn write_file(path: &Path, bytes: &[u8], private: bool) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = path.with_extension("part");
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut f = opts.open(&part).map_err(|e| e.to_string())?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&part, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_values() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("sc-plugin-store-{}", std::process::id()));
        let s = KeyFile::secrets(&dir);
        assert_eq!(s.get("key"), None);
        s.set("key", Some("sk-123"))?;
        s.set("other", Some("x"))?;
        assert_eq!(s.get("key").as_deref(), Some("sk-123"));
        s.set("key", None)?;
        assert_eq!(s.get("key"), None);
        assert_eq!(s.get("other").as_deref(), Some("x"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("secrets.json")).map_err(|e| e.to_string())?.permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
