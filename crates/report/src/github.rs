//! The few GitHub calls reports need: device sign-in, who signed in, opening an issue, and listing the person's own
//! reports. Every call blocks; the UI runs them on their own thread.

use serde_json::{Value, json};
use std::time::Duration;

const API: &str = "https://api.github.com";
const MAX_ANSWER: u64 = 2 << 20;
/// GitHub refuses an issue body longer than this many characters.
pub const MAX_BODY: usize = 65_536;
pub const MAX_TITLE: usize = 256;

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(format!("ShapeCraft/{}", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(45)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn describe(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => "the connection timed out".into(),
        ureq::Error::Timeout(_) => "the connection timed out".into(),
        ureq::Error::HostNotFound => "no internet connection (couldn't reach github.com)".into(),
        other => other.to_string(),
    }
}

/// An answer: its status and its JSON (Null when it wasn't JSON).
fn answer(result: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(u16, Value), String> {
    let mut resp = result.map_err(|e| describe(&e))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().with_config().limit(MAX_ANSWER).read_to_string().map_err(|e| describe(&e))?;
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&")
}

/// Percent-encodes everything but the unreserved characters (RFC 3986), for forms and query strings.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn post_form(url: &str, pairs: &[(&str, &str)]) -> Result<(u16, Value), String> {
    answer(
        agent()
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(form(pairs)),
    )
}

fn api_get(token: &str, path: &str) -> Result<(u16, Value), String> {
    answer(
        agent()
            .get(&format!("{API}{path}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Authorization", &format!("Bearer {token}"))
            .call(),
    )
}

fn api_post(token: &str, path: &str, body: &Value) -> Result<(u16, Value), String> {
    answer(
        agent()
            .post(&format!("{API}{path}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Authorization", &format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .send(body.to_string()),
    )
}

/// The code to type on GitHub, and what to poll with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    /// "WDJB-MJHT".
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until the code stops working.
    pub expires_in: u64,
    /// Seconds to wait between polls.
    pub interval: u64,
}

/// Asks GitHub for a sign-in code.
pub fn start_sign_in() -> Result<DeviceCode, String> {
    let (status, v) = post_form("https://github.com/login/device/code", &[("client_id", crate::CLIENT_ID)])?;
    parse_device_code(status, &v)
}

pub fn parse_device_code(status: u16, v: &Value) -> Result<DeviceCode, String> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    match (status, s("device_code"), s("user_code")) {
        (200, Some(device_code), Some(user_code)) => Ok(DeviceCode {
            device_code,
            user_code,
            verification_uri: s("verification_uri").unwrap_or_else(|| crate::DEVICE_PAGE.into()),
            expires_in: v.get("expires_in").and_then(Value::as_u64).unwrap_or(900).clamp(60, 3600),
            interval: v.get("interval").and_then(Value::as_u64).unwrap_or(5).clamp(1, 60),
        }),
        _ => Err(github_error(status, v, "GitHub didn't give a sign-in code")),
    }
}

/// Where a sign-in stands after one poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poll {
    /// Not approved yet: ask again after this many seconds.
    Waiting(u64),
    Approved(String),
    /// The code ran out; start again.
    Expired,
    /// The person pressed Cancel on GitHub.
    Denied,
}

/// Asks once whether the code was approved.
pub fn poll_sign_in(code: &DeviceCode) -> Result<Poll, String> {
    let (status, v) = post_form(
        "https://github.com/login/oauth/access_token",
        &[
            ("client_id", crate::CLIENT_ID),
            ("device_code", &code.device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ],
    )?;
    parse_poll(status, &v, code.interval)
}

pub fn parse_poll(status: u16, v: &Value, interval: u64) -> Result<Poll, String> {
    if let Some(token) = v.get("access_token").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        return Ok(Poll::Approved(token.to_string()));
    }
    match v.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => Ok(Poll::Waiting(interval)),
        Some("slow_down") => Ok(Poll::Waiting(v.get("interval").and_then(Value::as_u64).unwrap_or(interval + 5).clamp(1, 60))),
        Some("expired_token") | Some("token_expired") => Ok(Poll::Expired),
        Some("access_denied") => Ok(Poll::Denied),
        _ => Err(github_error(status, v, "GitHub didn't finish the sign-in")),
    }
}

/// The login a token belongs to.
pub fn who(token: &str) -> Result<String, String> {
    let (status, v) = api_get(token, "/user")?;
    match (status, v.get("login").and_then(Value::as_str)) {
        (200, Some(login)) => Ok(login.to_string()),
        (401, _) => Err(SIGNED_OUT.into()),
        _ => Err(github_error(status, &v, "GitHub didn't say who signed in")),
    }
}

/// What every call answers when GitHub no longer accepts the sign-in (revoked, or the app was removed).
pub const SIGNED_OUT: &str = "GitHub no longer accepts this sign-in. Sign in again to send.";

/// An issue that was opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filed {
    pub number: u64,
    pub url: String,
}

/// Opens an issue on this project's repository as the signed-in person.
pub fn file_issue(token: &str, title: &str, body: &str) -> Result<Filed, String> {
    let title: String = title.chars().take(MAX_TITLE).collect();
    let body: String = body.chars().take(MAX_BODY).collect();
    let (status, v) = api_post(token, &format!("/repos/{}/issues", crate::REPO), &json!({ "title": title, "body": body }))?;
    parse_filed(status, &v)
}

pub fn parse_filed(status: u16, v: &Value) -> Result<Filed, String> {
    match (status, v.get("number").and_then(Value::as_u64), v.get("html_url").and_then(Value::as_str)) {
        (201, Some(number), Some(url)) => Ok(Filed { number, url: url.to_string() }),
        (401, ..) => Err(SIGNED_OUT.into()),
        (403 | 404, ..) => Err(format!(
            "GitHub didn't let ShapeCraft open the issue ({}). You can still send it from the browser.",
            v.get("message").and_then(Value::as_str).unwrap_or("no access")
        )),
        _ => Err(github_error(status, v, "GitHub didn't open the issue")),
    }
}

/// One of the person's own reports, as the Your Reports list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mine {
    pub number: u64,
    pub title: String,
    pub open: bool,
    pub comments: u64,
    pub url: String,
    /// "2026-11-20T10:00:00Z".
    pub updated: String,
}

/// The person's latest issues on the repository (pull requests left out), newest activity first.
pub fn my_reports(token: &str, login: &str) -> Result<Vec<Mine>, String> {
    let (status, v) =
        api_get(token, &format!("/repos/{}/issues?creator={}&state=all&sort=updated&per_page=30", crate::REPO, encode(login)))?;
    if status == 401 {
        return Err(SIGNED_OUT.into());
    }
    if status != 200 {
        return Err(github_error(status, &v, "GitHub didn't list your reports"));
    }
    Ok(parse_mine(&v))
}

pub fn parse_mine(v: &Value) -> Vec<Mine> {
    let Some(list) = v.as_array() else { return Vec::new() };
    list.iter()
        .filter(|i| i.get("pull_request").is_none())
        .filter_map(|i| {
            Some(Mine {
                number: i.get("number")?.as_u64()?,
                title: i.get("title")?.as_str()?.to_string(),
                open: i.get("state").and_then(Value::as_str) != Some("closed"),
                comments: i.get("comments").and_then(Value::as_u64).unwrap_or(0),
                url: i.get("html_url")?.as_str()?.to_string(),
                updated: i.get("updated_at").and_then(Value::as_str).unwrap_or("").to_string(),
            })
        })
        .collect()
}

fn github_error(status: u16, v: &Value, what: &str) -> String {
    let said = v.get("error_description").or_else(|| v.get("message")).and_then(Value::as_str);
    match said {
        Some(m) => format!("{what}: {m} ({status})"),
        None => format!("{what} ({status})"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_sign_in_code_is_read_and_its_numbers_kept_sane() {
        let v = json!({"device_code":"d","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":899,"interval":5});
        let c = parse_device_code(200, &v).unwrap();
        assert_eq!((c.user_code.as_str(), c.interval, c.expires_in), ("WDJB-MJHT", 5, 899));
        let wild = json!({"device_code":"d","user_code":"X","interval":0,"expires_in":999_999});
        let c = parse_device_code(200, &wild).unwrap();
        assert_eq!((c.interval, c.expires_in, c.verification_uri.as_str()), (1, 3600, crate::DEVICE_PAGE));
        let e =
            parse_device_code(400, &json!({"error":"device_flow_disabled","error_description":"Device Flow must be explicitly enabled"}));
        assert!(e.unwrap_err().contains("explicitly enabled"));
    }

    #[test]
    fn every_poll_answer_is_understood() {
        assert_eq!(parse_poll(200, &json!({"error":"authorization_pending"}), 5).unwrap(), Poll::Waiting(5));
        assert_eq!(parse_poll(200, &json!({"error":"slow_down","interval":10}), 5).unwrap(), Poll::Waiting(10));
        assert_eq!(parse_poll(200, &json!({"error":"expired_token"}), 5).unwrap(), Poll::Expired);
        assert_eq!(parse_poll(200, &json!({"error":"access_denied"}), 5).unwrap(), Poll::Denied);
        assert_eq!(parse_poll(200, &json!({"access_token":"ghu_1","token_type":"bearer"}), 5).unwrap(), Poll::Approved("ghu_1".into()));
        assert!(parse_poll(200, &json!({"error":"incorrect_client_credentials"}), 5).is_err());
    }

    #[test]
    fn filing_answers_say_what_to_do() {
        let ok = parse_filed(201, &json!({"number":91,"html_url":"https://github.com/jackharvest/shapecraft/issues/91"})).unwrap();
        assert_eq!(ok.number, 91);
        assert_eq!(parse_filed(401, &json!({"message":"Bad credentials"})).unwrap_err(), SIGNED_OUT);
        assert!(parse_filed(403, &json!({"message":"Resource not accessible by integration"})).unwrap_err().contains("browser"));
        assert!(parse_filed(422, &json!({"message":"Validation Failed"})).unwrap_err().contains("Validation Failed"));
    }

    #[test]
    fn your_reports_leave_out_pull_requests() {
        let v = json!([
            {"number":5,"title":"Fillet fails","state":"open","comments":2,"html_url":"u5","updated_at":"t"},
            {"number":6,"title":"A PR","state":"open","comments":0,"html_url":"u6","pull_request":{}},
            {"number":7,"title":"Idea","state":"closed","comments":0,"html_url":"u7"}
        ]);
        let m = parse_mine(&v);
        assert_eq!(m.iter().map(|m| (m.number, m.open, m.comments)).collect::<Vec<_>>(), vec![(5, true, 2), (7, false, 0)]);
    }

    #[test]
    fn forms_encode_like_github_expects() {
        assert_eq!(encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(form(&[("client_id", "Iv1"), ("x", "a b")]), "client_id=Iv1&x=a%20b");
    }
}
