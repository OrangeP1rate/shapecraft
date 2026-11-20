//! What a plugin may reach over the network, and the requests themselves. The host makes every request (plugins
//! can't open sockets), after checking the address against the plugin's permissions.

use std::io::Read;
use std::time::Duration;

use crate::manifest::Permissions;

/// The most a single answer may hold (a few generated pictures fit easily).
pub const MAX_BODY: u64 = 96 << 20;
/// What a request may send.
pub const MAX_SEND: usize = 64 << 20;

/// The parts of an address the checks need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub https: bool,
    /// Lowercase, without brackets for IPv6.
    pub host: String,
}

/// Reads `scheme://host[:port][/…]`. No user names in addresses (`user@host` reads as a different host than it is).
pub fn parse(url: &str) -> Result<Address, String> {
    let (scheme, rest) = url.split_once("://").ok_or_else(|| format!("\"{url}\" isn't a web address"))?;
    let https = match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => false,
        other => return Err(format!("{other}:// addresses aren't allowed")),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("addresses with a user name aren't allowed".into());
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("").to_string()
    } else {
        authority.split(':').next().unwrap_or("").to_string()
    };
    if host.is_empty() {
        return Err(format!("\"{url}\" has no host"));
    }
    Ok(Address { https, host: host.to_ascii_lowercase() })
}

/// The computer itself or a private network: localhost, 127/8, ::1, 10/8, 172.16/12, 192.168/16, fd00::/8,
/// `*.local`, `*.lan`, `*.home.arpa`, or a bare name without dots ("gpu-box").
pub fn is_local(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") || host == "::1" {
        return true;
    }
    if host.ends_with(".local") || host.ends_with(".lan") || host.ends_with(".home.arpa") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return ip.is_loopback() || ip.is_private();
    }
    if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        return ip.is_loopback() || (ip.segments()[0] & 0xff00) == 0xfd00;
    }
    !host.contains('.') && !host.contains(':')
}

fn host_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    match pattern.strip_prefix("*.") {
        Some(base) => host.len() > base.len() + 1 && host.ends_with(&format!(".{base}")),
        None => host == pattern,
    }
}

/// Whether `perms` let a plugin reach `url`. Internet hosts need HTTPS; the local network may use plain HTTP.
pub fn allowed(perms: &Permissions, url: &str) -> Result<Address, String> {
    let a = parse(url)?;
    if is_local(&a.host) {
        return if perms.local_network {
            Ok(a)
        } else {
            Err(format!("this plugin may not reach {} (it didn't ask for the local network)", a.host))
        };
    }
    if !perms.network.iter().any(|p| host_matches(p, &a.host)) {
        return Err(format!("this plugin may not reach {} (not in its permissions)", a.host));
    }
    if !a.https {
        return Err(format!("{} must be reached over https", a.host));
    }
    Ok(a)
}

pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

fn agent(timeout: Duration) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(concat!("ShapeCraft/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into()
}

fn describe(e: &ureq::Error, host: &str) -> String {
    match e {
        ureq::Error::Timeout(_) => format!("{host} took too long to answer"),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => format!("{host} took too long to answer"),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            format!("nothing is answering at {host} (is it running?)")
        }
        ureq::Error::HostNotFound => format!("couldn't find {host} (no internet, or the address is wrong)"),
        other => format!("{host}: {other}"),
    }
}

/// Sends one request. Redirects aren't followed (a redirect could leave the allowed hosts); the plugin sees them.
pub fn fetch(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
    timeout_secs: u32,
    host: &str,
) -> Result<Answer, String> {
    if body.len() > MAX_SEND {
        return Err(format!("the request is too big ({} MB)", body.len() >> 20));
    }
    let secs = if timeout_secs == 0 { 60 } else { timeout_secs.min(600) };
    let method = method.to_ascii_uppercase();
    let mut b = ureq::http::Request::builder().method(method.as_str()).uri(url);
    for (k, v) in headers {
        b = b.header(k.as_str(), v.as_str());
    }
    let a = agent(Duration::from_secs(u64::from(secs)));
    let resp = if body.is_empty() && matches!(method.as_str(), "GET" | "HEAD" | "DELETE" | "OPTIONS") {
        let req = b.body(()).map_err(|e| e.to_string())?;
        a.run(req)
    } else {
        let req = b.body(body).map_err(|e| e.to_string())?;
        a.run(req)
    };
    let mut resp = resp.map_err(|e| describe(&e, host))?;
    let status = resp.status().as_u16();
    let headers = resp.headers().iter().filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string()))).collect();
    let mut body = Vec::new();
    resp.body_mut()
        .with_config()
        .limit(MAX_BODY)
        .reader()
        .read_to_end(&mut body)
        .map_err(|e| format!("{host}: the answer couldn't be read ({e})"))?;
    Ok(Answer { status, headers, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(net: &[&str], local: bool) -> Permissions {
        Permissions { network: net.iter().map(|s| s.to_string()).collect(), local_network: local, ..Default::default() }
    }

    #[test]
    fn parses_addresses() -> Result<(), String> {
        assert_eq!(parse("https://OpenRouter.ai/api/v1?x=1")?, Address { https: true, host: "openrouter.ai".into() });
        assert_eq!(parse("http://127.0.0.1:8188/prompt")?.host, "127.0.0.1");
        assert_eq!(parse("http://[::1]:8188/")?.host, "::1");
        assert!(parse("ftp://x.com").is_err());
        assert!(parse("https://evil.com@openrouter.ai/").is_err());
        assert!(parse("openrouter.ai").is_err());
        Ok(())
    }

    #[test]
    fn local_hosts() {
        for h in ["localhost", "127.0.0.1", "::1", "10.1.2.3", "172.20.0.5", "192.168.1.9", "gpu-box", "pc.local", "fd12::1"] {
            assert!(is_local(h), "{h}");
        }
        for h in ["openrouter.ai", "8.8.8.8", "172.32.0.1", "2001:db8::1"] {
            assert!(!is_local(h), "{h}");
        }
    }

    #[test]
    fn checks_permissions() {
        let p = perms(&["openrouter.ai", "*.googleapis.com"], false);
        assert!(allowed(&p, "https://openrouter.ai/api").is_ok());
        assert!(allowed(&p, "https://generativelanguage.googleapis.com/v1beta").is_ok());
        assert!(allowed(&p, "https://googleapis.com/").is_err());
        assert!(allowed(&p, "http://openrouter.ai/api").is_err());
        assert!(allowed(&p, "https://api.openai.com/").is_err());
        assert!(allowed(&p, "http://127.0.0.1:8188/").is_err());
        let local = perms(&[], true);
        assert!(allowed(&local, "http://127.0.0.1:8188/").is_ok());
        assert!(allowed(&local, "http://192.168.1.20:7860/").is_ok());
        assert!(allowed(&local, "https://openrouter.ai/").is_err());
    }
}
