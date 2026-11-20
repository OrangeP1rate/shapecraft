//! The plugin list ("Browse"): `plugins-index.json`, built by CI from `plugins/` and published with each release,
//! next to the packages and covered by the release's `SHA256SUMS.txt`. Each ShapeCraft version reads the list made
//! for it, so the precompiled code always matches its runtime.

use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;

pub const REPO: &str = "jackharvest/shapecraft";
pub const INDEX: &str = "plugins-index.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Index {
    /// The ShapeCraft version it was built for.
    pub app: String,
    pub plugins: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub manifest: Manifest,
    /// "official" (made with ShapeCraft) or "community" (reviewed, made by others).
    pub tier: String,
    /// The package's file name among the release's files.
    pub file: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// Where this version's list and packages are (`SHAPECRAFT_PLUGIN_INDEX` may point at a folder, for testing).
pub fn base_url(app_version: &str) -> String {
    if let Ok(v) = std::env::var("SHAPECRAFT_PLUGIN_INDEX") {
        return v.trim_end_matches('/').to_string();
    }
    format!("https://github.com/{REPO}/releases/download/v{app_version}-alpha")
}

impl Index {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut idx: Index = serde_json::from_str(text).map_err(|e| format!("the plugin list couldn't be read ({e})"))?;
        idx.plugins.retain(|e| {
            crate::manifest::valid_id(&e.manifest.id)
                && e.sha256.len() == 64
                && e.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                && !e.file.contains(['/', '\\'])
        });
        Ok(idx)
    }
}

/// Gets the list (a local folder when the base is a path).
pub fn fetch(app_version: &str) -> Result<Index, String> {
    let base = base_url(app_version);
    let text = if base.starts_with("https://") {
        let a = crate::net::parse(&base)?;
        let ans = crate::net::fetch("GET", &format!("{base}/{INDEX}"), &[], Vec::new(), 30, &a.host)?;
        // GitHub sends release files through a redirect to its file host.
        let ans = follow(ans)?;
        match ans.status {
            200 => String::from_utf8(ans.body).map_err(|_| "the plugin list isn't text".to_string())?,
            404 => return Err("no plugins are listed for this version yet".into()),
            s => return Err(format!("GitHub answered {s}")),
        }
    } else {
        std::fs::read_to_string(std::path::Path::new(&base).join(INDEX)).map_err(|e| format!("{base}: {e}"))?
    };
    Index::parse(&text)
}

/// One redirect, only to GitHub's own hosts.
fn follow(ans: crate::net::Answer) -> Result<crate::net::Answer, String> {
    if !(300..400).contains(&ans.status) {
        return Ok(ans);
    }
    let to = ans
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("location"))
        .map(|(_, v)| v.clone())
        .ok_or("a redirect without an address")?;
    let a = crate::net::parse(&to)?;
    if !a.https || !(a.host == "github.com" || a.host.ends_with(".githubusercontent.com")) {
        return Err(format!("the download went somewhere unexpected ({})", a.host));
    }
    crate::net::fetch("GET", &to, &[], Vec::new(), 300, &a.host)
}

/// Downloads a listed package and checks it against the list's SHA-256.
pub fn download(app_version: &str, e: &Entry) -> Result<crate::library::Package, String> {
    let base = base_url(app_version);
    let bytes = if base.starts_with("https://") {
        let a = crate::net::parse(&base)?;
        let ans = follow(crate::net::fetch("GET", &format!("{base}/{}", e.file), &[], Vec::new(), 300, &a.host)?)?;
        if ans.status != 200 {
            return Err(format!("the download failed (GitHub answered {})", ans.status));
        }
        ans.body
    } else {
        std::fs::read(std::path::Path::new(&base).join(&e.file)).map_err(|err| err.to_string())?
    };
    let got = {
        use sha2::Digest;
        sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    if got != e.sha256.to_ascii_lowercase() {
        return Err("the download doesn't match the plugin list (checksum differs); nothing was installed".into());
    }
    let pkg = crate::library::Package::read(&bytes).map_err(|err| err.to_string())?;
    if pkg.manifest.id != e.manifest.id || pkg.manifest.permissions != e.manifest.permissions {
        return Err("the package doesn't match its listing; nothing was installed".into());
    }
    Ok(pkg)
}
