# Reviewing a community plugin

A plugin in `plugins/community/` is built from source by CI for every release and offered to everyone in Browse. We
review it like code that runs on people's computers, because it does.

## Must

- **Builds from source** with `cargo xtask plugin build <id>`, no binaries or generated code checked in, no
  `build.rs` that downloads anything. Dependencies are from crates.io with a lockfile entry.
- **Permissions match the description.** Every `network` host is one the plugin clearly needs (its service's API,
  its sign-in page). `local-network`, `capture` and `sign-in` only when a feature visibly uses them.
- **Says where data goes.** If the view or anything the person types leaves the computer, the README and the panel
  say to whom.
- **No surprises.** No analytics, tracking, ads, crypto-mining, or calls home beyond the stated service. No
  obfuscated code. No hidden actions.
- **Paid buttons say so.** Every button that can cost the person money carries `spends` (docs/plugins.md, "Panels"),
  updated for what's picked (service, model, how many), so an assistant can't spend without the person's Allow.
- **Secrets stay secrets.** Keys go through `secret`/`set-secret`, never into settings, logs, results or errors.
- **Plain, accurate text** in the manifest, panels and errors. No claims of being made by ShapeCraft or by the
  service it connects to (unless it is).
- **Licence** compatible with being distributed in our releases (MIT, Apache-2.0, BSD, MPL, GPL-compatible are
  fine), stated in `plugin.toml`.
- **Clean room:** nothing taken from other CAD products (names, icons, help text), same as the app.
- **`cargo fmt` and `cargo clippy --release -- -D warnings` pass** in `plugins/`, and `cargo xtask plugin test`
  stays green.

## Should

- Errors that say what to do next. Progress text while something takes more than a second. Cancel respected
  (check `progress`/`wait` in loops).
- Panels that read like the app: sentence case, short labels, no exclamation marks.
- A `README.md` (shown when someone adds it from a file, and on GitHub) with what it does and what it costs, if
  anything.

## Updating

A version bump in `plugin.toml` is a new review of the diff. A permissions change is called out in the PR, because
people who have it will be asked again.
