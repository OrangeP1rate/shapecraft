//! Write ShapeCraft plugins in Rust.
//!
//! ```ignore
//! use shapecraft_plugin::{Guest, Panel, Values, ui};
//!
//! struct Hello;
//!
//! impl Guest for Hello {
//!     fn start() -> Result<(), String> { Ok(()) }
//!     fn panel(id: String, values: String) -> Result<String, String> {
//!         Ok(Panel::new("Hello").with(ui::text("name", "Your name")).with(ui::button("hi", "Say Hello", "hello")).json())
//!     }
//!     fn run(action: String, values: String) -> Result<String, String> {
//!         let v = Values::parse(&values);
//!         shapecraft_plugin::message(&format!("Hello, {}!", v.str("name")))
//!     }
//! }
//!
//! shapecraft_plugin::export!(Hello with_types_in shapecraft_plugin::bindings);
//! ```
//!
//! Build with `cargo xtask plugin build <folder>` from the ShapeCraft repository; it makes the `.scplugin` package
//! the app installs. The API is `wit/shapecraft-plugin.wit`; docs/plugins.md explains panels and permissions.

/// Generated from `wit/shapecraft-plugin.wit`.
pub mod bindings {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "plugin",
        pub_export_macro: true,
        default_bindings_module: "shapecraft_plugin::bindings",
    });
}

pub use bindings::Guest;
pub use bindings::export;

pub use bindings::shapecraft::plugin::host;
pub use bindings::shapecraft::plugin::host::{Image, Mask, Request, Response, ViewCapture};
pub use serde_json::{Value, json};

/// A finished action's answer with a line for the person.
pub fn message(text: &str) -> Result<String, String> {
    Ok(json!({ "message": text }).to_string())
}

/// The values a panel sends (control id → value).
#[derive(Clone, Debug, Default)]
pub struct Values(pub serde_json::Map<String, Value>);

impl Values {
    pub fn parse(text: &str) -> Self {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => Self(m),
            _ => Self::default(),
        }
    }

    pub fn str(&self, id: &str) -> String {
        self.0.get(id).and_then(Value::as_str).unwrap_or("").to_string()
    }

    pub fn bool(&self, id: &str) -> bool {
        self.0.get(id).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn get(&self, id: &str) -> Option<&Value> {
        self.0.get(id)
    }
}

/// A panel being described.
#[derive(Clone, Debug, Default)]
pub struct Panel {
    title: String,
    controls: Vec<Value>,
}

impl Panel {
    pub fn new(title: &str) -> Self {
        Self { title: title.to_string(), controls: Vec::new() }
    }

    pub fn with(mut self, control: Value) -> Self {
        self.controls.push(control);
        self
    }

    pub fn with_if(self, cond: bool, control: Value) -> Self {
        if cond { self.with(control) } else { self }
    }

    pub fn json(&self) -> String {
        json!({ "title": self.title, "controls": self.controls }).to_string()
    }
}

/// Controls (docs/plugins.md, "Panels"). Each returns JSON that can be changed further before adding it.
pub mod ui {
    use super::{Value, json};

    pub fn text(id: &str, placeholder: &str) -> Value {
        json!({ "kind": "text", "id": id, "placeholder": placeholder })
    }

    pub fn text_box(id: &str, placeholder: &str) -> Value {
        json!({ "kind": "text", "id": id, "placeholder": placeholder, "multiline": true })
    }

    pub fn secret(id: &str, label: &str, placeholder: &str) -> Value {
        json!({ "kind": "secret", "id": id, "label": label, "placeholder": placeholder })
    }

    pub fn toggle(id: &str, label: &str, value: bool) -> Value {
        json!({ "kind": "toggle", "id": id, "label": label, "value": value })
    }

    pub fn segmented(id: &str, options: &[&str], value: &str) -> Value {
        json!({ "kind": "segmented", "id": id, "options": options, "value": value })
    }

    /// A choice for [`menu`].
    pub fn choice(value: &str, label: &str, group: &str, detail: &str) -> Value {
        json!({ "value": value, "label": label, "group": group, "detail": detail })
    }

    pub fn menu(id: &str, label: &str, options: Vec<Value>, value: &str, empty: &str) -> Value {
        json!({ "kind": "menu", "id": id, "label": label, "options": options, "value": value, "empty": empty })
    }

    pub fn button(id: &str, label: &str, action: &str) -> Value {
        json!({ "kind": "button", "id": id, "label": label, "action": action })
    }

    /// The blue button; `capture` takes a picture of the view first.
    pub fn primary(id: &str, label: &str, action: &str, capture: bool) -> Value {
        json!({ "kind": "button", "id": id, "label": label, "action": action, "primary": true, "capture": capture })
    }

    /// tone: "dim", "normal", "good", "warning" or "error".
    pub fn note(text: &str, tone: &str) -> Value {
        json!({ "kind": "note", "text": text, "tone": tone })
    }

    pub fn heading(text: &str) -> Value {
        json!({ "kind": "heading", "text": text })
    }

    pub fn link(label: &str, url: &str) -> Value {
        json!({ "kind": "link", "label": label, "url": url })
    }

    pub fn results() -> Value {
        json!({ "kind": "results" })
    }

    pub fn separator() -> Value {
        json!({ "kind": "separator" })
    }
}

/// HTTP through the host (only to hosts in the plugin's permissions).
pub mod http {
    use super::host::{Header, Request, Response, fetch};
    use super::{Value, host};

    pub fn headers(pairs: &[(&str, &str)]) -> Vec<Header> {
        pairs.iter().map(|(n, v)| Header { name: (*n).to_string(), value: (*v).to_string() }).collect()
    }

    pub fn send(method: &str, url: &str, headers: Vec<Header>, body: Vec<u8>, timeout: u32) -> Result<Response, String> {
        fetch(&Request { method: method.to_string(), url: url.to_string(), headers, body, timeout })
    }

    pub fn get(url: &str, h: &[(&str, &str)]) -> Result<Response, String> {
        send("GET", url, headers(h), Vec::new(), 30)
    }

    /// POSTs JSON; `timeout` in seconds.
    pub fn post_json(url: &str, h: &[(&str, &str)], body: &Value, timeout: u32) -> Result<Response, String> {
        let mut hs = headers(h);
        hs.push(Header { name: "Content-Type".into(), value: "application/json".into() });
        send("POST", url, hs, body.to_string().into_bytes(), timeout)
    }

    /// One part of a multipart form: a field, or a file when `filename` is set.
    pub struct Part<'a> {
        pub name: &'a str,
        pub filename: Option<&'a str>,
        pub content_type: Option<&'a str>,
        pub data: &'a [u8],
    }

    /// POSTs a multipart/form-data body.
    pub fn post_multipart(url: &str, h: &[(&str, &str)], parts: &[Part], timeout: u32) -> Result<Response, String> {
        let token = hex(&host::random(12));
        let boundary = format!("----shapecraft{token}");
        let mut body = Vec::new();
        for p in parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match p.filename {
                Some(f) => {
                    body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"; filename=\"{f}\"\r\n", p.name).as_bytes())
                }
                None => body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"\r\n", p.name).as_bytes()),
            }
            if let Some(ct) = p.content_type {
                body.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
            }
            body.extend_from_slice(b"\r\n");
            body.extend_from_slice(p.data);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let mut hs = headers(h);
        hs.push(Header { name: "Content-Type".into(), value: format!("multipart/form-data; boundary={boundary}") });
        send("POST", url, hs, body, timeout)
    }

    /// The body as JSON.
    pub fn json(r: &Response) -> Result<Value, String> {
        serde_json::from_slice(&r.body).map_err(|_| format!("the answer wasn't JSON (HTTP {})", r.status))
    }

    pub fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Signing in with OAuth + PKCE.
pub mod oauth {
    use super::host;

    /// base64url without padding.
    pub fn b64url(bytes: &[u8]) -> String {
        host::base64_encode(bytes).trim_end_matches('=').replace('+', "-").replace('/', "_")
    }

    /// A fresh PKCE verifier and its S256 challenge.
    pub fn pkce() -> (String, String) {
        let verifier = b64url(&host::random(48));
        let challenge = b64url(&host::sha256(verifier.as_bytes()));
        (verifier, challenge)
    }

    /// The value of `key` in a query string (`code=…&state=…`), percent-decoded.
    pub fn query_value(query: &str, key: &str) -> Option<String> {
        query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k == key).then(|| decode(v))
        })
    }

    fn decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < b.len() => match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                },
                c => out.push(c),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Percent-encodes a query value.
    pub fn encode(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    }
}
