# Plugins

Plugins add features people can choose to have: Generative Render is one. They're added from **Preferences ▸
Plugins ▸ Browse**, and they can be switched off or removed there at any time (removing one also removes its
settings, keys and pictures).

A plugin is a small WebAssembly program. ShapeCraft runs it sealed off: it can't read your files, open network
connections, see other plugins or touch the model on its own. Everything it does goes through a short list of
functions ShapeCraft provides, and each of those is checked against the permissions the plugin asked for. Those
permissions are shown before you add it.

- What's here: [How plugins work](#how-plugins-work) · [Write one](#write-one) · [Panels](#panels) ·
  [Permissions](#permissions) · [Build, test, share](#build-test-share) · [Submitting a plugin](#submitting-a-plugin)
- Generative Render, for people using it: [generative-render.md](generative-render.md)

## How plugins work

| Piece | Where |
|---|---|
| The API: what a plugin provides and what it may call | [`wit/shapecraft-plugin.wit`](../wit/shapecraft-plugin.wit) |
| The host: runs plugins, checks permissions, keeps their settings | [`crates/plugin-host`](../crates/plugin-host) |
| The Rust SDK for writing plugins | [`plugins/sdk`](../plugins/sdk) |
| A small example covering every feature | [`plugins/examples/hello`](../plugins/examples/hello) |
| ShapeCraft's own plugins | [`plugins/official`](../plugins/official) |
| Plugins by others, reviewed | [`plugins/community`](../plugins/community) |

- **Runtime.** wasmtime's Pulley interpreter, with no compiler inside the app: plugins arrive precompiled. That keeps
  the app small and lets the same plugin run on macOS, Windows, Linux, Android and iOS. Plugins do I/O-bound work
  (talking to services), and heavy picture work (decoding, scaling, compositing) is done by the host natively.
- **Three functions.** A plugin exports `start`, `panel(id, values)` (describe a panel as JSON) and
  `run(action, values)` (do something when a button is pressed or a command runs).
- **Two instances.** Each plugin runs twice, side by side: one answers panels, one runs actions. A minute-long
  generation never freezes its settings. Whatever needs to survive goes in settings (`setting`/`set-setting`),
  which both instances share.
- **Never crash.** A panic, a runaway loop (Cancel or a time limit stops it) or running out of memory (1 GB) ends
  only that call. The plugin restarts fresh and the app carries on.
- **Versions.** A plugin names the API it was built for (`api = "0.1"`). In 0.x every minor version may change the
  API, so the app loads only plugins built for its own minor version. The registry rebuilds every listed plugin for
  each ShapeCraft release.

## Write one

1. Copy `plugins/examples/hello` to `plugins/community/<your-id>` and rename it in `Cargo.toml` and `plugin.toml`.
2. Write the three functions:

```rust
use shapecraft_plugin::{Guest, Panel, Values, host, ui};

struct MyPlugin;

impl Guest for MyPlugin {
    fn start() -> Result<(), String> {
        Ok(())
    }

    fn panel(id: String, values: String) -> Result<String, String> {
        let v = Values::parse(&values);
        Ok(Panel::new("My Plugin")
            .with(ui::text("name", "Your name"))
            .with(ui::primary("go", "Say Hello", "greet", false))
            .with_if(!v.str("name").is_empty(), ui::note("Ready.", "dim"))
            .json())
    }

    fn run(action: String, values: String) -> Result<String, String> {
        let v = Values::parse(&values);
        match action.as_str() {
            "greet" => shapecraft_plugin::message(&format!("Hello, {}!", v.str("name"))),
            other => Err(format!("no action {other}")),
        }
    }
}

shapecraft_plugin::export!(MyPlugin with_types_in shapecraft_plugin::bindings);
```

3. Describe it in `plugin.toml`:

```toml
id = "my-plugin"              # lowercase letters, digits, dashes
name = "My Plugin"
version = "0.1.0"
api = "0.1"
description = "One or two sentences for the Browse list."
authors = ["your-github-name"]
license = "MIT OR Apache-2.0"
repository = "https://github.com/you/my-plugin"
icon = "plugin"               # plugin, sparkle, image or cube

[permissions]
network = ["api.example.com"] # internet hosts, HTTPS only; "*.example.com" for subdomains
local-network = false         # localhost and the local network (AI servers people run themselves)
capture = false               # a picture of the view when one of its buttons asks
sign-in = false               # signing in through the browser (OAuth) with a `network` host

[[commands]]
id = "open"
label = "My Plugin"
help = "Shown in command search"
panel = "main"                # open this panel… (or: action = "…" to run an action)
```

Errors you return from `run` are shown to the person as they are, so write them as plain sentences that say what
to do ("Add an API key in Preferences ▸ Plugins first.").

### What the host gives you

All in `shapecraft_plugin::host` (see the WIT file for details):

| Function | |
|---|---|
| `fetch(request)` | HTTP, only to permitted hosts; no redirects followed. Helpers: `http::get`, `http::post_json`, `http::post_multipart`, `http::json` |
| `secret` / `set-secret` | API keys and tokens, kept apart from settings, readable only by this user |
| `setting` / `set-setting` | JSON values. A panel's values are kept under `panel.<id>` automatically |
| `capture()` | The view (RGBA) and a mask of where the model is (a mask of the picked faces is planned; `selection` is empty for now). Only for buttons with `capture: true` |
| `progress(fraction, text)` | Shown while an action runs; returns false once the person pressed Cancel |
| `wait(ms)` | For polling a server; returns false on Cancel |
| `decode-image`, `encode-png`, `encode-jpeg`, `fit`, `composite` | Picture work, done natively |
| `base64-encode/decode`, `sha256`, `random`, `now` | |
| `add-result(picture, title, details)` | Keeps a picture; it shows under the panel with Save and Copy |
| `sign-in(url)` | OAuth with a loopback redirect: put `{redirect}` in the URL; you get the query string back. Helpers: `oauth::pkce`, `oauth::query_value` |
| `open-url(url)` | Opens an https page in the browser |
| `log(level, text)` | Help ▸ Show Log |

## Panels

A panel is JSON the plugin returns from `panel(id, values)`. ShapeCraft draws every control, so plugins look like
the rest of the app, and touch, pen, keyboard and the numpad all just work. `"main"` is the panel a command opens.
`"settings"` shows under the plugin in Preferences ▸ Plugins.

```json
{ "title": "My Plugin", "style": "", "controls": [ … ] }
```

| `kind` | Fields | Value sent back |
|---|---|---|
| `text` | `id`, `label`, `placeholder`, `multiline` | the text |
| `secret` | `id`, `label`, `placeholder` | — (saved by the app; read it with `secret(id)`) |
| `toggle` | `id`, `label`, `value` | true/false |
| `segmented` | `id`, `label`, `options`, `value` | the picked option |
| `menu` | `id`, `label`, `options` (`value`, `label`, `group`, `detail`), `value`, `empty` | the picked value |
| `button` | `id`, `label`, `action`, `primary`, `capture`, `enabled` | — (runs `action`) |
| `note` | `text`, `tone` (dim, normal, good, warning, error) | |
| `heading`, `separator`, `link` (`label`, `url`) | | |
| `results` | | pictures from `add-result` |
| `stage` | | (sheets) the view, then the newest result |

The app asks for the panel again whenever a value changes and after every action, so a panel can depend on its
values (show a field only when a toggle is on). Panel calls must answer within 5 seconds: no network in `panel`.
Fetch in an action and keep the answer in a setting.

`"style": "sheet"` opens the panel as a card in the middle of the window. The `stage` picture comes first, then the
first `text` box and the first `primary` button as a bar over its bottom edge, then menus on their own line and the
other controls in a row. Generative Render uses it.

## Permissions

Ask for as little as the plugin needs. The person reads the list before adding it, in words like "Talk to
api.example.com" and "See a picture of the view when you press its buttons". A plugin asking for more than its
description explains won't be accepted into the list.

There is deliberately no file access, no raw sockets and no access to the document beyond the captured view. Tools
that change the model will come through the command registry (so undo and history keep working) in a later API
version.

## Build, test, share

```sh
rustup target add wasm32-unknown-unknown
cargo xtask plugin build my-plugin     # → target/plugins/my-plugin-0.1.0.scplugin
cargo xtask plugin test                # builds everything and runs the host's end-to-end tests
```

To try it, use **Preferences ▸ Plugins ▸ Add from File…** and pick the `.scplugin`. Plugins added from a file are
marked as such, and the app says so before adding them. To drive it from a script, use the control channel
(`plugins.install`, `plugins.panel`, `plugins.run`; see [mcp.md](mcp.md)).

A `.scplugin` is precompiled for one ShapeCraft runtime. A file built against a different release is refused with
"can't be loaded by this version". Rebuild it with that release's `cargo xtask plugin build`.

## Submitting a plugin

Open a pull request that adds `plugins/community/<id>/` (source, `plugin.toml`, `README.md`). CI builds it from
source for every release, so nobody ships a binary. What's checked is in [plugin-review.md](plugin-review.md).
Accepted plugins appear in Browse under "From the Community" with the next release, credited to you.
