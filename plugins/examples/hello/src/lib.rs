//! The example plugin: everything a plugin can do, in a few lines.

use shapecraft_plugin::{Guest, Panel, Values, host, http, json, ui};

struct Hello;

impl Guest for Hello {
    fn start() -> Result<(), String> {
        host::log(host::Level::Info, "hello plugin started");
        Ok(())
    }

    fn panel(id: String, values: String) -> Result<String, String> {
        let v = Values::parse(&values);
        match id.as_str() {
            "main" => Ok(Panel::new("Hello")
                .with(ui::text("name", "Your name"))
                .with(ui::toggle("loud", "Shout", false))
                .with(ui::primary("greet", "Say Hello", "greet", false))
                .with(ui::button("shot", "Picture of the View", "picture"))
                .with_if(!v.str("name").is_empty(), ui::note(&format!("Ready to greet {}.", v.str("name")), "dim"))
                .with(ui::results())
                .json()),
            "settings" => {
                Ok(Panel::new("Hello").with(ui::text("server", "http://127.0.0.1:8188")).with(ui::button("ping", "Test", "ping")).json())
            }
            other => Err(format!("no panel \"{other}\"")),
        }
    }

    fn run(action: String, values: String) -> Result<String, String> {
        let v = Values::parse(&values);
        match action.as_str() {
            "greet" => {
                let name = if v.str("name").is_empty() { "there".to_string() } else { v.str("name") };
                let text = format!("Hello, {name}!");
                let count = host::setting("count").and_then(|c| c.parse::<u64>().ok()).unwrap_or(0) + 1;
                host::set_setting("count", Some(&count.to_string()))?;
                shapecraft_plugin::message(&if v.bool("loud") { text.to_uppercase() } else { text })
            }
            "picture" => {
                // The view when there is one, a gradient otherwise.
                let pic = match host::capture() {
                    Some(c) => c.view,
                    None => {
                        let (w, h) = (64u32, 48u32);
                        let mut px = Vec::with_capacity((w * h * 4) as usize);
                        for y in 0..h {
                            for x in 0..w {
                                px.extend_from_slice(&[(x * 4) as u8, (y * 5) as u8, 200, 255]);
                            }
                        }
                        host::Image { width: w, height: h, pixels: px }
                    }
                };
                host::progress(0.5, "Keeping the picture");
                host::add_result(&pic, "View", "made by the Hello plugin");
                shapecraft_plugin::message("Kept a picture.")
            }
            "ping" => {
                let server = if v.str("server").is_empty() { "http://127.0.0.1:8188".to_string() } else { v.str("server") };
                let r = http::get(&format!("{server}/ping"), &[])?;
                Ok(json!({ "message": format!("{server} answered {}: {}", r.status, String::from_utf8_lossy(&r.body)) }).to_string())
            }
            "spin" => {
                // Runs until cancelled (tests Cancel and the time limits).
                let mut n: u64 = 0;
                loop {
                    n = n.wrapping_add(1);
                    if n.is_multiple_of(1_000_000) && !host::progress(-1.0, "Spinning") {
                        return Err("stopped".into());
                    }
                }
            }
            "panic" => Err(format!("{}", [1, 2, 3][v.str("i").len() + 5])),
            other => Err(format!("no action \"{other}\"")),
        }
    }
}

shapecraft_plugin::export!(Hello with_types_in shapecraft_plugin::bindings);
