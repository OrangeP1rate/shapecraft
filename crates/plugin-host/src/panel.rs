//! Panels: what a plugin shows, described as JSON and drawn by the app (docs/plugins.md, "Panels").
//!
//! ```json
//! { "title": "Generative Render",
//!   "controls": [
//!     { "id": "mode", "kind": "segmented", "options": ["Surroundings", "Model"], "value": "Surroundings" },
//!     { "id": "prompt", "kind": "text", "multiline": true, "placeholder": "Describe the scene" },
//!     { "id": "go", "kind": "button", "label": "Generate", "action": "generate", "primary": true, "capture": true }
//!   ] }
//! ```
//! Values the person sets are kept by the app and sent back to the plugin as `{ "mode": "Model", … }`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    #[serde(default)]
    pub title: String,
    /// "" (a column of controls) or "sheet" (a card in the middle of the window: the [`Control::Stage`] picture,
    /// the first text box and the blue button as a bar over its bottom edge, the rest in a row under it).
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub controls: Vec<Control>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    #[serde(default)]
    pub label: String,
    /// Shown as a heading over the choices that share it ("OpenRouter", "On this computer").
    #[serde(default)]
    pub group: String,
    /// A short note on the right ("$0.04", "free").
    #[serde(default)]
    pub detail: String,
}

impl Choice {
    pub fn shown(&self) -> &str {
        if self.label.is_empty() { &self.value } else { &self.label }
    }
}

/// What a button would spend, in words the person reads before allowing it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spend {
    /// Whose account pays ("OpenAI").
    #[serde(default)]
    pub with: String,
    /// What it makes ("2 pictures with GPT Image 2").
    #[serde(default)]
    pub what: String,
    /// The provider's price when known ("about $0.03 each"), else empty.
    #[serde(default)]
    pub estimate: String,
    /// One more line, like the description being sent.
    #[serde(default)]
    pub detail: String,
}

impl Spend {
    /// Caps each field so a plugin can't fill the card.
    pub fn tidy(mut self) -> Self {
        for (f, n) in [(&mut self.with, 60), (&mut self.what, 160), (&mut self.estimate, 80), (&mut self.detail, 300)] {
            *f = f.chars().filter(|c| !c.is_control()).take(n).collect();
        }
        self
    }
}

/// How a note reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    #[default]
    Dim,
    Normal,
    Good,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Control {
    /// A line of text, or a box of several lines.
    Text {
        id: String,
        #[serde(default)]
        label: String,
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        multiline: bool,
        #[serde(default)]
        value: String,
    },
    /// A secret (an API key): typed once, kept by the app, never shown again. Its value is true once saved.
    Secret {
        id: String,
        #[serde(default)]
        label: String,
        #[serde(default)]
        placeholder: String,
    },
    Toggle {
        id: String,
        label: String,
        #[serde(default)]
        value: bool,
    },
    /// A row of segments, one picked.
    Segmented {
        id: String,
        #[serde(default)]
        label: String,
        options: Vec<String>,
        #[serde(default)]
        value: String,
    },
    /// A pop-up menu of choices, grouped.
    Menu {
        id: String,
        #[serde(default)]
        label: String,
        options: Vec<Choice>,
        #[serde(default)]
        value: String,
        /// Shown when nothing can be picked.
        #[serde(default)]
        empty: String,
    },
    Button {
        id: String,
        label: String,
        /// The action `run` is called with.
        action: String,
        /// The blue one.
        #[serde(default)]
        primary: bool,
        /// Capture the view first (needs the `capture` permission).
        #[serde(default)]
        capture: bool,
        #[serde(default)]
        enabled: Option<bool>,
        /// Pressing it costs the person money (a paid service on their account). An assistant driving the app
        /// (control channel, MCP) can't press it without the person's Allow in the app.
        #[serde(default)]
        spends: Option<Spend>,
    },
    Note {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    Heading {
        text: String,
    },
    /// Opens an https:// page in the browser.
    Link {
        label: String,
        url: String,
    },
    /// Where the pictures this plugin made show up (one per panel; at the end when left out).
    Results {},
    /// A sheet's picture: the view when the panel opened, replaced by the newest result.
    Stage {},
    /// A thin line between sections.
    Separator {},
}

/// The most controls a panel may have (anything past this is dropped).
pub const MAX_CONTROLS: usize = 60;

impl Control {
    pub fn id(&self) -> Option<&str> {
        match self {
            Control::Text { id, .. }
            | Control::Secret { id, .. }
            | Control::Toggle { id, .. }
            | Control::Segmented { id, .. }
            | Control::Menu { id, .. }
            | Control::Button { id, .. } => Some(id),
            _ => None,
        }
    }

    /// The value it starts with, before the person changes anything.
    pub fn default_value(&self) -> Option<Value> {
        match self {
            Control::Text { value, .. } => Some(Value::from(value.clone())),
            Control::Toggle { value, .. } => Some(Value::from(*value)),
            Control::Segmented { value, options, .. } => {
                Some(Value::from(if value.is_empty() { options.first().cloned().unwrap_or_default() } else { value.clone() }))
            }
            Control::Menu { value, options, .. } => Some(Value::from(if value.is_empty() {
                options.first().map(|c| c.value.clone()).unwrap_or_default()
            } else {
                value.clone()
            })),
            _ => None,
        }
    }
}

impl Panel {
    /// Reads a plugin's JSON. Unknown control kinds are skipped (a newer plugin on an older app), not fatal.
    pub fn parse(text: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("the panel isn't valid JSON ({e})"))?;
        let title = v.get("title").and_then(Value::as_str).unwrap_or("").chars().take(80).collect();
        let style = v.get("style").and_then(Value::as_str).filter(|s| *s == "sheet").unwrap_or("").to_string();
        let mut controls = Vec::new();
        for c in v.get("controls").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            if controls.len() == MAX_CONTROLS {
                break;
            }
            if let Ok(c) = serde_json::from_value::<Control>(c.clone()) {
                controls.push(c);
            }
        }
        Ok(Panel { title, style, controls })
    }

    pub fn find(&self, id: &str) -> Option<&Control> {
        self.controls.iter().find(|c| c.id() == Some(id))
    }

    /// The button that runs `action`: whether it captures the view and what it spends.
    pub fn button_for(&self, action: &str) -> Option<(bool, Option<Spend>)> {
        self.controls.iter().find_map(|c| match c {
            Control::Button { action: a, capture, spends, .. } if a == action => Some((*capture, spends.clone().map(Spend::tidy))),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_panel_and_skips_unknown_kinds() -> Result<(), String> {
        let p = Panel::parse(
            r#"{"title":"Generative Render","controls":[
                {"id":"mode","kind":"segmented","options":["Surroundings","Model"]},
                {"id":"prompt","kind":"text","multiline":true,"placeholder":"Describe the scene"},
                {"kind":"hologram","id":"x"},
                {"id":"model","kind":"menu","options":[{"value":"a","label":"Model A","group":"OpenRouter","detail":"$0.04"}]},
                {"id":"go","kind":"button","label":"Generate","action":"generate","primary":true,"capture":true},
                {"kind":"note","text":"Hi","tone":"warning"},
                {"kind":"results"}
            ]}"#,
        )?;
        assert_eq!(p.controls.len(), 6);
        assert_eq!(p.find("mode").and_then(Control::default_value), Some(Value::from("Surroundings")));
        assert_eq!(p.find("model").and_then(Control::default_value), Some(Value::from("a")));
        assert!(matches!(p.controls[4], Control::Note { tone: Tone::Warning, .. }));
        assert!(Panel::parse("nope").is_err());
        assert_eq!(p.button_for("generate"), Some((true, None)));
        assert_eq!(p.button_for("other"), None);
        Ok(())
    }

    #[test]
    fn a_paid_button_says_what_it_spends() -> Result<(), String> {
        let p = Panel::parse(
            r#"{"controls":[{"id":"go","kind":"button","label":"Generate","action":"generate",
                "spends":{"with":"OpenAI","what":"2 pictures with GPT Image 2","estimate":""}}]}"#,
        )?;
        let (capture, spend) = p.button_for("generate").ok_or("no button")?;
        assert!(!capture);
        let s = spend.ok_or("no spend")?;
        assert_eq!((s.with.as_str(), s.what.as_str()), ("OpenAI", "2 pictures with GPT Image 2"));
        Ok(())
    }
}
