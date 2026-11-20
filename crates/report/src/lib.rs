//! Problem reports and ideas, filed from inside ShapeCraft as GitHub issues under the person's own name.
//!
//! Signing in uses GitHub's device sign-in for the "ShapeCraft Reports" GitHub App: ShapeCraft shows a short code, the
//! person approves it on github.com (any browser, any device), and from then on ShapeCraft may open issues on this
//! project's repository for them. The app's only permission is Issues, and it is installed on this one repository,
//! so the sign-in reaches nothing else of theirs. No secret ships in ShapeCraft and no server of ours is involved.
//!
//! Nothing is sent until the person presses Send, and they see every line of it first (see [`draft`]).

#![forbid(unsafe_code)]

pub mod account;
pub mod draft;
pub mod github;

/// Issues go to this repository only.
pub const REPO: &str = "jackharvest/shapecraft";
/// The "ShapeCraft Reports" GitHub App's public client id (device sign-in needs nothing else).
pub const CLIENT_ID: &str = "Iv23liQNCyewz723726g";
/// Where the person types the code.
pub const DEVICE_PAGE: &str = "https://github.com/login/device";
/// Where someone without an account makes one (free).
pub const SIGN_UP_PAGE: &str = "https://github.com/signup";
/// Where the person can see or revoke what ShapeCraft may do.
pub const APP_SETTINGS_PAGE: &str = "https://github.com/settings/apps/authorizations";
/// The new-issue page (the fallback when filing from here can't work).
pub const NEW_ISSUE_PAGE: &str = "https://github.com/jackharvest/shapecraft/issues/new";
