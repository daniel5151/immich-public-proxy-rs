use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Validates a path/query identifier such as a share key, slug, asset ID, or session token.
/// Rejects empty strings, overly long inputs, and any non-[A-Za-z0-9_-] characters.
pub fn is_safe_param(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validates an Immich shared-link token before forwarding it inside an upstream
/// `Cookie: immich_shared_link_token=...` header.
///
/// Restricts bytes to RFC 6265 `cookie-octet` (`0x21`, `0x23..=0x2B`, `0x2D..=0x3A`,
/// `0x3C..=0x5B`, `0x5D..=0x7E`), explicitly blocking `;`, whitespace, quotes, commas,
/// backslashes, and control characters so a client cannot inject additional upstream cookies.
pub fn is_valid_share_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 512
        && s.bytes().all(|b| {
            matches!(
                b,
                0x21 | 0x23..=0x2B | 0x2D..=0x3A | 0x3C..=0x5B | 0x5D..=0x7E
            )
        })
}

/// Validates and trims an `x-uploader-name` header value before using it as a
/// `SharedBy/{name}` tag in Immich. Rejects empty/whitespace-only values, names over
/// 64 Unicode scalar values, path separators (`/` and `\`), and control characters.
pub fn validate_uploader_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 64 {
        return None;
    }
    if trimmed
        .chars()
        .any(|c| c == '/' || c == '\\' || c.is_control())
    {
        return None;
    }
    Some(trimmed.to_string())
}

/// Computes a per-process keyed SHA-256 digest of a share token so caches can key on
/// `(share_key, Option<[u8; 32]>)` with collision resistance and without storing
/// plaintext tokens in memory.
pub fn hash_token_keyed(token: &str) -> [u8; 32] {
    static PROCESS_KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let key = PROCESS_KEY.get_or_init(|| {
        let mut buf = [0u8; 32];
        getrandom::fill(&mut buf).expect("OS CSPRNG unavailable");
        buf
    });

    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update((token.len() as u64).to_le_bytes());
    hasher.update(token.as_bytes());
    hasher.finalize().into()
}

/// Generates 16 random bytes formatted as lowercase hex (32 ASCII chars) for
/// multipart boundaries or random tokens.
pub fn random_hex_128() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("OS CSPRNG unavailable");
    let mut out = String::with_capacity(32);
    for b in buf {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", b);
    }
    out
}

/// Returns the configured `IPP_PUBLIC_BASE_URL` if set and non-empty.
pub fn configured_public_base_url() -> Option<&'static str> {
    static IPP_PUBLIC_BASE_URL_ENV: OnceLock<Option<String>> = OnceLock::new();
    IPP_PUBLIC_BASE_URL_ENV
        .get_or_init(|| {
            std::env::var("IPP_PUBLIC_BASE_URL")
                .ok()
                .map(|v| v.trim().trim_end_matches('/').to_string())
                .filter(|v| !v.is_empty())
        })
        .as_deref()
}

/// Extracts and validates the scheme from `X-Forwarded-Proto`, accepting only `"http"` or `"https"`.
/// If a comma-separated list is present, inspects the leftmost hop.
pub fn validated_forwarded_proto(headers: &HeaderMap) -> &'static str {
    headers
        .get("x-forwarded-proto")
        .and_then(|p| p.to_str().ok())
        .and_then(|raw| raw.split(',').next())
        .map(str::trim)
        .map(|proto| {
            if proto.eq_ignore_ascii_case("https") {
                "https"
            } else {
                "http"
            }
        })
        .unwrap_or("http")
}

/// Determines whether the incoming request arrived over HTTPS.
///
/// 1. If `X-Forwarded-Proto` is present (e.g. set by Nginx), uses the validated leftmost hop.
/// 2. Otherwise, checks if `IPP_PUBLIC_BASE_URL` is configured as `https://...` AND its
///    authority matches the request's `Host` header (so local debugging over
///    `http://127.0.0.1:3000` still works over plain HTTP even when `IPP_PUBLIC_BASE_URL`
///    is set in `.env`).
pub fn is_https_request(headers: &HeaderMap) -> bool {
    if headers.contains_key("x-forwarded-proto") {
        return validated_forwarded_proto(headers) == "https";
    }
    if let Some(base) = configured_public_base_url() {
        if let Ok(uri) = base.parse::<axum::http::Uri>() {
            let is_https_base = uri
                .scheme_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("https"));
            if is_https_base {
                let req_host = headers
                    .get(header::HOST)
                    .and_then(|h| h.to_str().ok())
                    .map(str::trim);
                if let (Some(base_auth), Some(host)) = (uri.authority(), req_host) {
                    return base_auth.as_str().eq_ignore_ascii_case(host);
                }
            }
        }
    }
    false
}

/// Inserts `Cache-Control: private, no-store` and `Vary: Cookie` on responses that
/// may carry share-specific or cookie-authenticated state.
pub fn insert_private_no_store(headers: &mut HeaderMap) {
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Cookie"));
}

/// Builds a `429 Too Many Requests` response with a `Retry-After` header.
pub fn rate_limited_response(retry_after_secs: u64, body: impl IntoResponse) -> Response {
    let mut resp = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
    if let Ok(val) = HeaderValue::from_str(&retry_after_secs.to_string()) {
        resp.headers_mut().insert(header::RETRY_AFTER, val);
    }
    resp
}

/// Validates a `Host` header value to prevent URL/meta-tag injection when
/// `IPP_PUBLIC_BASE_URL` is not explicitly configured.
fn is_valid_host_header(host: &str) -> bool {
    if host.is_empty() || host.len() > 255 {
        return false;
    }
    // Disallow whitespace, control chars, slashes, userinfo, query, fragment, quotes
    if host.bytes().any(|b| {
        b <= 0x20 || b >= 0x7F || matches!(b, b'/' | b'\\' | b'@' | b'?' | b'#' | b'"' | b'\'')
    }) {
        return false;
    }
    // Must parse cleanly as an HTTP URI authority
    format!("http://{}", host)
        .parse::<axum::http::Uri>()
        .ok()
        .and_then(|u| u.authority().map(|a| a.as_str() == host))
        .unwrap_or(false)
}

/// Resolves the public base URL (`https://example.com`) using `IPP_PUBLIC_BASE_URL`
/// when configured, or validated `X-Forwarded-Proto` + `Host` headers otherwise.
pub fn resolve_public_base_url(headers: &HeaderMap) -> Result<String, &'static str> {
    if let Some(base) = configured_public_base_url() {
        return Ok(base.to_string());
    }

    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::trim)
        .filter(|h| is_valid_host_header(h))
        .ok_or("Host header must be present and valid")?;

    let proto = validated_forwarded_proto(headers);
    Ok(format!("{}://{}", proto, host))
}

/// Validates that the request origin matches the host and scheme, for CSRF protection.
/// Returns true if the request passes the CSRF check (i.e., is same-origin).
pub fn check_csrf(headers: &HeaderMap) -> bool {
    // Prefer the Sec-Fetch-Site header (set by modern browsers, unforgeable)
    if let Some(site) = headers.get("sec-fetch-site") {
        return site == "same-origin";
    }

    // Fallback: compare parsed Origin scheme + authority against expected scheme + Host
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let expected_scheme = if is_https_request(headers) {
        "https"
    } else {
        "http"
    };

    match (origin, host) {
        (Some(o), Some(h)) => match o.parse::<axum::http::Uri>() {
            Ok(uri) => {
                let scheme_ok = uri
                    .scheme_str()
                    .map(|s| s.eq_ignore_ascii_case(expected_scheme))
                    .unwrap_or(false);
                let auth_ok = uri
                    .authority()
                    .map(|auth| auth.as_str().eq_ignore_ascii_case(h))
                    .unwrap_or(false);
                scheme_ok && auth_ok
            }
            Err(_) => false,
        },
        // If neither header is present, deny by default
        _ => false,
    }
}

/// Returns whether reverse-proxy IP headers (`X-Real-IP`, `CF-Connecting-IP`,
/// `X-Forwarded-For`) should be trusted (`IPP_TRUST_PROXY=1|true`). Defaults to `false`.
pub fn trust_proxy_headers() -> bool {
    static TRUST: OnceLock<bool> = OnceLock::new();
    *TRUST.get_or_init(|| {
        std::env::var("IPP_TRUST_PROXY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// Extracts the client IP address for per-IP rate limiting.
///
/// When `trust_proxy` is enabled (`IPP_TRUST_PROXY=1`), checks headers in anti-spoof order:
/// `X-Real-IP` → `CF-Connecting-IP` → rightmost `X-Forwarded-For` entry → `peer`.
/// When disabled (the default), ignores forwarded headers and returns `peer` directly.
pub fn extract_client_ip_with_trust(headers: &HeaderMap, peer: IpAddr, trust_proxy: bool) -> IpAddr {
    if !trust_proxy {
        return peer;
    }

    if let Some(ip) = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
    {
        return ip;
    }

    if let Some(ip) = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
    {
        return ip;
    }

    if let Some(ip) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(',')
                .rev()
                .find_map(|part| part.trim().parse::<IpAddr>().ok())
        })
    {
        return ip;
    }

    peer
}

pub fn extract_client_ip(headers: &HeaderMap, peer: IpAddr) -> IpAddr {
    extract_client_ip_with_trust(headers, peer, trust_proxy_headers())
}

/// Bounded per-IP token-bucket rate limiter.
pub struct RateLimiter {
    max_tokens: f64,
    refill_per_sec: f64,
    max_entries: usize,
    stale_after: Duration,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    pub fn new(max_tokens: f64, refill_per_sec: f64, max_entries: usize) -> Self {
        // An entry becomes stale once it has been idle long enough to fully refill (min 5 min).
        let full_refill_secs = (max_tokens / refill_per_sec).ceil() as u64;
        let stale_after = Duration::from_secs(full_refill_secs.max(300));
        Self {
            max_tokens,
            refill_per_sec,
            max_entries,
            stale_after,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    fn refill_bucket(&self, bucket: &mut Bucket, now: Instant) {
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        if elapsed > 0.0 {
            bucket.tokens = elapsed
                .mul_add(self.refill_per_sec, bucket.tokens)
                .min(self.max_tokens);
            bucket.last_refill = now;
        }
    }

    fn sweep_if_needed(&self, map: &mut HashMap<IpAddr, Bucket>, now: Instant) {
        if map.len() < self.max_entries {
            return;
        }
        map.retain(|_, b| now.duration_since(b.last_refill) < self.stale_after);
        while map.len() >= self.max_entries {
            if let Some(oldest_ip) = map
                .iter()
                .min_by_key(|(_, b)| b.last_refill)
                .map(|(ip, _)| *ip)
            {
                map.remove(&oldest_ip);
            } else {
                break;
            }
        }
    }

    /// Consumes 1 token for `ip`. Returns `Ok(())` if allowed, or `Err(retry_after_secs)` if limited.
    pub fn check(&self, ip: IpAddr) -> Result<(), u64> {
        self.check_at(ip, Instant::now())
    }

    fn check_at(&self, ip: IpAddr, now: Instant) -> Result<(), u64> {
        let mut map = self.buckets.lock();
        if !map.contains_key(&ip) {
            self.sweep_if_needed(&mut map, now);
        }
        let bucket = map.entry(ip).or_insert(Bucket {
            tokens: self.max_tokens,
            last_refill: now,
        });
        self.refill_bucket(bucket, now);
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            let needed = 1.0 - bucket.tokens;
            let retry_after = (needed / self.refill_per_sec).ceil().max(1.0) as u64;
            Err(retry_after)
        }
    }

    /// Non-consuming check: returns `Err(retry_after_secs)` only if `ip` is currently exhausted.
    /// Allows valid share requests to proceed freely while blocking abusive scanners.
    pub fn check_blocked(&self, ip: IpAddr) -> Result<(), u64> {
        self.check_blocked_at(ip, Instant::now())
    }

    fn check_blocked_at(&self, ip: IpAddr, now: Instant) -> Result<(), u64> {
        let mut map = self.buckets.lock();
        let Some(bucket) = map.get_mut(&ip) else {
            return Ok(());
        };
        self.refill_bucket(bucket, now);
        if bucket.tokens >= 1.0 {
            Ok(())
        } else {
            let needed = 1.0 - bucket.tokens;
            let retry_after = (needed / self.refill_per_sec).ceil().max(1.0) as u64;
            Err(retry_after)
        }
    }

    /// Deducts 1 token from `ip` after a failed/invalid probe.
    pub fn penalize(&self, ip: IpAddr) {
        self.penalize_at(ip, Instant::now());
    }

    fn penalize_at(&self, ip: IpAddr, now: Instant) {
        let mut map = self.buckets.lock();
        if !map.contains_key(&ip) {
            self.sweep_if_needed(&mut map, now);
        }
        let bucket = map.entry(ip).or_insert(Bucket {
            tokens: self.max_tokens,
            last_refill: now,
        });
        self.refill_bucket(bucket, now);
        bucket.tokens = (bucket.tokens - 1.0).max(0.0);
    }
}

/// Rate limiter for `POST /share/unlock`: burst 5, refill 1 token per 12s (~5 attempts/min).
pub fn unlock_limiter() -> &'static RateLimiter {
    static LIMITER: OnceLock<RateLimiter> = OnceLock::new();
    LIMITER.get_or_init(|| RateLimiter::new(5.0, 1.0 / 12.0, 8192))
}

/// Failure-only rate limiter for invalid share key probes (`/api/share/{key}`, `/share/{key}`, `/s/{key}`):
/// burst 20 misses, refill 1 token per 2s (~30 misses/min). Valid shares are never penalized.
pub fn probe_limiter() -> &'static RateLimiter {
    static LIMITER: OnceLock<RateLimiter> = OnceLock::new();
    LIMITER.get_or_init(|| RateLimiter::new(20.0, 0.5, 8192))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_is_safe_param() {
        assert!(is_safe_param("abc123_-XYZ"));
        assert!(is_safe_param("bulk_upload_test_album"));
        assert!(!is_safe_param(""));
        assert!(!is_safe_param(&"a".repeat(129)));
        assert!(is_safe_param(&"a".repeat(128)));
        assert!(!is_safe_param("../etc/passwd"));
        assert!(!is_safe_param("foo/bar"));
        assert!(!is_safe_param("foo\nbar"));
        assert!(!is_safe_param("foo bar"));
        assert!(!is_safe_param("foo;bar"));
    }

    #[test]
    fn test_is_valid_share_token() {
        assert!(is_valid_share_token(
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc_123-XYZ.sig+456/789="
        ));
        assert!(!is_valid_share_token(""));
        assert!(!is_valid_share_token("abc; immich_access_token=evil"));
        assert!(!is_valid_share_token("abc;def"));
        assert!(!is_valid_share_token("abc def"));
        assert!(!is_valid_share_token("abc\r\ndef"));
        assert!(!is_valid_share_token("abc\"def"));
        assert!(!is_valid_share_token("abc\\def"));
        assert!(!is_valid_share_token("abc,def"));
        assert!(!is_valid_share_token(&"a".repeat(513)));
    }

    #[test]
    fn test_validate_uploader_name() {
        assert_eq!(validate_uploader_name("  Alice  "), Some("Alice".into()));
        assert_eq!(
            validate_uploader_name("José García"),
            Some("José García".into())
        );
        assert_eq!(validate_uploader_name(""), None);
        assert_eq!(validate_uploader_name("   \t  "), None);
        assert_eq!(validate_uploader_name("Alice/Bob"), None);
        assert_eq!(validate_uploader_name("Alice\\Bob"), None);
        assert_eq!(validate_uploader_name("Alice\nBob"), None);
        assert_eq!(validate_uploader_name("Alice\x1b[31m"), None);
        assert_eq!(validate_uploader_name(&"a".repeat(65)), None);
        assert_eq!(validate_uploader_name(&"a".repeat(64)), Some("a".repeat(64)));
    }

    #[test]
    fn test_validated_forwarded_proto() {
        let mut h = HeaderMap::new();
        assert_eq!(validated_forwarded_proto(&h), "http");

        h.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert_eq!(validated_forwarded_proto(&h), "https");

        h.insert(
            "x-forwarded-proto",
            HeaderValue::from_static("HTTPS, http"),
        );
        assert_eq!(validated_forwarded_proto(&h), "https");

        h.insert("x-forwarded-proto", HeaderValue::from_static("javascript"));
        assert_eq!(validated_forwarded_proto(&h), "http");
    }

    #[test]
    fn test_check_csrf_scheme_and_authority() {
        let mut h = HeaderMap::new();
        h.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(check_csrf(&h));

        h.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert!(!check_csrf(&h));

        // Origin fallback: matching scheme + host
        let mut h = HeaderMap::new();
        h.insert("host", HeaderValue::from_static("photos.example.com"));
        h.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        h.insert(
            "origin",
            HeaderValue::from_static("https://photos.example.com"),
        );
        assert!(check_csrf(&h));

        // Scheme mismatch (http origin against https host) must be rejected
        h.insert(
            "origin",
            HeaderValue::from_static("http://photos.example.com"),
        );
        assert!(!check_csrf(&h));

        // Host mismatch must be rejected
        h.insert("origin", HeaderValue::from_static("https://evil.example.com"));
        assert!(!check_csrf(&h));
    }

    #[test]
    fn test_extract_client_ip_precedence() {
        let peer = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
        let mut h = HeaderMap::new();
        h.insert("x-real-ip", HeaderValue::from_static("203.0.113.5"));
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("1.1.1.1, 198.51.100.7"),
        );

        // Untrusted proxy: ignores headers
        assert_eq!(extract_client_ip_with_trust(&h, peer, false), peer);

        // Trusted proxy: prefers X-Real-IP
        assert_eq!(
            extract_client_ip_with_trust(&h, peer, true),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5))
        );

        // Without X-Real-IP, uses rightmost X-Forwarded-For (anti-spoof)
        h.remove("x-real-ip");
        assert_eq!(
            extract_client_ip_with_trust(&h, peer, true),
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7))
        );

        // CF-Connecting-IP wins over X-Forwarded-For
        h.insert("cf-connecting-ip", HeaderValue::from_static("2001:db8::1"));
        assert_eq!(
            extract_client_ip_with_trust(&h, peer, true),
            IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1))
        );
    }

    #[test]
    fn test_rate_limiter_burst_refill_and_cap() {
        let rl = RateLimiter::new(2.0, 0.5, 2); // burst 2, 1 token per 2s, max 2 IPs
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        let ip3 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));
        let t0 = Instant::now();

        assert!(rl.check_at(ip1, t0).is_ok());
        assert!(rl.check_at(ip1, t0).is_ok());
        assert!(rl.check_at(ip1, t0).is_err());

        // After 2 seconds, 1 token has refilled
        let t1 = t0 + Duration::from_secs(2);
        assert!(rl.check_at(ip1, t1).is_ok());
        assert!(rl.check_at(ip1, t1).is_err());

        // Max entries eviction keeps map bounded at 2
        assert!(rl.check_at(ip2, t1 + Duration::from_millis(1)).is_ok());
        assert!(rl.check_at(ip3, t1 + Duration::from_millis(2)).is_ok());
        assert!(rl.buckets.lock().len() <= 2);
    }

    #[test]
    fn test_probe_limiter_non_consuming_check() {
        let rl = RateLimiter::new(2.0, 0.5, 16);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9));
        let t0 = Instant::now();

        // Repeated check_blocked without penalize never exhausts bucket
        for _ in 0..10 {
            assert!(rl.check_blocked_at(ip, t0).is_ok());
        }
        rl.penalize_at(ip, t0);
        assert!(rl.check_blocked_at(ip, t0).is_ok());
        rl.penalize_at(ip, t0);
        assert!(rl.check_blocked_at(ip, t0).is_err());
    }
}
