//! Plugins: build the ones in `plugins/` (official and community) into `.scplugin` packages, write the list the app
//! browses, and run the end-to-end tests against real built plugins.
//!
//!   cargo xtask plugin build [FOLDER…]   packages into target/plugins/ (official, community and examples)
//!   cargo xtask plugin index             target/plugins/plugins-index.json for this version
//!   cargo xtask plugin test              builds, then runs the host's end-to-end tests with the packages

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::Digest;

use crate::util::{root, target, version};

pub fn main(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("build") => build(&args[1..]).map(|_| ()),
        Some("index") => index(),
        Some("test") => test(),
        _ => bail!("cargo xtask plugin build [FOLDER…] | index | test"),
    }
}

fn out_dir() -> PathBuf {
    target().join("plugins")
}

/// Every plugin folder with a plugin.toml: plugins/official/* and plugins/community/* (listed in Browse), and with
/// `examples` also plugins/examples/* (built and tested, never listed).
fn folders(examples: bool) -> Vec<(PathBuf, &'static str)> {
    let mut out = Vec::new();
    let tiers: &[&'static str] = if examples { &["official", "community", "examples"] } else { &["official", "community"] };
    for &tier in tiers {
        let Ok(rd) = std::fs::read_dir(root().join("plugins").join(tier)) else { continue };
        let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.join("plugin.toml").exists()).collect();
        v.sort();
        out.extend(v.into_iter().map(|p| (p, tier)));
    }
    out
}

/// The cdylib's file name from its Cargo.toml (`name = "hello"` → hello.wasm).
fn crate_name(dir: &Path) -> Result<String> {
    let text = std::fs::read_to_string(dir.join("Cargo.toml")).with_context(|| format!("{}: Cargo.toml", dir.display()))?;
    let v: toml::Value = toml::from_str(&text).with_context(|| format!("{}: Cargo.toml", dir.display()))?;
    let name = v.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()).context("no package name")?;
    Ok(name.replace('-', "_"))
}

/// Builds the plugins workspace for WebAssembly, then packages each folder asked for (all when none).
pub fn build(only: &[String]) -> Result<Vec<PathBuf>> {
    let ws = root().join("plugins");
    let plugin_target = target().join("plugins-build");
    let status = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(&ws)
        .args(["build", "--release", "--target", "wasm32-unknown-unknown", "--target-dir"])
        .arg(&plugin_target)
        .status()
        .context("couldn't start cargo")?;
    if !status.success() {
        bail!("building the plugins failed (rustup target add wasm32-unknown-unknown?)");
    }
    let wasm_dir = plugin_target.join("wasm32-unknown-unknown").join("release");
    std::fs::create_dir_all(out_dir())?;
    let mut made = Vec::new();
    for (dir, _) in folders(true) {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if !only.is_empty() && !only.iter().any(|o| o == &name || Path::new(o).ends_with(&name)) {
            continue;
        }
        let toml = std::fs::read_to_string(dir.join("plugin.toml"))?;
        let manifest = shapecraft_plugin_host::Manifest::parse(&toml).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
        let wasm_path = wasm_dir.join(format!("{}.wasm", crate_name(&dir)?));
        let wasm = std::fs::read(&wasm_path).with_context(|| format!("{} wasn't built", wasm_path.display()))?;
        let readme = std::fs::read_to_string(dir.join("README.md")).ok();
        let pkg = shapecraft_plugin_host::build_package(&wasm, &toml, readme.as_deref())
            .map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
        let out = out_dir().join(format!("{}-{}.scplugin", manifest.id, manifest.version));
        std::fs::write(&out, &pkg)?;
        println!("{} ({} KB)", out.display(), pkg.len() / 1024);
        made.push(out);
    }
    Ok(made)
}

/// The list the app reads: every built package with its manifest, tier, size and SHA-256.
fn index() -> Result<()> {
    let mut plugins = Vec::new();
    for (dir, tier) in folders(false) {
        let toml = std::fs::read_to_string(dir.join("plugin.toml"))?;
        let manifest = shapecraft_plugin_host::Manifest::parse(&toml).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
        let file = format!("{}-{}.scplugin", manifest.id, manifest.version);
        let bytes = std::fs::read(out_dir().join(&file)).with_context(|| format!("{file} isn't built (cargo xtask plugin build)"))?;
        let sha256: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        plugins.push(shapecraft_plugin_host::index::Entry { manifest, tier: tier.to_string(), file, size: bytes.len() as u64, sha256 });
    }
    let idx = shapecraft_plugin_host::index::Index { app: version()?, plugins };
    let path = out_dir().join(shapecraft_plugin_host::index::INDEX);
    std::fs::write(&path, serde_json::to_string_pretty(&idx)?)?;
    println!("{} ({} plugins)", path.display(), idx.plugins.len());
    Ok(())
}

fn test() -> Result<()> {
    build(&[])?;
    index()?;
    let status = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root())
        .args(["test", "-p", "shapecraft-plugin-host", "--features", "compile", "--test", "e2e"])
        .env("SHAPECRAFT_TEST_PLUGINS", out_dir())
        .status()
        .context("couldn't start cargo")?;
    if !status.success() {
        bail!("the plugin end-to-end tests failed");
    }
    Ok(())
}
