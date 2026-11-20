//! Pictures plugins make, kept in the plugin's `results/` folder (PNG + a small JSON beside it) so nothing is lost
//! when the panel closes or the app quits.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::images::{self, Picture};
use crate::store::write_file;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The PNG.
    #[serde(skip)]
    pub path: PathBuf,
    pub title: String,
    pub details: String,
    /// Milliseconds since 1970.
    pub time: u64,
    pub width: u32,
    pub height: u32,
}

fn folder(dir: &Path) -> PathBuf {
    dir.join("results")
}

pub fn save(dir: &Path, picture: &Picture, title: &str, details: &str) -> Result<Entry, String> {
    let png = images::encode_png(picture)?;
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let f = folder(dir);
    // Unique even for several in the same millisecond.
    let mut n = 0;
    let path = loop {
        let p = f.join(format!("{time}-{n}.png"));
        if !p.exists() {
            break p;
        }
        n += 1;
    };
    let entry = Entry {
        path: path.clone(),
        title: title.chars().take(2000).collect(),
        details: details.chars().take(4000).collect(),
        time,
        width: picture.width,
        height: picture.height,
    };
    write_file(&path, &png, false)?;
    let meta = serde_json::to_string_pretty(&entry).map_err(|e| e.to_string())?;
    write_file(&path.with_extension("json"), meta.as_bytes(), false)?;
    // Nothing is removed on its own: these may have cost money. People delete them in History.
    Ok(entry)
}

/// Everything kept, newest first.
pub fn list(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(folder(dir)) else { return Vec::new() };
    let mut out: Vec<Entry> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let mut e: Entry = serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
            e.path = p.with_extension("png");
            e.path.exists().then_some(e)
        })
        .collect();
    out.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| b.path.cmp(&a.path)));
    out
}

pub fn remove(entry: &Entry) {
    let _ = std::fs::remove_file(&entry.path);
    let _ = std::fs::remove_file(entry.path.with_extension("json"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_and_lists_results() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("sc-plugin-results-{}", std::process::id()));
        let p = Picture { width: 2, height: 2, pixels: vec![255; 16] };
        save(&dir, &p, "First", "prompt: a desk")?;
        save(&dir, &p, "Second", "")?;
        let l = list(&dir);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].title, "Second");
        remove(&l[0]);
        assert_eq!(list(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
