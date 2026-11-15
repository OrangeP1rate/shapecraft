//! The Windows downloads from a release build: a portable zip (the app, the CLI and every DLL they load that Windows
//! doesn't ship: OpenCASCADE, TBB, the VC runtime…), an installer built from the same folder with Inno Setup, and an
//! MSI of the same folder with WiX (what the Craft App Suite's launcher installs).
//!
//!   cargo build --release -p shapecraft -p shapecraft-cli
//!   cargo xtask bundle-windows          (in a Visual Studio developer shell, for dumpbin)
//!   → target\shapecraft-<version>-windows-x64-portable.zip, …-windows-x64-setup.exe and …-windows-x64.msi
//!
//! DLLs are looked up in OCCT_BIN (default: CONDA_PREFIX\Library\bin), then CONDA_PREFIX, then the Visual C++ runtime
//! that comes with the build tools (VCToolsRedistDir); anything not found there is treated as part of Windows. The
//! files are named for the chip they're built on: `windows-x64`, or `windows-arm64` on Windows on Arm (#61), whose
//! OpenCASCADE comes from `cargo xtask windows-occt` (conda-forge has none for Arm). The installer needs Inno Setup 6 (iscc on PATH or in Program Files), the MSI WiX 5
//! (`wix` on PATH); either is skipped with a warning when its tool is missing.

use std::collections::{BTreeSet, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::util::{clear, copy, output, root, s, target, version};

/// The chip the files are named for, as Windows names it: the one this build runs on.
fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" }
}

/// Builds OpenCASCADE for this machine's chip as DLLs: only the toolkits the shim links and none of the optional
/// third-party products, as for Android. For Windows on Arm, where conda-forge has no OCCT (#61). Run it in a Visual
/// Studio developer shell for the chip (cl, cmake and ninja on PATH), with OCCT_SRC an OCCT 7.9 source tree:
///   cargo xtask windows-occt   → target\windows-occt-<arch>\{bin,lib,include\opencascade}
/// then point the build at it with OCCT_BIN, OCCT_LIB_DIR and OCCT_INCLUDE_DIR.
pub fn occt() -> Result<()> {
    use crate::android::{find, linked_toolkits};
    use crate::util::run;
    let src = PathBuf::from(std::env::var_os("OCCT_SRC").context("set OCCT_SRC to an OCCT 7.9 source tree")?);
    let build = target().join(format!("windows-occt-build-{}", arch()));
    let install = target().join(format!("windows-occt-{}", arch()));
    let mut args = vec![
        "-S".to_string(),
        s(&src).to_string(),
        "-B".to_string(),
        s(&build).to_string(),
        "-G".to_string(),
        "Ninja".to_string(),
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
        "-DBUILD_LIBRARY_TYPE=Shared".to_string(),
        "-DBUILD_DOC_Overview=OFF".to_string(),
        "-DBUILD_SAMPLES_QT=OFF".to_string(),
        "-DBUILD_MODULE_Draw=OFF".to_string(),
    ];
    for off in ["FREETYPE", "OPENGL", "GLES2", "D3D", "TK", "FREEIMAGE", "RAPIDJSON", "TBB", "VTK", "DRACO", "OPENVR", "FFMPEG"] {
        args.push(format!("-DUSE_{off}=OFF"));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run("cmake", &args)?;
    let toolkits = linked_toolkits()?;
    let mut build_args = vec!["--build", s(&build), "--target"];
    build_args.extend(toolkits.iter().map(String::as_str));
    run("cmake", &build_args)?;
    // `install` would build every toolkit, so copy the DLLs, their import libraries and the headers.
    clear(&install)?;
    for tk in &toolkits {
        for (name, into) in [(format!("{tk}.dll"), "bin"), (format!("{tk}.lib"), "lib")] {
            let found = find(&build, &name).with_context(|| format!("{name} wasn't built"))?;
            copy(&found, &install.join(into).join(&name))?;
        }
    }
    // Headers: where the configure step laid them out (it does on Linux and Android), else gathered flat from the
    // source tree as `install` would (on Windows the build folder has none).
    let into = install.join("include").join("opencascade");
    std::fs::create_dir_all(&into)?;
    let from = match find(&build, "Standard.hxx").and_then(|p| p.parent().map(Path::to_path_buf)) {
        Some(dir) => vec![dir],
        None => vec![src.join("src")],
    };
    let mut copied = 0usize;
    let mut stack = from;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).with_context(|| format!("no headers in {}", dir.display()))? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let header = path.extension().is_some_and(|x| ["hxx", "lxx", "gxx", "h", "pxx"].iter().any(|e| x.eq_ignore_ascii_case(e)));
            if let (true, Some(name)) = (header, path.file_name()) {
                std::fs::copy(&path, into.join(name)).with_context(|| format!("copying {}", path.display()))?;
                copied += 1;
            }
        }
    }
    if !into.join("Standard.hxx").exists() {
        bail!("no OCCT headers found ({copied} files copied)");
    }
    println!("OCCT for Windows {} ({} toolkits) → {}", arch(), toolkits.len(), install.display());
    Ok(())
}

/// The DLLs a file imports (dumpbin /dependents).
fn imports(file: &Path) -> Result<Vec<String>> {
    Ok(output("dumpbin", &["/nologo", "/dependents", s(file)])?
        .lines()
        .map(str::trim)
        .filter(|l| !l.contains(' ') && l.to_ascii_lowercase().ends_with(".dll"))
        .map(str::to_string)
        .collect())
}

/// Zips `dir` with its own folder name at the top, as Compress-Archive does.
fn zip_folder(dir: &Path, out: &Path) -> Result<()> {
    let top = dir.file_name().map(|n| n.to_string_lossy().into_owned()).context("a folder without a name")?;
    let file = std::fs::File::create(out).with_context(|| format!("creating {}", out.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(9));
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&d)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            let rel = p.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
            let name = format!("{top}/{rel}");
            if p.is_dir() {
                zip.add_directory(format!("{name}/"), opts)?;
                stack.push(p);
            } else {
                zip.start_file(name, opts)?;
                zip.write_all(&std::fs::read(&p)?)?;
            }
        }
    }
    zip.finish()?;
    Ok(())
}

pub fn bundle() -> Result<()> {
    let version = version()?;
    let release = target().join("release");
    let name = format!("shapecraft-{version}-windows-{}", arch());
    let stage = target().join("windows-bundle").join("ShapeCraft");
    let mut search: Vec<PathBuf> = Vec::new();
    if let Some(b) = std::env::var_os("OCCT_BIN") {
        search.push(PathBuf::from(b));
    }
    if let Some(c) = std::env::var_os("CONDA_PREFIX") {
        search.push(PathBuf::from(&c).join("Library").join("bin"));
        search.push(PathBuf::from(c));
    }
    if search.is_empty() {
        bail!("set OCCT_BIN or activate the conda environment with OCCT");
    }
    // The C++ runtime (msvcp140, vcruntime140…): conda's copy where there is one, else the build tools' own
    // redistributable folder, so a machine without the VC++ Redistributable still starts the app.
    if let Some(redist) = std::env::var_os("VCToolsRedistDir") {
        let dir = PathBuf::from(redist).join(arch());
        if let Ok(entries) = std::fs::read_dir(&dir) {
            search.extend(entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| {
                p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Microsoft.VC") && n.to_string_lossy().ends_with(".CRT"))
            }));
        }
    }
    clear(&target().join("windows-bundle"))?;
    std::fs::create_dir_all(&stage)?;
    for exe in ["shapecraft.exe", "shapecraft-cli.exe"] {
        if !release.join(exe).exists() {
            bail!("no release build: {}", release.join(exe).display());
        }
        copy(&release.join(exe), &stage.join(exe))?;
    }

    // The DLL closure: what each file imports, found in the search folders, copied, and walked in turn.
    let mut queue: VecDeque<PathBuf> = VecDeque::from([stage.join("shapecraft.exe"), stage.join("shapecraft-cli.exe")]);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while let Some(file) = queue.pop_front() {
        for dll in imports(&file)? {
            if !seen.insert(dll.to_ascii_lowercase()) {
                continue;
            }
            if let Some(src) = search.iter().map(|d| d.join(&dll)).find(|p| p.exists()) {
                copy(&src, &stage.join(&dll))?;
                queue.push_back(stage.join(&dll));
            }
        }
    }
    let count = std::fs::read_dir(&stage)?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")))
        .count();
    println!("{count} DLLs bundled");
    for f in ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "DISCLAIMER.md"] {
        if root().join(f).exists() {
            copy(&root().join(f), &stage.join(f))?;
        }
    }
    copy(&root().join("assets/icon/shapecraft.ico"), &stage.join("shapecraft.ico"))?;

    // Both programs must start with only the folder's DLLs: the CLI is enough to load all of OCCT.
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let status = std::process::Command::new(stage.join("shapecraft-cli.exe"))
        .arg("commands")
        .env("PATH", format!("{windir}\\system32;{windir}"))
        .stdout(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        bail!("shapecraft-cli.exe didn't start from the bundle folder ({status})");
    }

    let zip = target().join(format!("{name}-portable.zip"));
    let _ = std::fs::remove_file(&zip);
    zip_folder(&stage, &zip)?;
    println!("{}", zip.display());

    let iscc = std::process::Command::new("iscc").arg("/?").output().is_ok().then(|| PathBuf::from("iscc")).or_else(|| {
        let pf = std::env::var("ProgramFiles(x86)").ok()?;
        let p = PathBuf::from(pf).join("Inno Setup 6").join("ISCC.exe");
        p.exists().then_some(p)
    });
    let Some(iscc) = iscc else {
        eprintln!("warning: Inno Setup not found: no installer made");
        return msi(&stage, &version, &name);
    };
    let status = std::process::Command::new(&iscc)
        .args([
            "/Qp".to_string(),
            format!("/DVersion={version}"),
            format!("/DStage={}", stage.display()),
            format!("/DOutDir={}", target().display()),
            format!("/DOutName={name}-setup"),
            root().join("packaging/windows/shapecraft.iss").display().to_string(),
        ])
        .status()
        .context("couldn't start Inno Setup")?;
    if !status.success() {
        bail!("Inno Setup failed ({status})");
    }
    println!("{}", target().join(format!("{name}-setup.exe")).display());
    msi(&stage, &version, &name)
}

/// The MSI, from the same folder (WiX v5: `dotnet tool install --global wix --version 5.0.2`). The launcher of the
/// Craft App Suite installs apps from their MSI. Its version is the numeric X.Y.Z (MSI versions have no labels).
fn msi(stage: &Path, version: &str, name: &str) -> Result<()> {
    if std::process::Command::new("wix").arg("--version").output().is_err() {
        eprintln!("warning: WiX not found: no MSI made");
        return Ok(());
    }
    let numeric = version.split(['-', '+']).next().unwrap_or(version);
    let out = target().join(format!("{name}.msi"));
    let _ = std::fs::remove_file(&out);
    let status = std::process::Command::new("wix")
        .args([
            "build".to_string(),
            "-arch".into(),
            arch().into(),
            "-d".into(),
            format!("Version={numeric}"),
            "-d".into(),
            format!("Stage={}", stage.display()),
            "-d".into(),
            format!("Marker={}", root().join("packaging/windows/installed-by-msi.txt").display()),
            "-o".into(),
            out.display().to_string(),
            root().join("packaging/windows/shapecraft.wxs").display().to_string(),
        ])
        .status()
        .context("couldn't start WiX")?;
    if !status.success() {
        bail!("WiX failed ({status})");
    }
    println!("{}", out.display());
    Ok(())
}
