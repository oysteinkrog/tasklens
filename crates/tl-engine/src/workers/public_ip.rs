//! The public IP fetch (PERF-060, APP-056, R-131): the only outbound network request in
//! tasklens.
//!
//! The Inventory worker runs [`fetch`] on its blocking pool, and only on a
//! `DeviceCommand::FetchPublicIp` command. The answer is untrusted: at most [`MAX_BODY`] bytes,
//! accepted only when it parses as an IP address, and no redirect is followed. The address is
//! shown on screen only. It is never logged or saved.

use std::io::{ErrorKind, Read};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tl_model::snapshot::PublicIpState;

pub const URL_V4: &str = "https://api.ipify.org";
pub const URL_V6: &str = "https://api6.ipify.org";
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// Read limit for the answer, in bytes. An address is at most 39 characters.
pub const MAX_BODY: u64 = 64;
/// Read limit for `/proc/net/ipv6_route`, in bytes.
const ROUTE_MAX: u64 = 64 * 1024;

/// One HTTPS GET. `Ok` holds the trimmed body of a 200 answer, `Err` the text shown in the
/// Public IP row.
pub trait Fetcher: Send + Sync {
    fn get(&self, url: &str) -> Result<String, Arc<str>>;
}

/// The real fetcher: `ureq` with rustls, a 5 s global timeout and no redirects.
pub struct UreqFetcher {
    agent: ureq::Agent,
}

impl UreqFetcher {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .max_redirects(0)
            .http_status_as_error(false)
            .build();
        UreqFetcher {
            agent: config.into(),
        }
    }
}

impl Default for UreqFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher for UreqFetcher {
    fn get(&self, url: &str) -> Result<String, Arc<str>> {
        let host = host_of(url);
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| error_text(host, &e))?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(format!("{host} answered {status}").into());
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()
            .map_err(|e| error_text(host, &e))?;
        Ok(body.trim().to_owned())
    }
}

/// The text shown for a failed request to `host`. It never holds the response body.
pub fn error_text(host: &str, e: &ureq::Error) -> Arc<str> {
    let text = match e {
        ureq::Error::Timeout(_) => format!("No answer from {host} within 5 s"),
        ureq::Error::Io(io) if io.kind() == ErrorKind::TimedOut => {
            format!("No answer from {host} within 5 s")
        }
        ureq::Error::StatusCode(code) => format!("{host} answered {code}"),
        ureq::Error::BodyExceedsLimit(_) => unexpected(host),
        _ => format!("Could not reach {host}"),
    };
    text.into()
}

fn unexpected(host: &str) -> String {
    format!("Unexpected answer from {host}")
}

/// `api.ipify.org` from `https://api.ipify.org`.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
}

/// Blocking. `has_v6_route` from `/proc/net/ipv6_route` (any non-loopback default or global
/// route line). Asks `URL_V6` only when it is true.
pub fn fetch(f: &dyn Fetcher, has_v6_route: bool, now_unix_s: i64) -> PublicIpState {
    let v4 = get_addr::<Ipv4Addr>(f, URL_V4);
    let v6 = if has_v6_route {
        Some(get_addr::<Ipv6Addr>(f, URL_V6))
    } else {
        None
    };
    match (v4, v6) {
        (Err(first), None | Some(Err(_))) => PublicIpState::Failed(first),
        (v4, v6) => PublicIpState::Fetched {
            v4: v4.ok(),
            v6: v6.and_then(Result::ok),
            fetched_at_s: now_unix_s,
        },
    }
}

/// One GET whose body must parse as `A`. Returns the address in its canonical form.
fn get_addr<A>(f: &dyn Fetcher, url: &str) -> Result<Arc<str>, Arc<str>>
where
    A: std::str::FromStr + std::fmt::Display,
{
    let body = f.get(url)?;
    match body.trim().parse::<A>() {
        Ok(addr) => Ok(addr.to_string().into()),
        Err(_) => Err(unexpected(host_of(url)).into()),
    }
}

/// True when `<proc_root>/net/ipv6_route` (read up to 64 KiB) has a line whose destination
/// prefix length is `00` (a default route) on an interface other than `lo`.
pub fn has_global_v6_route(proc_root: &Path) -> bool {
    let Ok(file) = std::fs::File::open(proc_root.join("net/ipv6_route")) else {
        return false;
    };
    let mut text = String::new();
    if file.take(ROUTE_MAX).read_to_string(&mut text).is_err() {
        return false;
    }
    text.lines().any(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        fields.len() >= 10 && fields[1] == "00" && fields.last().is_some_and(|dev| *dev != "lo")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::thread;

    /// Answers each URL from a fixed list and records the URLs asked.
    struct Fake {
        answers: Vec<(&'static str, Result<String, Arc<str>>)>,
        asked: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(answers: Vec<(&'static str, Result<String, Arc<str>>)>) -> Fake {
            Fake {
                answers,
                asked: Mutex::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<String> {
            self.asked.lock().expect("asked lock").clone()
        }
    }

    impl Fetcher for Fake {
        fn get(&self, url: &str) -> Result<String, Arc<str>> {
            self.asked.lock().expect("asked lock").push(url.to_owned());
            self.answers
                .iter()
                .find(|(u, _)| *u == url)
                .map(|(_, answer)| answer.clone())
                .unwrap_or_else(|| Err("no answer".into()))
        }
    }

    const NOW: i64 = 1_759_500_000;

    #[test]
    fn v4_only() {
        let fake = Fake::new(vec![(URL_V4, Ok("203.0.113.4\n".into()))]);
        assert_eq!(
            fetch(&fake, false, NOW),
            PublicIpState::Fetched {
                v4: Some("203.0.113.4".into()),
                v6: None,
                fetched_at_s: NOW,
            }
        );
        assert_eq!(fake.asked(), [URL_V4]);
    }

    #[test]
    fn v4_and_v6() {
        let fake = Fake::new(vec![
            (URL_V4, Ok("203.0.113.4".into())),
            (URL_V6, Ok("2001:db8::1".into())),
        ]);
        let PublicIpState::Fetched { v4, v6, .. } = fetch(&fake, true, NOW) else {
            panic!("not fetched");
        };
        assert_eq!(v4.as_deref(), Some("203.0.113.4"));
        assert_eq!(v6.as_deref(), Some("2001:db8::1"));
        assert_eq!(fake.asked(), [URL_V4, URL_V6]);

        // One half is enough.
        let fake = Fake::new(vec![(URL_V6, Ok("2001:db8::1".into()))]);
        let PublicIpState::Fetched { v4, v6, .. } = fetch(&fake, true, NOW) else {
            panic!("not fetched");
        };
        assert_eq!(v4, None);
        assert_eq!(v6.as_deref(), Some("2001:db8::1"));
    }

    #[test]
    fn garbage_body_fails() {
        let fake = Fake::new(vec![(URL_V4, Ok("<html>hi</html>".into()))]);
        assert_eq!(
            fetch(&fake, false, NOW),
            PublicIpState::Failed("Unexpected answer from api.ipify.org".into())
        );
        // A v4 address from the v6 URL is not accepted either.
        let fake = Fake::new(vec![
            (URL_V4, Ok("x".into())),
            (URL_V6, Ok("203.0.113.4".into())),
        ]);
        assert_eq!(
            fetch(&fake, true, NOW),
            PublicIpState::Failed("Unexpected answer from api.ipify.org".into())
        );
    }

    #[test]
    fn timeout_text() {
        let text = error_text(
            "api.ipify.org",
            &ureq::Error::Timeout(ureq::Timeout::Global),
        );
        assert_eq!(&*text, "No answer from api.ipify.org within 5 s");
        let fake = Fake::new(vec![(URL_V4, Err(text))]);
        assert_eq!(
            fetch(&fake, false, NOW),
            PublicIpState::Failed("No answer from api.ipify.org within 5 s".into())
        );
        assert_eq!(
            &*error_text("api.ipify.org", &ureq::Error::ConnectionFailed),
            "Could not reach api.ipify.org"
        );
        assert_eq!(
            &*error_text("api.ipify.org", &ureq::Error::Tls("bad certificate")),
            "Could not reach api.ipify.org"
        );
        assert_eq!(
            &*error_text("api.ipify.org", &ureq::Error::StatusCode(503)),
            "api.ipify.org answered 503"
        );
    }

    #[test]
    fn v6_route_detection() {
        let dir = tempfile::tempdir().expect("temp dir");
        let net = dir.path().join("net");
        std::fs::create_dir(&net).expect("net dir");
        let route = net.join("ipv6_route");
        assert!(!has_global_v6_route(dir.path()), "missing file");

        let lo = "00000000000000000000000000000001 80 00000000000000000000000000000000 00 \
                  00000000000000000000000000000000 00000000 00000002 00000000 00000001       lo\n\
                  00000000000000000000000000000000 00 00000000000000000000000000000000 00 \
                  00000000000000000000000000000000 ffffffff 00000001 00000000 00200200       lo\n";
        std::fs::write(&route, lo).expect("write lo");
        assert!(!has_global_v6_route(dir.path()));

        let link = "fe800000000000000000000000000000 40 00000000000000000000000000000000 00 \
                    00000000000000000000000000000000 00000100 00000001 00000000 00000001     eno1\n";
        std::fs::write(&route, format!("{lo}{link}")).expect("write link");
        assert!(!has_global_v6_route(dir.path()), "link-local only");

        let default = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 \
                       fe80000000000000022b67fffe123456 00000400 00000002 00000000 00450003     eno1\n";
        std::fs::write(&route, format!("{lo}{link}{default}")).expect("write default");
        assert!(has_global_v6_route(dir.path()));
    }

    /// One HTTP answer from a listener on 127.0.0.1. Returns its URL.
    fn serve_once(answer: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1");
        let port = listener.local_addr().expect("local addr").port();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(answer.as_bytes());
            }
        });
        format!("http://127.0.0.1:{port}/")
    }

    #[test]
    fn ureq_fetcher_local() {
        let f = UreqFetcher::new();
        let ok = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\n203.0.113.4\n",
        );
        assert_eq!(f.get(&ok).as_deref(), Ok("203.0.113.4"));

        // A redirect is not followed.
        let moved = serve_once(
            "HTTP/1.1 301 Moved\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let err = f.get(&moved).expect_err("redirect fails");
        assert!(err.ends_with(" answered 301"), "{err}");

        let missing =
            serve_once("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let err = f.get(&missing).expect_err("404 fails");
        assert!(err.ends_with(" answered 404"), "{err}");

        // A body over MAX_BODY bytes is refused.
        let big = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n\
             0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        );
        let err = f.get(&big).expect_err("big body fails");
        assert!(err.starts_with("Unexpected answer from "), "{err}");
    }
}
