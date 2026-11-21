//! ShapeCraft plugins.
//!
//! A plugin is a WebAssembly component (API: `wit/shapecraft-plugin.wit`) with a `plugin.toml` that names its
//! permissions. The app runs it in wasmtime's Pulley interpreter: no files, sockets or clock of its own, only the
//! host functions here, each checked against those permissions. Panels are JSON the app draws itself.
//!
//! Nothing here depends on the UI: the app (or a test) starts plugins from the [`library::Library`], shows their
//! [`panel::Panel`]s and runs their actions through [`worker::Loaded`].

pub mod images;
pub mod index;
pub mod library;
pub mod manifest;
pub mod net;
pub mod panel;
pub mod results;
pub mod runtime;
pub mod signin;
pub mod store;
pub mod worker;

pub use library::{Installed, Library, Package};
pub use manifest::Manifest;
pub use panel::{Control, Panel, Spend};
pub use runtime::{Activity, Capture};
pub use worker::{Loaded, Ticket};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("plugin.toml: {0}")]
    Manifest(String),
    #[error("this plugin needs a newer ShapeCraft (plugin API {0})")]
    Api(String),
    #[error("the package can't be used: {0}")]
    Package(String),
    #[error("the plugin can't be loaded by this version of ShapeCraft: {0}")]
    Load(String),
    #[error("{0}")]
    Runtime(String),
    /// An error the plugin itself returned: shown as it is.
    #[error("{0}")]
    Plugin(String),
    #[error("the plugin stopped unexpectedly ({0})")]
    Trap(String),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Builds a package from a component (`.wasm`): precompiles it for the app's runtime and zips it with its manifest.
/// Used by `cargo xtask plugin` and the tests; the app itself never compiles.
#[cfg(feature = "compile")]
pub fn build_package(component_or_module: &[u8], manifest_toml: &str, readme: Option<&str>) -> Result<Vec<u8>> {
    Manifest::parse(manifest_toml)?;
    let component = componentize(component_or_module)?;
    let cwasm = runtime::precompile(&component)?;
    Package::write(manifest_toml, &cwasm, readme)
}

/// A core module built with wit-bindgen (wasm32-unknown-unknown) becomes a component; a component stays as it is.
#[cfg(feature = "compile")]
pub fn componentize(bytes: &[u8]) -> Result<Vec<u8>> {
    // Byte 4..8 is the version/layer: components have layer 1.
    if bytes.len() >= 8 && bytes[0..4] == *b"\0asm" && bytes[6..8] == [1, 0] {
        return Ok(bytes.to_vec());
    }
    wit_component::ComponentEncoder::default()
        .module(bytes)
        .and_then(|e| e.validate(true).encode())
        .map_err(|e| Error::Package(format!("{e:#}")))
}
