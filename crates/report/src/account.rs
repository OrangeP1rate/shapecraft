//! The GitHub sign-in ShapeCraft keeps for reports: the token and the name it belongs to, in a file only this user can
//! read. The token can only open and read issues on this project's repository (that's all the GitHub App may do), and
//! the person can revoke it on GitHub or with Sign Out at any time.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// The GitHub login (without the @).
    pub login: String,
    /// The user access token from the device sign-in.
    pub token: String,
}

impl Account {
    /// "@login".
    pub fn handle(&self) -> String {
        format!("@{}", self.login)
    }
}

/// `report-account.json`, next to the settings file.
pub fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("ShapeCraft").join("report-account.json"))
}

/// The saved sign-in, if any.
pub fn load() -> Option<Account> {
    load_from(&path()?)
}

pub fn load_from(path: &std::path::Path) -> Option<Account> {
    let text = std::fs::read_to_string(path).ok()?;
    let account: Account = serde_json::from_str(&text).ok()?;
    (!account.login.is_empty() && !account.token.is_empty()).then_some(account)
}

/// Keeps the sign-in for next time.
pub fn save(account: &Account) -> Result<(), String> {
    save_to(&path().ok_or("no settings folder on this system")?, account)
}

pub fn save_to(path: &std::path::Path, account: &Account) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(account).map_err(|e| e.to_string())?;
    let part = path.with_extension("json.part");
    write_private(&part, text.as_bytes())?;
    std::fs::rename(&part, path).map_err(|e| e.to_string())
}

/// Forgets the sign-in on this computer.
pub fn forget() {
    if let Some(p) = path() {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(path).map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    // A Windows user's AppData is already private to them.
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_saved_account_comes_back_and_only_its_owner_can_read_it() {
        let dir = std::env::temp_dir().join(format!("shapecraft-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("report-account.json");
        let a = Account { login: "octocat".into(), token: "ghu_x".into() };
        save_to(&file, &a).unwrap();
        assert_eq!(load_from(&file), Some(a.clone()));
        assert_eq!(a.handle(), "@octocat");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&file, "{\"login\":\"\",\"token\":\"\"}").unwrap();
        assert_eq!(load_from(&file), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
