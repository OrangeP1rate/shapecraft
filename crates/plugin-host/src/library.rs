//! Installed plugins: adding one from a package, removing it, turning it on and off, and starting the ones that are on.
//!
//! A package (`<id>-<version>.scplugin`) is a zip with `plugin.toml` and `plugin.cwasm` (and optionally `README.md`).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::manifest::Manifest;
use crate::runtime::{self, Activity};
use crate::store::write_file;
use crate::worker::Loaded;
use crate::{Error, Result};

/// The biggest package accepted.
pub const MAX_PACKAGE: u64 = 64 << 20;

/// What's inside a package, read and checked but not yet installed.
pub struct Package {
    pub manifest: Manifest,
    pub cwasm: Vec<u8>,
    pub readme: Option<String>,
}

impl Package {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_PACKAGE {
            return Err(Error::Package("the package is too big".into()));
        }
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| Error::Package(e.to_string()))?;
        let mut file = |name: &str| -> Result<Option<Vec<u8>>> {
            let Ok(f) = zip.by_name(name) else { return Ok(None) };
            let mut v = Vec::new();
            f.take(MAX_PACKAGE).read_to_end(&mut v).map_err(|e| Error::Package(e.to_string()))?;
            Ok(Some(v))
        };
        let toml = file("plugin.toml")?.ok_or_else(|| Error::Package("no plugin.toml".into()))?;
        let cwasm = file("plugin.cwasm")?.ok_or_else(|| Error::Package("no plugin.cwasm".into()))?;
        let readme = file("README.md")?.and_then(|b| String::from_utf8(b).ok());
        let manifest = Manifest::parse(std::str::from_utf8(&toml).map_err(|_| Error::Package("plugin.toml isn't UTF-8".into()))?)?;
        Ok(Self { manifest, cwasm, readme })
    }

    /// Writes a package (used by `cargo xtask plugin` and tests).
    pub fn write(manifest_toml: &str, cwasm: &[u8], readme: Option<&str>) -> Result<Vec<u8>> {
        use std::io::Write;
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut put = |name: &str, data: &[u8]| -> Result<()> {
            out.start_file(name, opts).map_err(|e| Error::Package(e.to_string()))?;
            out.write_all(data).map_err(|e| Error::Package(e.to_string()))
        };
        put("plugin.toml", manifest_toml.as_bytes())?;
        put("plugin.cwasm", cwasm)?;
        if let Some(r) = readme {
            put("README.md", r.as_bytes())?;
        }
        Ok(out.finish().map_err(|e| Error::Package(e.to_string()))?.into_inner())
    }
}

/// Where installed plugins are, and which are switched off.
pub struct Library {
    pub root: PathBuf,
}

/// An installed plugin as listed in Preferences ▸ Plugins.
#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub enabled: bool,
    /// From a file rather than the registry (shown as "Added from a file").
    pub sideloaded: bool,
}

impl Library {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn open() -> Option<Self> {
        Some(Self::new(crate::store::root()?))
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn list(&self) -> Vec<Installed> {
        let Ok(rd) = std::fs::read_dir(&self.root) else { return Vec::new() };
        let mut out: Vec<Installed> = rd
            .flatten()
            .filter_map(|e| {
                let dir = e.path();
                let manifest = Manifest::parse(&std::fs::read_to_string(dir.join("plugin.toml")).ok()?).ok()?;
                let enabled = !dir.join("disabled").exists();
                let sideloaded = dir.join("sideloaded").exists();
                Some(Installed { manifest, dir, enabled, sideloaded })
            })
            .collect();
        out.sort_by_key(|a| a.manifest.name.to_lowercase());
        out
    }

    /// Installs (or updates) a checked package. The plugin's settings, secrets and results are kept on update.
    /// The code is test-loaded first, so a package this app can't run is refused before anything changes.
    pub fn install(&self, pkg: &Package, sideloaded: bool) -> Result<Installed> {
        runtime::load(&pkg.cwasm)?;
        let dir = self.dir(&pkg.manifest.id);
        std::fs::create_dir_all(&dir).map_err(|e| Error::Io(e.to_string()))?;
        let toml = toml::to_string(&pkg.manifest).map_err(|e| Error::Package(e.to_string()))?;
        write_file(&dir.join("plugin.cwasm"), &pkg.cwasm, false).map_err(Error::Io)?;
        write_file(&dir.join("plugin.toml"), toml.as_bytes(), false).map_err(Error::Io)?;
        match &pkg.readme {
            Some(r) => write_file(&dir.join("README.md"), r.as_bytes(), false).map_err(Error::Io)?,
            None => {
                let _ = std::fs::remove_file(dir.join("README.md"));
            }
        }
        if sideloaded {
            write_file(&dir.join("sideloaded"), b"", false).map_err(Error::Io)?;
        } else {
            let _ = std::fs::remove_file(dir.join("sideloaded"));
        }
        let _ = std::fs::remove_file(dir.join("disabled"));
        Ok(Installed { manifest: pkg.manifest.clone(), dir, enabled: true, sideloaded })
    }

    /// Removes a plugin and everything it kept (settings, secrets, results).
    pub fn remove(&self, id: &str) -> Result<()> {
        if !crate::manifest::valid_id(id) {
            return Err(Error::Io(format!("\"{id}\" isn't a plugin id")));
        }
        let dir = self.dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| Error::Io(e.to_string()))?;
        }
        Ok(())
    }

    pub fn set_enabled(&self, id: &str, on: bool) -> Result<()> {
        let flag = self.dir(id).join("disabled");
        if on {
            let _ = std::fs::remove_file(flag);
            Ok(())
        } else {
            write_file(&flag, b"", false).map_err(Error::Io)
        }
    }

    /// Starts an installed plugin (two instances: one for panels, one for actions).
    pub fn start(&self, installed: &Installed) -> Result<Loaded> {
        let bytes = std::fs::read(installed.dir.join("plugin.cwasm")).map_err(|e| Error::Io(e.to_string()))?;
        let component = runtime::load(&bytes)?;
        Loaded::start(installed.manifest.clone(), installed.dir.clone(), component, Arc::new(Activity::default()))
    }
}

/// Reads a package file from disk.
pub fn read_package(path: &Path) -> Result<Package> {
    let len = std::fs::metadata(path).map_err(|e| Error::Io(e.to_string()))?.len();
    if len > MAX_PACKAGE {
        return Err(Error::Package("the package is too big".into()));
    }
    Package::read(&std::fs::read(path).map_err(|e| Error::Io(e.to_string()))?)
}
