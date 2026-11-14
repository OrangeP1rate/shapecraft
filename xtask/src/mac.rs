//! ShapeCraft.app and a DMG from a release build: the app, the CLI (MCP server) beside it, and every library they
//! load that macOS doesn't ship (OpenCASCADE and what it brings), so the Mac needs nothing else installed.
//!
//!   cargo build --release -p shapecraft -p shapecraft-cli && cargo xtask bundle-mac
//!   → target/shapecraft-<version>-macos-<arch>.dmg (this Mac's chip only)
//!
//!   cargo build --release --target aarch64-apple-darwin … && cargo build --release --target x86_64-apple-darwin …
//!   OCCT_LIB_DIR_ARM64=… OCCT_LIB_DIR_X86_64=… cargo xtask bundle-mac --universal
//!   → target/shapecraft-<version>-macos-universal.dmg (Apple silicon and Intel in one app; what releases ship)
//!
//! A universal app is put together once per chip, each from its own build and its own OCCT, then every program and
//! library is merged with `lipo`.
//!
//! Libraries are found the way the loader would (absolute paths, @rpath through each file's LC_RPATH, @loader_path),
//! with OCCT_LIB_DIR and $CONDA_PREFIX/lib as fallbacks, copied into Contents/Frameworks and pointed at from there.
//! The app is signed ad hoc (unsigned builds: first launch needs Open Anyway in Privacy & Security).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::util::{clear, copy, mb, output, root, run, s, target, version};

const SYSTEM: [&str; 2] = ["/usr/lib/", "/System/"];
const INSIDE: &str = "@executable_path/../Frameworks/";

/// The libraries a Mach-O file loads (not its own id).
fn deps(path: &Path) -> Result<Vec<String>> {
    let names: Vec<String> = output("otool", &["-L", s(path)])?
        .lines()
        .skip(1)
        .filter_map(|l| l.trim().split(" (").next().map(str::to_string))
        .filter(|n| !n.is_empty())
        .collect();
    let own = output("otool", &["-D", s(path)])?.lines().nth(1).map(|l| l.trim().to_string());
    Ok(names.into_iter().filter(|n| own.as_deref() != Some(n.as_str())).collect())
}

/// The LC_RPATH entries of a Mach-O file, as written.
fn rpath_entries(path: &Path) -> Result<Vec<String>> {
    let text = output("otool", &["-l", s(path)])?;
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    Ok(lines
        .windows(3)
        .filter(|w| w[0] == "cmd LC_RPATH")
        .filter_map(|w| w[2].strip_prefix("path ").and_then(|p| p.split(" (").next()).map(str::to_string))
        .collect())
}

fn rpaths(path: &Path) -> Result<Vec<PathBuf>> {
    let dir = path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    Ok(rpath_entries(path)?
        .into_iter()
        .map(|r| PathBuf::from(r.replace("@loader_path", s(&dir)).replace("@executable_path", s(&dir))))
        .collect())
}

/// Where a single-chip bundle also looks for libraries.
fn host_fallbacks() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = std::env::var_os("OCCT_LIB_DIR") {
        v.push(PathBuf::from(d));
    }
    if let Some(c) = std::env::var_os("CONDA_PREFIX") {
        v.push(PathBuf::from(c).join("lib"));
    }
    v
}

/// Where the loader would find `name`, asked for by the file at `origin`.
fn resolve(name: &str, origin: &Path, fallbacks: &[PathBuf]) -> Result<PathBuf> {
    let found = |p: PathBuf| p.exists().then(|| p.canonicalize().unwrap_or(p));
    if let Some(rest) = name.strip_prefix("@rpath/") {
        for d in rpaths(origin)?.into_iter().chain(fallbacks.iter().cloned()) {
            if let Some(p) = found(d.join(rest)) {
                return Ok(p);
            }
        }
    } else if let Some(rest) = name.strip_prefix("@loader_path/").or_else(|| name.strip_prefix("@executable_path/")) {
        if let Some(p) = origin.parent().and_then(|d| found(d.join(rest))) {
            return Ok(p);
        }
    } else if let Some(p) = found(PathBuf::from(name)) {
        return Ok(p);
    }
    let base = Path::new(name).file_name().unwrap_or_default();
    for d in fallbacks {
        if let Some(p) = found(d.join(base)) {
            return Ok(p);
        }
    }
    bail!("can't find {name} (needed by {})", origin.display())
}

fn plist(version: &str) -> String {
    let exts = ["shapr", "step", "stp", "iges", "igs", "x_t", "x_b", "stl", "obj"]
        .iter()
        .map(|e| format!("\t\t\t\t<string>{e}</string>\n"))
        .collect::<String>();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDisplayName</key>
	<string>ShapeCraft</string>
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleTypeExtensions</key>
			<array>
{exts}			</array>
			<key>CFBundleTypeName</key>
			<string>CAD file</string>
			<key>CFBundleTypeRole</key>
			<string>Viewer</string>
			<key>LSHandlerRank</key>
			<string>Alternate</string>
		</dict>
	</array>
	<key>CFBundleExecutable</key>
	<string>shapecraft</string>
	<key>CFBundleIconFile</key>
	<string>ShapeCraft</string>
	<key>CFBundleIdentifier</key>
	<string>com.jackharvest.shapecraft</string>
	<key>CFBundleName</key>
	<string>ShapeCraft</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.graphics-design</string>
	<key>LSMinimumSystemVersion</key>
	<string>13.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
"#
    )
}

pub fn bundle(args: &[String]) -> Result<()> {
    let version = version()?;
    let universal = args.iter().any(|a| a == "--universal");
    if let Some(other) = args.iter().find(|a| *a != "--universal") {
        bail!("bundle-mac: unknown option {other} (only --universal)");
    }
    let stage = target().join("macos-bundle");
    clear(&stage)?;
    let app = stage.join("ShapeCraft.app");
    let (arch, libraries) = if universal {
        let mut counts = Vec::new();
        let mut apps = Vec::new();
        for (triple, var) in [("aarch64-apple-darwin", "OCCT_LIB_DIR_ARM64"), ("x86_64-apple-darwin", "OCCT_LIB_DIR_X86_64")] {
            let lib = std::env::var_os(var).map(PathBuf::from).with_context(|| format!("--universal needs {var}"))?;
            let one = target().join(format!("macos-bundle-{triple}"));
            clear(&one)?;
            let one_app = one.join("ShapeCraft.app");
            counts.push(assemble(&one_app, &target().join(triple).join("release"), &version, &[lib])?);
            apps.push(one_app);
        }
        merge(&apps[0], &apps[1], &app)?;
        ("universal", counts.into_iter().max().unwrap_or(0))
    } else {
        let arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            a => a,
        };
        (arch, assemble(&app, &target().join("release"), &version, &host_fallbacks())?)
    };
    run("codesign", &["--force", "--deep", "--sign", "-", s(&app)])?;
    applications_link(&stage)?;
    let dmg = target().join(format!("shapecraft-{version}-macos-{arch}.dmg"));
    let _ = std::fs::remove_file(&dmg);
    run(
        "hdiutil",
        &[
            "create",
            "-quiet",
            "-volname",
            &format!("ShapeCraft {version}"),
            "-srcfolder",
            s(&stage),
            "-fs",
            "HFS+",
            "-format",
            "UDZO",
            "-imagekey",
            "zlib-level=9",
            s(&dmg),
        ],
    )?;
    println!("{}  ({libraries} libraries inside, {})", dmg.display(), mb(&dmg));
    Ok(())
}

/// ShapeCraft.app at `app` (unsigned) from the programs in `release`, with every library they load copied in from
/// where the loader would find it or from `fallbacks`. Returns how many libraries went in.
fn assemble(app: &Path, release: &Path, version: &str, fallbacks: &[PathBuf]) -> Result<usize> {
    let exes = ["shapecraft", "shapecraft-cli"];
    for e in exes {
        if !release.join(e).exists() {
            bail!("no release build at {} (cargo build --release -p shapecraft -p shapecraft-cli)", release.join(e).display());
        }
    }
    let (macos, frameworks, resources) = (app.join("Contents/MacOS"), app.join("Contents/Frameworks"), app.join("Contents/Resources"));
    for d in [&macos, &frameworks, &resources] {
        std::fs::create_dir_all(d)?;
    }
    for e in exes {
        copy(&release.join(e), &macos.join(e))?;
    }
    copy(&root().join("assets/icon/shapecraft.icns"), &resources.join("ShapeCraft.icns"))?;
    for lic in ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE"] {
        copy(&root().join(lic), &resources.join(lic))?;
    }
    std::fs::write(app.join("Contents/Info.plist"), plist(version))?;

    // Every non-system library, transitively, copied flat into Frameworks.
    let mut queue: Vec<(PathBuf, PathBuf)> = exes.iter().map(|e| (macos.join(e), release.join(e))).collect();
    let mut copied: BTreeMap<String, PathBuf> = BTreeMap::new();
    while let Some((inside, origin)) = queue.pop() {
        for name in deps(&inside)? {
            if SYSTEM.iter().any(|p| name.starts_with(p)) {
                continue;
            }
            let base = Path::new(&name).file_name().map(|b| b.to_string_lossy().into_owned()).context("a library without a name")?;
            if !copied.contains_key(&base) {
                let src = resolve(&name, &origin, fallbacks)?;
                let dst = frameworks.join(&base);
                copy(&src, &dst)?;
                set_mode(&dst)?;
                queue.push((dst, src.clone()));
                copied.insert(base.clone(), src);
            }
            run("install_name_tool", &["-change", &name, &format!("{INSIDE}{base}"), s(&inside)])?;
        }
    }
    for base in copied.keys() {
        run("install_name_tool", &["-id", &format!("{INSIDE}{base}"), s(&frameworks.join(base))])?;
    }
    // Old search paths point at the build machine; drop them so nothing outside the app is ever loaded.
    let files: Vec<PathBuf> = exes.iter().map(|e| macos.join(e)).chain(copied.keys().map(|b| frameworks.join(b))).collect();
    for f in &files {
        for rp in rpath_entries(f)? {
            let _ = std::process::Command::new("install_name_tool").args(["-delete_rpath", &rp, s(f)]).output();
        }
    }
    let mut leaks = Vec::new();
    for f in &files {
        for d in deps(f)? {
            if !SYSTEM.iter().any(|p| d.starts_with(p)) && !d.starts_with(INSIDE) {
                leaks.push(format!("{}: {d}", f.display()));
            }
        }
    }
    if !leaks.is_empty() {
        bail!("still pointing outside the app:\n  {}", leaks.join("\n  "));
    }
    Ok(copied.len())
}

/// The chips a file holds code for (`arm64 x86_64`), or None when it isn't a program or library.
fn archs(p: &Path) -> Option<String> {
    output("lipo", &["-archs", s(p)]).ok().map(|o| o.trim().to_string()).filter(|o| !o.is_empty())
}

/// One app from two single-chip apps: programs and libraries both have are joined with `lipo`; anything only one
/// has (a library only one chip's OCCT needs) goes in as it is, since only that chip's programs load it.
fn merge(a: &Path, b: &Path, out: &Path) -> Result<()> {
    let mut rels = BTreeMap::new();
    for (i, tree) in [a, b].into_iter().enumerate() {
        for rel in files_under(tree, tree)? {
            rels.entry(rel).or_insert(i);
        }
    }
    for (rel, first) in rels {
        let (pa, pb, dst) = (a.join(&rel), b.join(&rel), out.join(&rel));
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if pa.exists() && pb.exists() && archs(&pa).is_some() {
            run("lipo", &["-create", s(&pa), s(&pb), "-output", s(&dst)])?;
            set_mode(&dst)?;
        } else {
            if archs(&pa).or_else(|| archs(&pb)).is_some() {
                println!("  only for {}: {}", if first == 0 { "Apple silicon" } else { "Intel" }, rel.display());
            }
            copy(if pa.exists() { &pa } else { &pb }, &dst)?;
        }
    }
    for e in ["shapecraft", "shapecraft-cli"] {
        let got = archs(&out.join("Contents/MacOS").join(e)).unwrap_or_default();
        if !(got.contains("arm64") && got.contains("x86_64")) {
            bail!("{e} isn't universal (has: {got})");
        }
    }
    Ok(())
}

/// Every file under `dir`, relative to `base`.
fn files_under(dir: &Path, base: &Path) -> Result<Vec<PathBuf>> {
    let mut v = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            v.extend(files_under(&p, base)?);
        } else {
            v.push(p.strip_prefix(base)?.to_path_buf());
        }
    }
    Ok(v)
}

#[cfg(unix)]
fn set_mode(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn applications_link(stage: &Path) -> Result<()> {
    std::os::unix::fs::symlink("/Applications", stage.join("Applications")).context("the Applications link")
}

#[cfg(not(unix))]
fn applications_link(_: &Path) -> Result<()> {
    bail!("bundle-mac runs on macOS")
}
