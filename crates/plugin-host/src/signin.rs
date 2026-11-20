//! Signing in through the browser for a plugin (OAuth with a loopback redirect, RFC 8252): listen on this computer,
//! open the provider's page, and wait for the browser to come back with `?code=…`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>ShapeCraft</title>\
<body style=\"font:15px -apple-system,system-ui,sans-serif;background:#1c1c1e;color:#eee;display:grid;place-items:center;height:90vh\">\
<div style=\"text-align:center\"><h2 style=\"font-weight:600\">You're signed in</h2>\
<p style=\"color:#aaa\">You can close this tab and go back to ShapeCraft.</p></div>";

pub struct Waiting {
    listener: TcpListener,
    pub redirect: String,
}

/// Starts listening on a free port of 127.0.0.1. The redirect uses "localhost" (what providers register for
/// native apps); browsers send localhost to 127.0.0.1 or ::1, so both are tried when ::1 is free.
pub fn listen() -> Result<Waiting, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("couldn't listen for the browser ({e})"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    Ok(Waiting { listener, redirect: format!("http://localhost:{port}/callback") })
}

/// Puts the redirect address into the provider's URL where it says `{redirect}`.
pub fn fill(url: &str, redirect: &str) -> String {
    url.replace("{redirect}", &percent(redirect))
}

fn percent(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

impl Waiting {
    /// Waits for the browser's request to /callback; returns its query string. Gives up after `limit` or when
    /// `stop` is set.
    pub fn wait(self, limit: Duration, stop: &AtomicBool) -> Result<String, String> {
        let v6 = self.redirect.split(':').nth(2).and_then(|p| p.split('/').next()?.parse::<u16>().ok()).and_then(|port| {
            let l = TcpListener::bind(("::1", port)).ok()?;
            l.set_nonblocking(true).ok()?;
            Some(l)
        });
        let start = Instant::now();
        while start.elapsed() < limit {
            if stop.load(Ordering::Relaxed) {
                return Err("sign-in cancelled".into());
            }
            for l in std::iter::once(&self.listener).chain(v6.as_ref()) {
                if let Ok((stream, _)) = l.accept()
                    && let Some(q) = answer(stream)
                {
                    return Ok(q);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err("sign-in timed out (nothing came back from the browser)".into())
    }
}

/// Reads one request; for `GET /callback?…` answers with the done page and returns the query.
fn answer(mut stream: TcpStream) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut line = String::new();
    BufReader::new(&stream).take_line(&mut line)?;
    let target = line.split_whitespace().nth(1)?.to_string();
    let query = target.strip_prefix("/callback").map(|rest| rest.trim_start_matches('?').to_string());
    let (status, body) = if query.is_some() { ("200 OK", DONE_PAGE) } else { ("404 Not Found", "") };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    query
}

trait TakeLine {
    fn take_line(self, out: &mut String) -> Option<()>;
}

impl<R: std::io::Read> TakeLine for BufReader<R> {
    fn take_line(mut self, out: &mut String) -> Option<()> {
        // A request line is short; refuse anything silly.
        let mut limited = (&mut self).take(8192);
        let mut r = BufReader::new(&mut limited);
        r.read_line(out).ok().filter(|n| *n > 0).map(|_| ())
    }
}

use std::io::Read as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_the_redirect() {
        assert_eq!(
            fill("https://openrouter.ai/auth?callback_url={redirect}&x=1", "http://localhost:5/callback"),
            "https://openrouter.ai/auth?callback_url=http%3A%2F%2Flocalhost%3A5%2Fcallback&x=1"
        );
    }

    #[test]
    fn catches_the_callback() -> Result<(), String> {
        let w = listen()?;
        let port: u16 = w.redirect.split(':').nth(2).and_then(|p| p.split('/').next()?.parse().ok()).ok_or("port")?;
        let browser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
            s.write_all(b"GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n").map_err(|e| e.to_string())?;
            let mut page = String::new();
            let _ = s.read_to_string(&mut page);
            Ok::<String, String>(page)
        });
        let q = w.wait(Duration::from_secs(5), &AtomicBool::new(false))?;
        assert_eq!(q, "code=abc&state=xyz");
        let page = browser.join().map_err(|_| "browser thread")??;
        assert!(page.contains("signed in"));
        Ok(())
    }
}
