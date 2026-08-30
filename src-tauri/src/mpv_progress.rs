// New subsystem: captures how far into a movie/series episode mpv actually
// got, and threads a saved position back in as --start on resume. Neither
// existed before — spawn_mpv (movplayer.rs) discarded mpv's process handle
// entirely and never inspected any of its events. See docs/superpowers/specs/
// 2026-09-05-continue-watching-progress-design.md for the full design.
//
// Two independent sources feed the final PlaybackProgressEvent:
// - Position comes from mpv's own --save-position-on-quit feature (a plain
//   text file mpv writes to --watch-later-dir when it quits) — not IPC.
//   mpv's *own* auto-resume matches by URL, which doesn't work here since
//   4KHDHub/MovieBox stream URLs are freshly resolved (and expire) every
//   play; this reads the file directly instead of relying on that matching.
// - Duration isn't something --save-position-on-quit records (it's a
//   source-file property, not a restorable setting), so it needs one IPC
//   round-trip to mpv's --input-ipc-server, made once, then dropped — not a
//   running poll loop.
//
// Every failure here is best-effort and silent by design (explicit user
// direction: "if you can't get the data, just don't display the progress
// bar") — nothing in this file is allowed to affect whether playback itself
// succeeds.

use crate::models::PlaybackProgressEvent;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::process::CommandEvent;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::Receiver;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
// mpv answers "property unavailable" (not silence) if the query lands before
// its file-open completes — a real remote stream commonly takes several
// seconds to open, so this needs to be re-sent, not just awaited once. This
// deadline only bounds that retry loop; track() doesn't await it until mpv
// has already exited (which can be minutes later), so a generous value here
// never slows down a normal run.
const QUERY_TIMEOUT: Duration = Duration::from_secs(20);

pub struct TrackingSetup {
    pipe_name: String,
    watch_later_dir: PathBuf,
}

impl TrackingSetup {
    /// `None` on any setup failure (e.g. can't create the temp dir) —
    /// playback still proceeds untracked rather than failing outright.
    pub fn new() -> Option<Self> {
        let id: u64 = rand::random();
        let watch_later_dir = std::env::temp_dir().join("trela-watch-later").join(format!("{id:016x}"));
        std::fs::create_dir_all(&watch_later_dir).ok()?;
        Some(Self {
            pipe_name: format!(r"\\.\pipe\trela-mpv-{id:016x}"),
            watch_later_dir,
        })
    }

    pub fn mpv_args(&self) -> [String; 3] {
        [
            "--save-position-on-quit".to_string(),
            format!("--watch-later-dir={}", self.watch_later_dir.display()),
            format!("--input-ipc-server={}", self.pipe_name),
        ]
    }
}

// mpv's watch-later files are plain `key=value` lines (plus a leading `#`
// comment line naming the source). Only `start` (seconds into the file) is
// needed here.
fn parse_watch_later_position(content: &str) -> Option<f64> {
    content.lines().find_map(|line| line.strip_prefix("start=")?.trim().parse::<f64>().ok())
}

fn read_watch_later_position(dir: &std::path::Path) -> Option<f64> {
    let entry = std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).next()?;
    let content = std::fs::read_to_string(entry.path()).ok()?;
    parse_watch_later_position(&content)
}

// mpv's IPC socket interleaves unsolicited event notifications with command
// responses — a response line looks like {"data":123.45,"error":"success"};
// an event line has no such "data"/"error":"success" pair, so this skips
// anything that isn't a successful data response instead of assuming the
// first line back is the answer.
fn parse_duration_response(line: &str) -> Option<f64> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("error").and_then(|e| e.as_str()) != Some("success") {
        return None;
    }
    value.get("data")?.as_f64()
}

async fn query_duration(pipe_name: &str) -> Option<f64> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let connect_deadline = Instant::now() + CONNECT_TIMEOUT;
    let client = loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(client) => break client,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(231) => {
                // 231 == ERROR_PIPE_BUSY: mpv hasn't created the pipe yet, or
                // another connection attempt briefly has it — both transient.
                if Instant::now() >= connect_deadline {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(_) => return None,
        }
    };

    let (read_half, mut write_half) = tokio::io::split(client);
    let mut reader = BufReader::new(read_half);
    let query_deadline = Instant::now() + QUERY_TIMEOUT;
    // The IPC pipe connects (~ms) well before mpv has actually opened the
    // source file, especially for a remote HTTPS stream — a query sent that
    // early gets exactly one {"error":"property unavailable"} reply and
    // nothing further, so this re-sends the request each pass instead of
    // asking once and only reading afterward.
    loop {
        if Instant::now() >= query_deadline {
            return None;
        }
        write_half.write_all(b"{\"command\":[\"get_property\",\"duration\"]}\n").await.ok()?;
        let mut line = String::new();
        match tokio::time::timeout(Duration::from_millis(500), reader.read_line(&mut line)).await {
            Ok(Ok(0)) => return None, // pipe closed (mpv exited)
            Ok(Ok(_)) => {
                if let Some(duration) = parse_duration_response(&line) {
                    return Some(duration);
                }
                // "property unavailable" or an unrelated event line — ask again
            }
            Ok(Err(_)) => return None, // read error
            Err(_) => {} // no reply within 500ms — loop and re-send
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Spawns the background task that waits for mpv to exit (draining `rx`
/// exactly like the old inline loop this replaces did, so mpv never blocks
/// on a full stdout/stderr pipe) and, if `setup` is present, reports what it
/// learned. `watch_id` is an opaque token the caller already computed (the
/// same id used for that title's continue-watching entry) — passed back
/// unchanged in the emitted event so the frontend doesn't have to guess
/// which entry a progress report is even for once mpv has been open a while
/// (see this plan's "Deviation from the spec" note).
pub fn track(app: AppHandle, mut rx: Receiver<CommandEvent>, setup: Option<TrackingSetup>, watch_id: String) {
    tauri::async_runtime::spawn(async move {
        let duration_task = setup.as_ref().map(|s| {
            let pipe_name = s.pipe_name.clone();
            tauri::async_runtime::spawn(async move { query_duration(&pipe_name).await })
        });

        while rx.recv().await.is_some() {}

        let Some(setup) = setup else { return };
        let duration_seconds = match duration_task {
            Some(task) => task.await.ok().flatten(),
            None => None,
        };
        let position_seconds = read_watch_later_position(&setup.watch_later_dir);
        let _ = std::fs::remove_dir_all(&setup.watch_later_dir);

        let _ = app.emit(
            "playback-progress",
            PlaybackProgressEvent { watch_id, position_seconds, duration_seconds },
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_watch_later_start_line() {
        let content = "# /some/path or URL\nstart=123.456000\n";
        assert_eq!(parse_watch_later_position(content), Some(123.456));
    }

    #[test]
    fn watch_later_position_missing_when_no_start_line() {
        let content = "# /some/path or URL\nsome-other-key=1\n";
        assert_eq!(parse_watch_later_position(content), None);
    }

    #[test]
    fn parses_successful_duration_response() {
        let line = r#"{"data":2560.789,"error":"success"}"#;
        assert_eq!(parse_duration_response(line), Some(2560.789));
    }

    #[test]
    fn ignores_unrelated_event_lines() {
        let line = r#"{"event":"file-loaded"}"#;
        assert_eq!(parse_duration_response(line), None);
    }

    #[test]
    fn ignores_error_responses() {
        let line = r#"{"error":"property unavailable"}"#;
        assert_eq!(parse_duration_response(line), None);
    }
}
