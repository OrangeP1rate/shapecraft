//! What a report says: the person's own words first, then the details they ticked, each shown before sending.
//! The home folder in any path becomes `~`, so a user name in a path doesn't travel.

/// A problem or an idea.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Problem,
    Idea,
}

/// A failed step the report is about.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Failure {
    /// The command's id ("fillet").
    pub command: String,
    /// What it was asked to do (its parameters, as JSON text).
    pub params: String,
    /// What it answered.
    pub error: String,
}

impl Failure {
    /// A short title for a report about it: "Fillet: the radius is too large for this edge".
    pub fn title(&self) -> String {
        let name = title_case(&self.command.replace('_', " "));
        let first = self.error.lines().next().unwrap_or("").trim();
        let t = if first.is_empty() { format!("{name} failed") } else { format!("{name}: {first}") };
        t.chars().take(120).collect()
    }

    /// The same failure the same way every time, so reports of one problem can be found together:
    /// "fillet · the radius is too large for this edge" (numbers and quoted names left out).
    pub fn signature(&self) -> String {
        let mut msg = String::new();
        let mut quoted = false;
        for ch in self.error.lines().next().unwrap_or("").chars() {
            if ch == '\'' || ch == '"' {
                quoted = !quoted;
                continue;
            }
            if quoted {
                continue;
            }
            if ch.is_ascii_digit() || ch == '.' {
                if !msg.ends_with('#') {
                    msg.push('#');
                }
                continue;
            }
            msg.push(ch.to_ascii_lowercase());
        }
        let msg = msg.split_whitespace().collect::<Vec<_>>().join(" ");
        format!("{} · {}", self.command, msg.chars().take(80).collect::<String>())
    }
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// What ShapeCraft knows about where it runs, gathered when the report opens.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Context {
    /// "0.10.5-alpha".
    pub version: String,
    /// "macOS 26.1 (aarch64)".
    pub system: String,
    /// "Apple M2 Pro (Metal)".
    pub graphics: String,
    /// "mouse", "touch + pen", …
    pub input: String,
    /// "1728 × 1080 pt at 2×".
    pub window: String,
    /// The last lines of this run's log, oldest first.
    pub log: Vec<String>,
    /// The failed step, when the report started from one.
    pub failure: Option<Failure>,
    /// The record of the last crash, when the report started from one.
    pub crash: Option<String>,
}

/// Which details go with the report (the person ticks them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Include {
    pub system: bool,
    pub log: bool,
    pub failure: bool,
    pub crash: bool,
    /// The screenshot goes on the clipboard for pasting on GitHub (no API can attach it).
    pub screenshot: bool,
}

impl Default for Include {
    fn default() -> Self {
        Include { system: true, log: true, failure: true, crash: true, screenshot: true }
    }
}

/// The report as the person wrote it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Draft {
    pub kind: Kind,
    pub title: String,
    /// What happened (a problem) or the idea.
    pub text: String,
    /// What they did before it happened (problems only; optional).
    pub steps: String,
    /// What they expected instead (problems only; optional).
    pub expected: String,
}

/// Lines of log a report carries at most.
pub const LOG_LINES: usize = 40;

/// The marker the triage workflow reads (an HTML comment: GitHub doesn't show it).
pub const MARKER: &str = "<!-- shapecraft-report";

impl Draft {
    /// The title to send: what they typed, else the failure's, else the first words of the text.
    pub fn send_title(&self, ctx: &Context) -> String {
        let typed = self.title.trim();
        if !typed.is_empty() {
            return typed.chars().take(crate::github::MAX_TITLE).collect();
        }
        if let Some(f) = &ctx.failure {
            return f.title();
        }
        let first = self.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        if first.is_empty() {
            return match self.kind {
                Kind::Problem => "Problem report".into(),
                Kind::Idea => "Idea".into(),
            };
        }
        let mut t: String = first.chars().take(80).collect();
        if first.chars().count() > 80 {
            t.push('…');
        }
        t
    }

    /// Nothing written and nothing to go on: Send waits for words.
    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty() && self.text.trim().is_empty()
    }

    /// The issue's text in Markdown, home folder hidden, within GitHub's limit.
    pub fn body(&self, ctx: &Context, inc: Include, home: Option<&str>) -> String {
        let mut log_lines = if inc.log { ctx.log.len().min(LOG_LINES) } else { 0 };
        loop {
            let b = self.body_with(ctx, inc, home, log_lines);
            if b.chars().count() <= crate::github::MAX_BODY - 200 || log_lines == 0 {
                return b.chars().take(crate::github::MAX_BODY - 200).collect();
            }
            log_lines /= 2;
        }
    }

    fn body_with(&self, ctx: &Context, inc: Include, home: Option<&str>, log_lines: usize) -> String {
        let clean = |s: &str| scrub(s, home);
        let mut out = String::new();
        let section = |out: &mut String, head: &str, text: &str| {
            let t = text.trim();
            if !t.is_empty() {
                out.push_str(&format!("### {head}\n\n{}\n\n", clean(t)));
            }
        };
        match self.kind {
            Kind::Problem => {
                section(&mut out, "What happened", &self.text);
                section(&mut out, "Steps to reproduce", &self.steps);
                section(&mut out, "What I expected", &self.expected);
            }
            Kind::Idea => section(&mut out, "The idea", &self.text),
        }
        if out.is_empty() {
            out.push_str("_(No description.)_\n\n");
        }
        if inc.screenshot {
            out.push_str("### Screenshot\n\n_Added below by the reporter (ShapeCraft copied it for pasting)._\n\n");
        }
        if inc.failure
            && let Some(f) = &ctx.failure
        {
            out.push_str(&format!(
                "### What failed\n\n**{}**: {}\n\n<details><summary>What it was asked to do</summary>\n\n```json\n{}\n```\n</details>\n\n",
                f.command,
                clean(f.error.trim()),
                clean(f.params.trim())
            ));
        }
        if inc.crash
            && let Some(c) = &ctx.crash
        {
            out.push_str(&format!(
                "### Last crash\n\n<details><summary>Crash record</summary>\n\n```text\n{}\n```\n</details>\n\n",
                clean(c.trim()).chars().take(12_000).collect::<String>()
            ));
        }
        if inc.system {
            out.push_str("### ShapeCraft and system\n\n");
            for (k, v) in [
                ("ShapeCraft", &ctx.version),
                ("System", &ctx.system),
                ("Graphics", &ctx.graphics),
                ("Input", &ctx.input),
                ("Window", &ctx.window),
            ] {
                if !v.is_empty() {
                    out.push_str(&format!("- {k}: {}\n", clean(v)));
                }
            }
            out.push('\n');
        }
        if log_lines > 0 {
            let start = ctx.log.len().saturating_sub(log_lines);
            let text: Vec<String> = ctx.log[start..].iter().map(|l| clean(l)).collect();
            out.push_str(&format!(
                "<details><summary>Log (last {} lines)</summary>\n\n```text\n{}\n```\n</details>\n\n",
                text.len(),
                text.join("\n")
            ));
        }
        let kind = match self.kind {
            Kind::Problem => "problem",
            Kind::Idea => "idea",
        };
        let sig = ctx
            .failure
            .as_ref()
            .filter(|_| inc.failure)
            .map(|f| format!(" sig=\"{}\"", f.signature().replace('"', "'")))
            .unwrap_or_default();
        out.push_str(&format!(
            "<sub>Sent from ShapeCraft {}.</sub>\n{MARKER} v1 kind={kind} screenshot={}{sig} -->\n",
            if ctx.version.is_empty() { "?" } else { &ctx.version },
            inc.screenshot
        ));
        out
    }
}

/// `text` with the home folder written as `~` (a bare root is never replaced).
pub fn scrub(text: &str, home: Option<&str>) -> String {
    let Some(home) = home.map(|h| h.trim_end_matches(['/', '\\'])).filter(|h| h.len() > 3) else {
        return text.to_string();
    };
    let mut out = text.replace(home, "~");
    // The same folder written with the other slash (Windows logs carry both).
    let other = if home.contains('\\') { home.replace('\\', "/") } else { home.replace('/', "\\") };
    if other != home {
        out = out.replace(&other, "~");
    }
    out
}

/// The home folder as text, for [`scrub`].
pub fn home_text() -> Option<String> {
    dirs::home_dir().map(|p| p.display().to_string())
}

/// GitHub's sign-in page carries a new-issue link back encoded once more; past about this many characters it drops it.
const MAX_LINK: usize = 7_600;

/// A link that opens GitHub's new-issue form with the report filled in, and whether the body fit (when it doesn't,
/// the link carries the title alone and the body goes on the clipboard).
pub fn browser_link(title: &str, body: &str) -> (String, bool) {
    let with = format!("{}?title={}&body={}", crate::NEW_ISSUE_PAGE, crate::github::encode(title), crate::github::encode(body));
    let resigned = crate::github::encode(&with).len();
    if resigned <= MAX_LINK { (with, true) } else { (format!("{}?title={}", crate::NEW_ISSUE_PAGE, crate::github::encode(title)), false) }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn ctx() -> Context {
        Context {
            version: "0.10.5-alpha".into(),
            system: "macOS 26.1 (aarch64)".into(),
            graphics: "Apple M2 (Metal)".into(),
            input: "mouse".into(),
            window: "1728 × 1080".into(),
            log: (0..100).map(|i| format!("12:00:{i:02} ui: opened /Users/pat/Documents/Part {i}.shapr")).collect(),
            failure: Some(Failure {
                command: "fillet".into(),
                params: "{\"edges\":[12,13],\"radius\":4}".into(),
                error: "the radius 4 mm is too large for edge 'Edge 12'".into(),
            }),
            crash: None,
        }
    }

    #[test]
    fn a_failure_titles_and_signs_itself() {
        let f = ctx().failure.unwrap();
        assert_eq!(f.title(), "Fillet: the radius 4 mm is too large for edge 'Edge 12'");
        assert_eq!(f.signature(), "fillet · the radius # mm is too large for edge");
        let g = Failure { error: "the radius 2.5 mm is too large for edge \"Edge 3\"".into(), ..f.clone() };
        assert_eq!(f.signature(), g.signature());
    }

    #[test]
    fn the_body_has_their_words_first_and_no_home_folder() {
        let d = Draft { text: "The fillet refused.".into(), steps: "1. Box\n2. Fillet both edges".into(), ..Default::default() };
        let b = d.body(&ctx(), Include::default(), Some("/Users/pat"));
        assert!(b.starts_with("### What happened\n\nThe fillet refused."), "{b}");
        assert!(b.contains("### Steps to reproduce") && !b.contains("What I expected"), "{b}");
        assert!(b.contains("**fillet**") && b.contains("\"radius\":4"), "{b}");
        assert!(b.contains("- ShapeCraft: 0.10.5-alpha") && b.contains("Log (last 40 lines)"), "{b}");
        assert!(!b.contains("/Users/pat") && b.contains("~/Documents/Part 99.shapr"), "{b}");
        assert!(
            b.contains("<!-- shapecraft-report v1 kind=problem screenshot=true sig=\"fillet · the radius # mm is too large for edge\" -->"),
            "{b}"
        );
    }

    #[test]
    fn unticked_details_stay_home() {
        let d = Draft { kind: Kind::Idea, text: "Gears!".into(), ..Default::default() };
        let none = Include { system: false, log: false, failure: false, crash: false, screenshot: false };
        let b = d.body(&ctx(), none, None);
        assert!(b.starts_with("### The idea\n\nGears!"), "{b}");
        for gone in ["fillet", "macOS", "Log", "Screenshot", "sig="] {
            assert!(!b.contains(gone), "{gone} in {b}");
        }
    }

    #[test]
    fn a_title_always_comes_from_somewhere() {
        let mut c = ctx();
        let d = Draft::default();
        assert_eq!(d.send_title(&c), "Fillet: the radius 4 mm is too large for edge 'Edge 12'");
        c.failure = None;
        assert_eq!(d.send_title(&c), "Problem report");
        let long = Draft { text: format!("\n  {}", "word ".repeat(40)), ..Default::default() };
        assert!(long.send_title(&c).ends_with('…') && long.send_title(&c).chars().count() == 81);
        assert!(Draft::default().is_empty() && !long.is_empty());
    }

    #[test]
    fn a_huge_log_is_cut_to_fit_github() {
        let mut c = ctx();
        c.log = vec!["x".repeat(4000); 40];
        let b = Draft { text: "t".into(), ..Default::default() }.body(&c, Include::default(), None);
        assert!(b.chars().count() <= crate::github::MAX_BODY - 200);
        assert!(b.contains("t") && b.contains("shapecraft-report"), "kept the words and the marker");
    }

    #[test]
    fn home_is_hidden_with_either_slash() {
        assert_eq!(scrub("C:\\Users\\pat\\a and C:/Users/pat/b", Some("C:\\Users\\pat")), "~\\a and ~/b");
        assert_eq!(scrub("/x", Some("/")), "/x");
    }

    #[test]
    fn a_long_report_goes_by_clipboard() {
        let (short, fits) = browser_link("T", "small body");
        assert!(fits && short.contains("body=small%20body"));
        let (long, fits) = browser_link("T", &"y".repeat(9000));
        assert!(!fits && !long.contains("body="));
    }
}
