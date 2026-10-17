use axum::http::{HeaderMap, Method, StatusCode, Uri};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::sync::Mutex;
use tokio::time::Instant;

use crate::admin::Loopback;

pub const BODY_LIMIT: usize = 64 * 1024;
pub const HEADER_LIMIT: usize = 16 * 1024;
pub const HEADER_COUNT: usize = 64;
pub const RATE_BURST: u32 = 60;
const RATE_PER_SECOND: f64 = 10.0;

pub struct Guard {
    host: String,
    login: String,
    origin: String,
    // Only one permitted login, and all peers are loopback. A single bucket
    // bounds state and cannot be bypassed by spoofing forwarding/IP headers.
    static_bucket: Mutex<Bucket>,
    api_bucket: Mutex<Bucket>,
}
struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl Guard {
    pub fn new(config: &Loopback) -> Result<Self, &'static str> {
        let authority = config
            .public_host
            .parse::<axum::http::uri::Authority>()
            .map_err(|_| "invalid-host")?;
        let host = authority.host();
        if host.is_empty()
            || host.bytes().any(|b| b.is_ascii_uppercase())
            || host.ends_with('.')
            || authority
                .port()
                .is_some_and(|p| p.as_u16() == 443 || p.as_str() != p.as_u16().to_string())
            || config.public_host.contains(['[', ']', '@'])
            || host.parse::<std::net::IpAddr>().is_ok()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
            || authority.as_str() != config.public_host
            || config.public_host.ends_with(':')
            || (config.public_host.contains(':')
                && authority.port_u16().filter(|p| *p != 0).is_none())
        {
            return Err("invalid-host");
        }
        if config.required_tailscale_login.is_empty()
            || config
                .required_tailscale_login
                .chars()
                .any(char::is_control)
        {
            return Err("invalid-login");
        }
        Ok(Self {
            host: config.public_host.clone(),
            login: config.required_tailscale_login.clone(),
            origin: format!("https://{}", config.public_host),
            api_bucket: Mutex::new(Bucket {
                tokens: f64::from(RATE_BURST),
                updated: Instant::now(),
            }),
            static_bucket: Mutex::new(Bucket {
                tokens: f64::from(RATE_BURST),
                updated: Instant::now(),
            }),
        })
    }

    pub fn check(&self, method: &Method, uri: &Uri, headers: &HeaderMap) -> Result<(), StatusCode> {
        if headers.len() > HEADER_COUNT
            || headers
                .iter()
                .map(|(k, v)| k.as_str().len() + v.len() + 4)
                .sum::<usize>()
                > HEADER_LIMIT
        {
            return Err(StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
        }
        let host = one(headers, "host").ok_or(StatusCode::MISDIRECTED_REQUEST)?;
        // DNS host casing is insignificant, but ports must match exactly.
        if !host.eq_ignore_ascii_case(&self.host)
            || uri.authority().is_some()
            || uri.scheme().is_some()
        {
            return Err(StatusCode::MISDIRECTED_REQUEST);
        }
        let login = one(headers, "tailscale-user-login")
            .and_then(decode_login)
            .ok_or(StatusCode::FORBIDDEN)?;
        // Login identifiers are exact, case-sensitive identities, not DNS names.
        if login != self.login {
            return Err(StatusCode::FORBIDDEN);
        }
        if (method != Method::GET || headers.contains_key("upgrade") || uri.path() == "/ws")
            && one(headers, "origin") != Some(self.origin.as_str())
        {
            return Err(StatusCode::FORBIDDEN);
        }
        if method == Method::GET
            && uri.path().starts_with("/api/")
            && one(headers, "origin") != Some(self.origin.as_str())
            && (headers.contains_key("origin")
                || one(headers, "sec-fetch-site") != Some("same-origin"))
        {
            return Err(StatusCode::FORBIDDEN);
        }
        let now = Instant::now();
        let bucket = if uri.path().starts_with("/api/") || uri.path() == "/ws" {
            &self.api_bucket
        } else {
            &self.static_bucket
        };
        let mut bucket = bucket.lock().map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        bucket.tokens = (bucket.tokens
            + now.duration_since(bucket.updated).as_secs_f64() * RATE_PER_SECOND)
            .min(f64::from(RATE_BURST));
        bucket.updated = now;
        if bucket.tokens < 1.0 {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        bucket.tokens -= 1.0;
        Ok(())
    }
}

fn one<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}

/// Strict RFC 2047 UTF-8 encoded words (Q/B), including adjacent words.
/// Plain ASCII remains exact; no trimming, case folding or mixed plain/encoded
/// identity text. Invalid/ambiguous encodings fail closed.
pub fn decode_login(input: &str) -> Option<String> {
    if !input.starts_with("=?") {
        return (!input.is_empty()
            && input.bytes().all(|b| (33..=126).contains(&b))
            && !input.contains("=?"))
        .then(|| input.to_owned());
    }
    let mut rest = input;
    let mut result = String::new();
    while !rest.is_empty() {
        let charset_end = rest.strip_prefix("=?")?.find('?')? + 2;
        let text_start = rest[charset_end + 1..].find('?')? + charset_end + 2;
        let end = rest[text_start..].find("?=")? + text_start + 2;
        let word = &rest[..end];
        if word.len() > 75 {
            return None;
        }
        let mut parts = word.strip_prefix("=?")?.strip_suffix("?=")?.split('?');
        let charset = parts.next()?;
        let encoding = parts.next()?;
        let text = parts.next()?;
        if !charset.eq_ignore_ascii_case("utf-8") || parts.next().is_some() || text.is_empty() {
            return None;
        }
        let bytes = if encoding.eq_ignore_ascii_case("b") {
            STANDARD.decode(text).ok()?
        } else if encoding.eq_ignore_ascii_case("q") {
            let mut bytes = Vec::new();
            let mut iter = text.bytes();
            while let Some(b) = iter.next() {
                match b {
                    b'_' => bytes.push(b' '),
                    b'=' => bytes.push(hex(iter.next()?)? * 16 + hex(iter.next()?)?),
                    33..=126 if b != b'?' => bytes.push(b),
                    _ => return None,
                }
            }
            bytes
        } else {
            return None;
        };
        result.push_str(std::str::from_utf8(&bytes).ok()?);
        rest = &rest[end..];
        if !rest.is_empty() {
            let trimmed = rest.trim_start_matches([' ', '\t']);
            if trimmed.len() == rest.len() || !trimmed.starts_with("=?") {
                return None;
            }
            rest = trimmed;
        }
    }
    if result.is_empty() || result.chars().any(char::is_control) {
        return None;
    }
    Some(result)
}
fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guard_rfc2047_table() {
        for (input, expected) in [
            ("user@example.com", Some("user@example.com")),
            ("User@example.com", Some("User@example.com")),
            ("=?UTF-8?Q?user=40example.com?=", Some("user@example.com")),
            ("=?utf-8?q?=E4=BD=A0=E5=A5=BD?=", Some("你好")),
            (
                "=?UTF-8?B?dXNlckBleGFtcGxlLmNvbQ==?=",
                Some("user@example.com"),
            ),
            ("=?UTF-8?q?first_last?=", Some("first last")),
            (
                "=?utf-8?b?dXNlcg==?= \t=?UTF-8?Q?=40example.com?=",
                Some("user@example.com"),
            ),
            ("", None),
            (" user@example.com", None),
            ("user@example.com ", None),
            ("=?iso-8859-1?Q?user?=", None),
            ("=?UTF-8?X?user?=", None),
            ("=?UTF-8?Q?=0?=", None),
            ("=?UTF-8?Q?=GG?=", None),
            ("=?UTF-8?Q?=FF?=", None),
            ("=?UTF-8?Q?=00?=", None),
            ("=?UTF-8?Q?=0D=0A?=", None),
            ("=?UTF-8?Q?user name?=", None),
            ("=?UTF-8?B?dQ?=", None),
            ("=?UTF-8?B?dR==?=", None),
            ("=?UTF-8?B?!!!!?=", None),
            ("=?UTF-8?Q??=", None),
            ("=?UTF-8?Q?user?=suffix", None),
            ("x=?UTF-8?Q?user?=", None),
            ("=?UTF-8?Q?user?= ", None),
            ("=?UTF-8?Q?user?=?=", None),
        ] {
            assert_eq!(decode_login(input).as_deref(), expected, "{input:?}");
        }
        assert!(decode_login(&format!("=?UTF-8?Q?{}?=", "x".repeat(65))).is_none());
    }
}
