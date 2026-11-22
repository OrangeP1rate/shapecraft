//! The one place an app picks its geometry kernel.
//!
//! Everything above the kernel (engine, UI, MCP, file formats) talks to `dyn Kernel` only, so the kernel is chosen
//! once, at startup: `--kernel NAME` on the command line, else `SHAPECRAFT_KERNEL=NAME`, else the default (OCCT when
//! this build has it, the pure-Rust kernel otherwise).
//!
//! - `occt`: Open CASCADE, the full modeler (needs the `occt` feature, on by default).
//! - `rust`: the pure-Rust kernel; flat-faced solids so far, everything else says "not in the Rust kernel yet".

use std::sync::Arc;

use shapecraft_kernel::Kernel;

/// The kernels this build can run, default first.
pub fn names() -> &'static [&'static str] {
    if cfg!(feature = "occt") { &["occt", "rust"] } else { &["rust"] }
}

/// A kernel by name.
pub fn make(name: &str) -> Result<Arc<dyn Kernel>, String> {
    match name.trim().to_ascii_lowercase().as_str() {
        #[cfg(feature = "occt")]
        "occt" | "opencascade" => Ok(Arc::new(shapecraft_kernel_occt::OcctKernel::new())),
        "rust" => Ok(Arc::new(shapecraft_kernel_rs::RustKernel::new())),
        other => Err(format!("no kernel called \"{other}\" in this build (it has: {})", names().join(", "))),
    }
}

/// The kernel a process was asked for: `--kernel NAME` (or `--kernel=NAME`) in `args`, else `SHAPECRAFT_KERNEL`,
/// else the default.
pub fn chosen(args: &[String]) -> Result<Arc<dyn Kernel>, String> {
    make(&chosen_name(args, std::env::var("SHAPECRAFT_KERNEL").ok()))
}

fn chosen_name(args: &[String], env: Option<String>) -> String {
    let flag = args.iter().position(|a| a == "--kernel").and_then(|i| args.get(i + 1)).cloned();
    let inline = args.iter().find_map(|a| a.strip_prefix("--kernel=").map(str::to_string));
    flag.or(inline).or(env.filter(|v| !v.trim().is_empty())).unwrap_or_else(|| names()[0].to_string())
}

/// `args` without `--kernel NAME` / `--kernel=NAME`, for apps that read their other arguments by position.
pub fn strip_kernel_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a == "--kernel" {
            skip = true;
        } else if !a.starts_with("--kernel=") {
            out.push(a.clone());
        }
    }
    out
}

/// Call last in `main`: lets the kernel end the process safely (OCCT can't be torn down under a measurement that
/// is still running). Returns when there's nothing to do.
pub fn finish_process(code: i32) {
    #[cfg(feature = "occt")]
    shapecraft_kernel_occt::finish_process(code);
    #[cfg(not(feature = "occt"))]
    let _ = code;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn the_flag_beats_the_environment_and_the_default() {
        assert_eq!(chosen_name(&s(&["run", "x.jsonl", "--kernel", "rust"]), Some("occt".into())), "rust");
        assert_eq!(chosen_name(&s(&["--kernel=rust"]), None), "rust");
        assert_eq!(chosen_name(&s(&["run"]), Some("rust".into())), "rust");
        assert_eq!(chosen_name(&s(&["run"]), None), names()[0]);
        assert_eq!(strip_kernel_args(&s(&["run", "--kernel", "rust", "a", "--kernel=occt", "b"])), s(&["run", "a", "b"]));
    }

    #[test]
    fn the_rust_kernel_is_always_there() {
        assert_eq!(make("rust").unwrap().name(), "rust");
        assert!(make("acis").err().is_some_and(|e| e.contains("rust")));
    }
}
