use std::future::Future;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// A single shared `reqwest::Client` for the AniList/TMDB HTTP calls.
/// `reqwest::Client` is already `Arc`-backed internally, so cloning it just
/// bumps a refcount and reuses the same connection pool/TLS session cache
/// instead of paying a fresh handshake on every request.
pub fn http_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new).clone()
}

/// Finds an executable on PATH, trying each candidate name in order.
/// Windows needs the exact .exe/.cmd/.bat variant since `where` won't
/// resolve a bare extensionless script.
pub fn find_exe(names: &[&str]) -> Option<String> {
    let finder = if cfg!(target_os = "windows") { "where" } else { "which" };
    for name in names {
        if let Ok(output) = Command::new(finder).arg(name).output() {
            if output.status.success() {
                if let Some(line) = String::from_utf8_lossy(&output.stdout).lines().next() {
                    return Some(line.trim().to_string());
                }
            }
        }
    }
    None
}

/// Shared retry/backoff skeleton for the reqwest-based JSON HTTP clients
/// (AniList, TMDB): a few attempts, backing off more on 429s, giving up
/// immediately on non-transient errors. `send` issues one fresh request per
/// attempt (a `reqwest::RequestBuilder` is consumed by `.send()`, so it
/// can't just be retried in place).
pub async fn json_with_retry<F, Fut>(service: &str, send: F) -> Result<serde_json::Value, String>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let mut last_err = String::new();

    for attempt in 0..3u32 {
        if attempt > 0 {
            let backoff_ms = if last_err.contains("429") { 1200 } else { 500 };
            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms * attempt as u64)).await;
        }

        let resp = match send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("Network error contacting {}: {}", service, e);
                continue;
            }
        };

        let status = resp.status();
        if status.is_success() {
            return resp
                .json::<serde_json::Value>()
                .await
                .map_err(|e| format!("Failed to parse {} response: {}", service, e));
        }

        last_err = format!("{} request failed: HTTP {}", service, status);
        if !matches!(status.as_u16(), 429 | 502 | 503 | 504) {
            break;
        }
    }

    Err(last_err)
}

/// Picks a pseudo-random integer in `0..bound` (0 if `bound` is 0) — good
/// enough for "shuffle me a random pick", not a cryptographic use. Seeded
/// from wall-clock nanos plus a call counter (so two calls within the same
/// nanosecond, e.g. two rapid Shuffle clicks, still diverge) and mixed with
/// splitmix64 to spread that seed out, avoiding a `rand` crate for this.
pub fn random_below(bound: u64) -> u64 {
    if bound == 0 {
        return 0;
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut z = nanos.wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E3779B97F4A7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    z % bound
}

/// Minimal query-string escaping — avoids pulling in a whole crate just for
/// `?query=<text>` where text may contain spaces/&/#.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}
