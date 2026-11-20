//! End to end with real built plugins (`cargo xtask plugin test` builds them and points SHAPECRAFT_TEST_PLUGINS at
//! target/plugins). Without that variable these tests say so and pass, so a plain `cargo test` stays quick.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use shapecraft_plugin_host::images::{Mask, Picture};
use shapecraft_plugin_host::{Capture, Error, Library, Panel, index, library};

fn packages() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("SHAPECRAFT_TEST_PLUGINS")?);
    p.join("hello-0.1.0.scplugin").exists().then_some(p)
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sc-plugins-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A one-shot web server on this computer answering `/ping` with "pong".
fn pong_server() -> u16 {
    let Ok(l) = TcpListener::bind(("127.0.0.1", 0)) else { return 0 };
    let port = l.local_addr().map(|a| a.port()).unwrap_or(0);
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\npong");
        }
    });
    port
}

#[test]
fn hello_end_to_end() -> Result<(), Error> {
    let Some(pkgs) = packages() else {
        eprintln!("SHAPECRAFT_TEST_PLUGINS not set: run `cargo xtask plugin test`");
        return Ok(());
    };
    let lib = Library::new(scratch("hello"));
    let pkg = library::read_package(&pkgs.join("hello-0.1.0.scplugin"))?;
    assert_eq!(pkg.manifest.id, "hello");
    let installed = lib.install(&pkg, false)?;
    assert_eq!(lib.list().len(), 1);
    let p = lib.start(&installed)?;

    // A panel, as JSON the app reads.
    let panel = Panel::parse(&p.panel("main", &json!({"name": "Ada"})).wait()?).map_err(Error::Plugin)?;
    assert_eq!(panel.title, "Hello");
    assert!(panel.find("greet").is_some());
    assert!(p.panel("nope", &json!({})).wait().is_err());

    // An action with settings kept between calls.
    let out = p.run("greet", &json!({"name": "Ada", "loud": true}), None).wait()?;
    assert!(out.contains("HELLO, ADA!"), "{out}");
    p.run("greet", &json!({}), None).wait()?;
    assert_eq!(shapecraft_plugin_host::store::KeyFile::settings(&installed.dir).get("count").as_deref(), Some("2"));

    // Pictures: a made one, then the captured view, kept as results.
    p.run("picture", &json!({}), None).wait()?;
    let view = Picture { width: 8, height: 6, pixels: vec![200; 8 * 6 * 4] };
    let model = Mask { width: 8, height: 6, values: vec![255; 48] };
    p.run("picture", &json!({}), Some(Capture { view, model, selection: None })).wait()?;
    let results = p.activity.take_results();
    assert_eq!(results.len(), 2);
    assert_eq!((results[1].width, results[1].height), (8, 6));
    assert_eq!(shapecraft_plugin_host::results::list(&installed.dir).len(), 2);

    // The local network (allowed) and the internet (not in its permissions).
    let port = pong_server();
    let out = p.run("ping", &json!({"server": format!("http://127.0.0.1:{port}")}), None).wait()?;
    assert!(out.contains("pong"), "{out}");
    let err = p.run("ping", &json!({"server": "https://example.com"}), None).wait();
    assert!(matches!(&err, Err(Error::Plugin(m)) if m.contains("may not reach")), "{err:?}");

    // A panic in the plugin is an error, not a crash, and the plugin keeps working after it.
    let err = p.run("panic", &json!({}), None).wait();
    assert!(matches!(err, Err(Error::Trap(_))), "{err:?}");
    assert!(p.run("greet", &json!({}), None).wait().is_ok());

    // Cancel stops a plugin that never finishes; one action at a time.
    let t = p.run("spin", &json!({}), None);
    std::thread::sleep(Duration::from_millis(200));
    assert!(p.busy());
    assert!(p.run("greet", &json!({}), None).wait().is_err());
    p.cancel();
    let err = t.wait();
    assert!(matches!(err, Err(Error::Cancelled) | Err(Error::Plugin(_))), "{err:?}");
    assert!(!p.busy());

    lib.remove("hello")?;
    assert!(lib.list().is_empty());
    Ok(())
}

#[test]
fn installs_from_the_index() -> Result<(), String> {
    let Some(pkgs) = packages() else { return Ok(()) };
    // SAFETY-free: a test-only variable naming a folder instead of GitHub.
    unsafe { std::env::set_var("SHAPECRAFT_PLUGIN_INDEX", &pkgs) };
    let idx = index::fetch("0.0.0")?;
    let hello = idx.plugins.iter().find(|e| e.manifest.id == "hello").ok_or("hello isn't listed")?;
    let pkg = index::download("0.0.0", hello)?;
    let lib = Library::new(scratch("index"));
    lib.install(&pkg, false).map_err(|e| e.to_string())?;
    // A changed byte is refused.
    let mut bad = hello.clone();
    bad.sha256 = "0".repeat(64);
    assert!(index::download("0.0.0", &bad).is_err());
    Ok(())
}
