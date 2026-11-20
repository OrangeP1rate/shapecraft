//! `plugin.toml`: who a plugin is, what it may do, and what it adds to the app. Read without running any plugin code,
//! so the person sees the permissions before anything is installed.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The API version this ShapeCraft speaks (the `wit/` package version, major.minor).
pub const API: (u32, u32) = (0, 1);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Lowercase letters, digits and dashes ("generative-render"). Unique in the registry.
    pub id: String,
    /// What people see ("Generative Render").
    pub name: String,
    /// Semantic version of the plugin ("0.1.0").
    pub version: String,
    /// The plugin API it was built for ("0.1").
    pub api: String,
    /// One or two sentences for the Browse list.
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub repository: String,
    /// One of the icons the app draws (see `ICONS`); anything else shows the generic plugin icon.
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub commands: Vec<CommandDef>,
}

/// What the plugin may ask the host for. Anything not listed here is refused.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Permissions {
    /// Internet hosts it may talk to over HTTPS ("openrouter.ai", "api.openai.com"). A leading "*." allows
    /// subdomains ("*.example.com").
    #[serde(default)]
    pub network: Vec<String>,
    /// This computer (localhost) and the local network (10.x, 172.16-31.x, 192.168.x, *.local), any port, http or
    /// https: for AI servers people run themselves.
    #[serde(default)]
    pub local_network: bool,
    /// The view as a picture when one of its buttons is pressed.
    #[serde(default)]
    pub capture: bool,
    /// Signing in through the browser (with one of the `network` hosts).
    #[serde(default)]
    pub sign_in: bool,
}

/// A command the plugin adds: in command search, the control channel and MCP, and optionally in a place in the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDef {
    /// Unique within the plugin ("open"). The full id is `plugin.<plugin id>.<id>`.
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub help: String,
    /// Opens this panel ("main")…
    #[serde(default)]
    pub panel: String,
    /// …or runs this action.
    #[serde(default)]
    pub action: String,
    /// Where it also shows: "visualization" (the Generative Render button in Visualization's right column).
    #[serde(default)]
    pub place: String,
}

/// Icons the app can draw for a plugin.
pub const ICONS: [&str; 4] = ["plugin", "sparkle", "image", "cube"];

/// Places a command can show.
pub const PLACES: [&str; 2] = ["", "visualization"];

impl Manifest {
    pub fn parse(text: &str) -> Result<Self> {
        let m: Manifest = toml::from_str(text).map_err(|e| Error::Manifest(e.message().to_string()))?;
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<()> {
        let bad = |what: String| Err(Error::Manifest(what));
        if !valid_id(&self.id) {
            return bad(format!("id \"{}\" must be 2-40 lowercase letters, digits or dashes", self.id));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 40 {
            return bad("name must be 1-40 characters".into());
        }
        if parse_version(&self.version).is_none() {
            return bad(format!("version \"{}\" isn't major.minor.patch", self.version));
        }
        let Some(api) = parse_api(&self.api) else { return bad(format!("api \"{}\" isn't major.minor", self.api)) };
        if !api_supported(api) {
            return Err(Error::Api(self.api.clone()));
        }
        if self.description.chars().count() > 300 {
            return bad("description is longer than 300 characters".into());
        }
        for h in &self.permissions.network {
            if !valid_host_pattern(h) {
                return bad(format!("network host \"{h}\" isn't a host name (no scheme, port or path)"));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.commands {
            if !valid_id(&c.id) || !seen.insert(c.id.as_str()) {
                return bad(format!("command id \"{}\" is invalid or repeated", c.id));
            }
            if c.label.trim().is_empty() {
                return bad(format!("command \"{}\" has no label", c.id));
            }
            if c.panel.is_empty() == c.action.is_empty() {
                return bad(format!("command \"{}\" needs exactly one of panel or action", c.id));
            }
            if !PLACES.contains(&c.place.as_str()) {
                return bad(format!("command \"{}\": unknown place \"{}\"", c.id, c.place));
            }
        }
        Ok(())
    }

    /// The full command id the registry, search and MCP use.
    pub fn command_id(&self, c: &CommandDef) -> String {
        format!("plugin.{}.{}", self.id, c.id)
    }

    /// The permissions as lines for the person to read before adding the plugin.
    pub fn permission_lines(&self) -> Vec<String> {
        let p = &self.permissions;
        let mut out = Vec::new();
        if !p.network.is_empty() {
            out.push(format!("Talk to {}", join_names(&p.network)));
        }
        if p.local_network {
            out.push("Talk to AI servers on this computer or your local network".to_string());
        }
        if p.capture {
            out.push("See a picture of the view when you press its buttons".to_string());
        }
        if p.sign_in {
            out.push("Sign you in through your browser".to_string());
        }
        if out.is_empty() {
            out.push("Nothing beyond its own settings".to_string());
        }
        out
    }
}

fn join_names(v: &[String]) -> String {
    match v {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

pub fn valid_id(id: &str) -> bool {
    (2..=40).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

fn valid_host_pattern(h: &str) -> bool {
    let name = h.strip_prefix("*.").unwrap_or(h);
    !name.is_empty()
        && name.len() <= 253
        && name.contains('.')
        && name.split('.').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// "1.2.3" → (1, 2, 3).
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.split('.').map(|p| p.parse::<u32>().ok());
    let out = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(out)
}

fn parse_api(v: &str) -> Option<(u32, u32)> {
    let (a, b) = v.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Same major, and (for 0.x, where every minor can break) the same minor; otherwise not newer than ours.
fn api_supported((major, minor): (u32, u32)) -> bool {
    if major == 0 { (major, minor) == API } else { major == API.0 && minor <= API.1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
id = "generative-render"
name = "Generative Render"
version = "0.1.0"
api = "0.1"
description = "Pictures of your model in any scene."
icon = "sparkle"

[permissions]
network = ["openrouter.ai", "api.openai.com"]
local-network = true
capture = true
sign-in = true

[[commands]]
id = "open"
label = "Generative Render"
panel = "main"
place = "visualization"
"#;

    #[test]
    fn reads_a_good_manifest() -> Result<()> {
        let m = Manifest::parse(GOOD)?;
        assert_eq!(m.id, "generative-render");
        assert!(m.permissions.local_network && m.permissions.sign_in);
        assert_eq!(m.command_id(&m.commands[0]), "plugin.generative-render.open");
        let lines = m.permission_lines();
        assert_eq!(lines[0], "Talk to openrouter.ai and api.openai.com");
        assert_eq!(lines.len(), 4);
        Ok(())
    }

    #[test]
    fn refuses_bad_manifests() {
        let cases = [
            GOOD.replace("generative-render\"", "Generative\""),
            GOOD.replace("0.1.0", "1"),
            GOOD.replace("api = \"0.1\"", "api = \"0.2\""),
            GOOD.replace("openrouter.ai", "https://openrouter.ai"),
            GOOD.replace("place = \"visualization\"", "place = \"menu\""),
            GOOD.replace("panel = \"main\"", "panel = \"main\"\naction = \"go\""),
            GOOD.replace("icon = \"sparkle\"", "icon = \"sparkle\"\nunknown = 1"),
        ];
        for c in cases {
            assert!(Manifest::parse(&c).is_err(), "accepted:\n{c}");
        }
    }

    #[test]
    fn api_versions() {
        assert!(api_supported((0, 1)));
        assert!(!api_supported((0, 0)));
        assert!(!api_supported((1, 0)));
    }
}
