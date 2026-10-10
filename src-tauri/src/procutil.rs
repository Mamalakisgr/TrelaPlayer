use rand::Rng;
use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::process::Command;
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

/// Shown wherever anime playback fails because a tool it needs is missing.
pub const ANIME_SETUP_HINT: &str =
    "Anime playback isn't set up yet — open the About page and click \"Set Up Anime Playback\".";

/// Where scripts/setup-anime.ps1 installs anime playback's tools. Trela
/// looks here itself instead of relying on PATH, which the setup script
/// doesn't touch (and which a running Trela wouldn't see change anyway).
pub fn tools_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("Trela").join("tools"))
}

/// `find_exe`, falling back to the tools dir — so a tool the user already
/// had on PATH (e.g. a Scoop install) keeps winning over Trela's copy.
pub fn find_tool(names: &[&str]) -> Option<String> {
    find_exe(names).or_else(|| {
        let dir = tools_dir()?;
        names.iter().map(|name| dir.join(name)).find(|path| path.is_file()).map(|path| path.display().to_string())
    })
}

/// PATH for ani-cli's process: the user's own PATH first, then the tools dir
/// (fzf, curl_chrome116) and Trela's own folder, home of the bundled mpv.exe
/// sidecar — a machine without mpv still plays anime, one with its own mpv
/// keeps using it.
pub fn tool_path_env() -> OsString {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    dirs.extend(tools_dir());
    dirs.extend(std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from)));
    std::env::join_paths(dirs).unwrap_or(path)
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
        // Both AniList (GraphQL errors[0].message) and TMDB (status_message)
        // put a real explanation in the error body — worth surfacing instead
        // of just the bare status code, e.g. AniList returning 403 with body
        // {"errors":[{"message":"The AniList API has been temporarily
        // disabled due to severe stability issues."}]} reads as some kind of
        // auth/permission problem as a bare "HTTP 403" when it's actually a
        // site-wide outage on AniList's own end.
        if let Ok(body) = serde_json::from_str::<serde_json::Value>(&resp.text().await.unwrap_or_default()) {
            let detail = body["errors"][0]["message"].as_str().or_else(|| body["status_message"].as_str());
            if let Some(detail) = detail {
                last_err = format!("{}: {}", last_err, detail);
            }
        }
        if !matches!(status.as_u16(), 429 | 502 | 503 | 504) {
            break;
        }
    }

    Err(last_err)
}

/// Picks a pseudo-random integer in `0..bound` (0 if `bound` is 0) — good
/// enough for "shuffle me a random pick", not a cryptographic use.
pub fn random_below(bound: u64) -> u64 {
    if bound == 0 {
        return 0;
    }
    rand::rng().random_range(0..bound)
}

/// Query-string escaping for a `?query=<text>` value — same unreserved set
/// (letters/digits/`-_.~`) the old hand-rolled version used.
pub fn urlencode(s: &str) -> String {
    const QUERY_SAFE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');
    percent_encoding::utf8_percent_encode(s, QUERY_SAFE).to_string()
}
