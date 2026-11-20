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
    let listed = idx.plugins.iter().find(|e| e.manifest.id == "generative-render").ok_or("generative-render isn't listed")?;
    assert!(idx.plugins.iter().all(|e| e.manifest.id != "hello"), "examples aren't listed");
    let pkg = index::download("0.0.0", listed)?;
    let lib = Library::new(scratch("index"));
    lib.install(&pkg, false).map_err(|e| e.to_string())?;
    // A changed byte is refused.
    let mut bad = listed.clone();
    bad.sha256 = "0".repeat(64);
    assert!(index::download("0.0.0", &bad).is_err());
    Ok(())
}

/// A stand-in for ComfyUI: answers the calls Generative Render makes, with a solid blue "generated" picture.
fn fake_comfy(seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> u16 {
    let Ok(l) = TcpListener::bind(("127.0.0.1", 0)) else { return 0 };
    let port = l.local_addr().map(|a| a.port()).unwrap_or(0);
    let scene = Picture { width: 20, height: 20, pixels: [0u8, 0, 255, 255].repeat(400) };
    let png = shapecraft_plugin_host::images::encode_png(&scene).unwrap_or_default();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut s = s;
            let mut buf = Vec::new();
            let mut chunk = [0u8; 65536];
            // Headers, then as much body as Content-Length says.
            loop {
                let Ok(n) = s.read(&mut chunk) else { break };
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
                    let len = head.lines().find_map(|l| l.strip_prefix("content-length:")?.trim().parse::<usize>().ok()).unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&buf).to_string();
            let line = text.lines().next().unwrap_or("").to_string();
            if let Ok(mut v) = seen.lock() {
                v.push(if line.starts_with("POST /prompt") { text.clone() } else { line.clone() });
            }
            let path = line.split_whitespace().nth(1).unwrap_or("");
            let loader =
                |node: &str, input: &str, files: &[&str]| json!({ node: { "input": { "required": { input: [files] } } } }).to_string();
            let (ctype, body): (&str, Vec<u8>) = if path.starts_with("/object_info/UNETLoader") {
                ("application/json", loader("UNETLoader", "unet_name", &["flux1-dev-kontext_fp8_scaled.safetensors"]).into_bytes())
            } else if path.starts_with("/object_info/DualCLIPLoader") {
                (
                    "application/json",
                    loader("DualCLIPLoader", "clip_name1", &["clip_l.safetensors", "t5xxl_fp8_e4m3fn_scaled.safetensors"]).into_bytes(),
                )
            } else if path.starts_with("/object_info/VAELoader") {
                ("application/json", loader("VAELoader", "vae_name", &["ae.safetensors"]).into_bytes())
            } else if let Some(node) = path.strip_prefix("/object_info/") {
                ("application/json", json!({ node: {} }).to_string().into_bytes())
            } else if path.starts_with("/upload/image") {
                ("application/json", br#"{"name":"view.png","subfolder":"","type":"input"}"#.to_vec())
            } else if path.starts_with("/prompt") {
                ("application/json", br#"{"prompt_id":"p1"}"#.to_vec())
            } else if path.starts_with("/history/p1") {
                ("application/json", br#"{"p1":{"status":{"status_str":"success"},"outputs":{"13":{"images":[{"filename":"out.png","subfolder":"","type":"output"}]}}}}"#.to_vec())
            } else if path.starts_with("/view") {
                ("image/png", png.clone())
            } else {
                ("text/plain", b"?".to_vec())
            };
            let _ = s.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                    .as_bytes(),
            );
            let _ = s.write_all(&body);
        }
    });
    port
}

#[test]
fn generative_render_with_a_local_comfyui() -> Result<(), Error> {
    let Some(pkgs) = packages() else { return Ok(()) };
    let lib = Library::new(scratch("genrender"));
    let installed = lib.install(&library::read_package(&pkgs.join("generative-render-0.1.0.scplugin"))?, false)?;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let port = fake_comfy(seen.clone());
    let settings = json!({ "comfy_on": true, "comfy_url": format!("http://127.0.0.1:{port}") }).to_string();
    shapecraft_plugin_host::store::KeyFile::settings(&installed.dir).set("panel.settings", Some(&settings)).map_err(Error::Io)?;
    let p = lib.start(&installed)?;

    // The sheet: the picture, the prompt bar, the model menu offering the local model.
    let panel = Panel::parse(&p.panel("main", &json!({})).wait()?).map_err(Error::Plugin)?;
    assert_eq!(panel.style, "sheet");
    assert!(panel.controls.iter().any(|c| matches!(c, shapecraft_plugin_host::Control::Stage {})));
    let menu = panel.find("model").ok_or(Error::Plugin("no model menu".into()))?;
    assert!(matches!(menu, shapecraft_plugin_host::Control::Menu { options, .. } if options.iter().any(|o| o.value == "comfy:kontext")));
    assert!(Panel::parse(&p.panel("settings", &json!({"comfy_on": true})).wait()?).is_ok());

    // Test Connection reports what it found (this fake has no /system_stats, so it says so plainly).
    let _ = p.run("comfy_test", &json!({ "comfy_url": format!("http://127.0.0.1:{port}") }), None).wait();

    // Generate: red model on the left half of a 10×10 view; the fake "AI" paints everything blue.
    let view = Picture { width: 10, height: 10, pixels: [255u8, 0, 0, 255].repeat(100) };
    let model = Mask { width: 10, height: 10, values: (0..100).map(|i| if i % 10 < 5 { 255 } else { 0 }).collect() };
    let values = json!({ "model": "comfy:kontext", "mode": "Surroundings", "exact": true, "prompt": "on an oak desk", "count": "1" });
    let out = p.run("generate", &values, Some(Capture { view, model, selection: None })).wait()?;
    assert!(out.contains("Done"), "{out}");
    let results = p.activity.take_results();
    assert_eq!(results.len(), 1);
    let pic = shapecraft_plugin_host::images::decode(&std::fs::read(&results[0].path).map_err(|e| Error::Io(e.to_string()))?)
        .map_err(Error::Plugin)?;
    // Kept exact: the model's own red on the left, the new scene's blue on the right, at the view's size.
    assert_eq!((pic.width, pic.height), (10, 10));
    assert_eq!(&pic.pixels[0..4], &[255, 0, 0, 255]);
    assert_eq!(&pic.pixels[9 * 4..10 * 4], &[0, 0, 255, 255]);
    assert!(results[0].details.contains("model kept exact"), "{}", results[0].details);
    // The workflow carried the person's words and the uploaded picture.
    let log = seen.lock().map(|v| v.join("\n")).unwrap_or_default();
    assert!(log.contains("on an oak desk") && log.contains("view.png") && log.contains("flux1-dev-kontext"), "{log}");

    // Selected Faces without a selection says what to do.
    let err = p
        .run(
            "generate",
            &json!({ "model": "comfy:kontext", "mode": "Selected Faces" }),
            Some(Capture {
                view: Picture { width: 4, height: 4, pixels: vec![9; 64] },
                model: Mask { width: 4, height: 4, values: vec![0; 16] },
                selection: None,
            }),
        )
        .wait();
    assert!(matches!(&err, Err(Error::Plugin(m)) if m.contains("Pick the faces")), "{err:?}");
    Ok(())
}
