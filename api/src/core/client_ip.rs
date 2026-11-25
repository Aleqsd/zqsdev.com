//! Visitor IP: `CF-Connecting-IP` only.
//!
//! The edge sets `CF-Connecting-IP` on requests from the internet, and the site Worker passes the
//! original request through the service binding. `X-Forwarded-For` and `X-Real-IP` are never read,
//! so a client cannot pick its rate-limit bucket.

use std::net::{IpAddr, Ipv6Addr};

/// Stored IP (and limiter key) when `CF-Connecting-IP` is missing or invalid.
pub const UNKNOWN: &str = "unknown";

/// The visitor IP as stored in logs, and the key of its per-IP admission windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientIp {
    /// Canonical address (`IpAddr::to_canonical`), or `unknown`.
    pub ip: String,
    /// IPv4 address, IPv6 `/64` prefix (`2001:db8:1:2::/64`), or `unknown`.
    pub key: String,
    /// False when the header was missing or did not parse.
    pub known: bool,
}

/// Resolves the visitor from the `CF-Connecting-IP` header value.
pub fn resolve(cf_connecting_ip: Option<&str>) -> ClientIp {
    match cf_connecting_ip.and_then(|v| v.parse::<IpAddr>().ok()) {
        Some(addr) => {
            let addr = addr.to_canonical();
            let key = match addr {
                IpAddr::V4(v4) => v4.to_string(),
                IpAddr::V6(v6) => {
                    let [a, b, c, d, ..] = v6.segments();
                    format!("{}/64", Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0))
                }
            };
            ClientIp {
                ip: addr.to_string(),
                key,
                known: true,
            }
        }
        None => ClientIp {
            ip: UNKNOWN.to_string(),
            key: UNKNOWN.to_string(),
            known: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ported from the original: a client-supplied forwarding header cannot choose the limiter key.
    /// Only `CF-Connecting-IP` reaches the API core; an XFF-style list in it is rejected.
    #[test]
    fn forwarded_header_cannot_spoof_limiter() {
        let v4 = resolve(Some("203.0.113.7"));
        assert_eq!(
            v4,
            ClientIp {
                ip: "203.0.113.7".into(),
                key: "203.0.113.7".into(),
                known: true
            }
        );
        for bad in [
            None,
            Some(""),
            Some("fake, 1.2.3.4"),
            Some("1.2.3.4:80"),
            Some(" 1.2.3.4"),
        ] {
            let r = resolve(bad);
            assert_eq!(
                (r.ip.as_str(), r.key.as_str(), r.known),
                (UNKNOWN, UNKNOWN, false)
            );
        }
        let v6 = resolve(Some("2001:db8:1:2:3:4:5:6"));
        assert_eq!(v6.ip, "2001:db8:1:2:3:4:5:6");
        assert_eq!(v6.key, "2001:db8:1:2::/64");
        assert_eq!(resolve(Some("2001:DB8::1")).key, "2001:db8::/64");
        let mapped = resolve(Some("::ffff:1.2.3.4"));
        assert_eq!(
            (mapped.ip.as_str(), mapped.key.as_str()),
            ("1.2.3.4", "1.2.3.4")
        );
        assert_eq!(resolve(Some("::1")).key, "::/64");
    }
}
