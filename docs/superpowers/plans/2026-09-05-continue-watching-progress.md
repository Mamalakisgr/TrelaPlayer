# Continue Watching Progress Bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show a real playback-progress bar on Continue Watching cards (movie/series only) and resume mpv from where the user left off when they click a card.

**Architecture:** A new `mpv_progress` Rust module captures position via mpv's own `--save-position-on-quit` file (not IPC) and duration via one *one-shot* mpv JSON-IPC query made right after launch — not a running poll loop. Both existing mpv-spawn sites (`movplayer.rs::spawn_mpv`, `fourkhdhub.rs::play_fourk_stream`) gain a `--start=<seconds>` resume flag and report a `playback-progress` Tauri event once mpv exits. The frontend already has all the continue-watching storage/rendering machinery (`continueWatching.js`, `renderContinueWatching` in `main.js`) — this plugs into it rather than replacing it.

**Tech Stack:** Rust (Tauri backend, `tokio` named pipes on Windows), vanilla JS frontend, no new dependencies (tokio needs one additional feature flag it doesn't currently enable).

**Spec:** `docs/superpowers/specs/2026-09-05-continue-watching-progress-design.md`

## Global Constraints

- Movie/series only — anime (ani-cli) is explicitly out of scope (spec's Problem section: ani-cli picks its own player, not reliably mpv).
- Every failure mode here is best-effort and silent — per explicit user direction, "if you can't get the data, just don't display the progress bar." Nothing in this feature may affect whether playback itself succeeds or block/delay it.
- Resume-seek applies only when launched via a Continue Watching card's click (`resumeContinueWatching`) — picking the same episode fresh from Browse/Details never auto-seeks.
- No new crate dependencies. `tokio`'s existing `net` feature flag is the only `Cargo.toml` change.
- **Deviation from the spec's exact wording, decided during planning:** the spec's §4 says the emitted event carries no id ("the frontend already knows which title this belongs to from `pendingPlay`"), matching `DownloadCompleteEvent`'s existing no-id pattern. This plan instead includes a `watch_id` (the same id string `saveContinueWatching` already uses) in the payload. Reason: `pendingPlay` is a single mutable global that can change before mpv's *own* process actually closes (nothing stops starting a second title while an earlier mpv window is still open) — trusting it at event-fire time risks writing one title's progress onto a different title's continue-watching entry. Threading an explicit id through costs one extra string parameter and removes the risk entirely.

---

## Task 1: `mpv_progress.rs` — core module (position/duration capture, no UI wiring yet)

**Files:**
- Modify: `src-tauri/Cargo.toml` (add `net` to tokio's features)
- Modify: `src-tauri/src/models.rs` (add `PlaybackProgressEvent`)
- Create: `src-tauri/src/mpv_progress.rs`
- Modify: `src-tauri/src/lib.rs` (register the new module)

**Interfaces:**
- Produces (used by Tasks 2 and 3):
  - `mpv_progress::TrackingSetup::new() -> Option<TrackingSetup>`
  - `TrackingSetup::mpv_args(&self) -> [String; 3]`
  - `mpv_progress::track(app: AppHandle, rx: tokio::sync::mpsc::Receiver<tauri_plugin_shell::process::CommandEvent>, setup: Option<TrackingSetup>, watch_id: String)`
  - `models::PlaybackProgressEvent { watch_id: String, position_seconds: Option<f64>, duration_seconds: Option<f64> }`

- [ ] **Step 1: Add tokio's `net` feature**

In `src-tauri/Cargo.toml`, change:
```toml
tokio = { version = "1", features = ["time", "rt", "sync", "fs"] }
```
to:
```toml
tokio = { version = "1", features = ["time", "rt", "sync", "fs", "net"] }
```
This is what gates `tokio::net::windows::named_pipe` (confirmed against the installed `tokio-1.53.1` source: the module is declared under `#[cfg(windows)]` + `#[cfg(feature = "net")]` in `net/mod.rs`).

- [ ] **Step 2: Add the event struct**

In `src-tauri/src/models.rs`, add next to the existing `DownloadCompleteEvent`/`DownloadErrorEvent`:
```rust
#[derive(Serialize, Clone)]
pub struct PlaybackProgressEvent {
    pub watch_id: String,
    pub position_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
}
```

- [ ] **Step 3: Write `mpv_progress.rs`**

Create `src-tauri/src/mpv_progress.rs` with this exact content:

```rust
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
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

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

#[cfg(windows)]
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
    write_half.write_all(b"{\"command\":[\"get_property\",\"duration\"]}\n").await.ok()?;

    let mut reader = BufReader::new(read_half);
    let query_deadline = Instant::now() + QUERY_TIMEOUT;
    loop {
        let remaining = query_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let mut line = String::new();
        match tokio::time::timeout(remaining, reader.read_line(&mut line)).await {
            Ok(Ok(0)) => return None, // pipe closed
            Ok(Ok(_)) => {
                if let Some(duration) = parse_duration_response(&line) {
                    return Some(duration);
                }
                // an unrelated event line — keep reading within the deadline
            }
            _ => return None, // read error or timed out
        }
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
```

- [ ] **Step 4: Register the module**

In `src-tauri/src/lib.rs`, add to the `mod` list (alphabetical, matching the existing order):
```rust
mod mpv_progress;
```

- [ ] **Step 5: Run the real unit tests**

Run: `cd src-tauri && cargo test --lib mpv_progress:: -- --nocapture`
Expected: 5 tests pass (`parses_watch_later_start_line`, `watch_later_position_missing_when_no_start_line`, `parses_successful_duration_response`, `ignores_unrelated_event_lines`, `ignores_error_responses`).

- [ ] **Step 6: Verify the whole crate still compiles**

Run: `cd src-tauri && cargo check --message-format=short`
Expected: `Finished` with no errors. A warning that `TrackingSetup`/`track` are unused is expected and fine — Tasks 2/3 wire them in.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/models.rs src-tauri/src/mpv_progress.rs src-tauri/src/lib.rs
git commit -m "Add mpv_progress: capture playback position/duration on exit

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 2: Wire `mpv_progress` into `movplayer.rs`

**Files:**
- Modify: `src-tauri/src/movplayer.rs:369-405` (`spawn_mpv`)
- Modify: `src-tauri/src/movplayer.rs:457-468` (`play_stream`)
- Modify: `src-tauri/src/movplayer.rs:561` (the `start_download` completion handler's `spawn_mpv` call)

**Interfaces:**
- Consumes: `mpv_progress::TrackingSetup::new/mpv_args`, `mpv_progress::track` (Task 1)
- Produces: `play_stream` command gains two new params (`start_seconds: Option<f64>`, `watch_id: String>`) — Task 4's frontend changes depend on this exact signature.

- [ ] **Step 1: Update `spawn_mpv`'s signature and body**

Replace the current `spawn_mpv` (movplayer.rs:369-405) — note the two comment blocks below (demuxer cache sizing, and the drain-loop rationale) are the file's real, current comments; keep the plan doc's earlier stripped-down quote out of mind, this is what's actually there:
```rust
fn spawn_mpv(app: &AppHandle, url: &str, window_title: &str, sub_files: &[String]) -> Result<(), String> {
    let mut sidecar = app.shell().sidecar("mpv").map_err(|e| format!("Failed to resolve bundled mpv: {}", e))?;
    sidecar = sidecar.arg(format!("--force-media-title={}", window_title));
    if url.starts_with("http://") || url.starts_with("https://") {
        // mpv's default demuxer cache (150MiB forward / 50MiB back) is sized
        // for local files, not a multi-GB remote episode — that thin a
        // buffer underruns often on a real network stream, and mpv pauses
        // on underrun by default (--cache-pause), which is what actually
        // shows up as "slow"/stuttery playback. Also force caching on
        // explicitly rather than relying on mpv's own URL auto-detection.
        //
        // Separately: a total file size being small (e.g. ~1.8GB) doesn't
        // protect against a mid-stream stall — a third-party CDN/mirror
        // dropping or hanging the connection looks identical to a freeze
        // regardless of file size. mpv has no auto-reconnect by default, so
        // that stall never recovers on its own; ffmpeg's own reconnect
        // options (mpv's network demuxers run on ffmpeg) make it retry
        // instead of hanging.
        sidecar = sidecar.args([
            "--cache=yes",
            "--demuxer-max-bytes=500MiB",
            "--demuxer-max-back-bytes=150MiB",
            "--stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_at_eof=1,reconnect_delay_max=5",
        ]);
    }
    for sub in sub_files {
        sidecar = sidecar.arg(format!("--sub-file={}", sub));
    }
    sidecar = sidecar.arg(url);

    let (mut rx, _child) = sidecar.spawn().map_err(|e| format!("Failed to launch mpv: {}", e))?;
    // The sidecar always pipes stdout/stderr; draining and discarding them
    // (playback is fire-and-forget, nothing here needs mpv's output) avoids
    // mpv blocking on a full pipe buffer during a long viewing session.
    tauri::async_runtime::spawn(async move { while rx.recv().await.is_some() {} });
    Ok(())
}
```
with — the demuxer-cache comment stays (still fully accurate, unrelated to this change); the drain-loop comment does not (that logic now lives in `mpv_progress::track`, which already documents it there — see Task 1):
```rust
fn spawn_mpv(
    app: &AppHandle,
    url: &str,
    window_title: &str,
    sub_files: &[String],
    start_seconds: Option<f64>,
    watch_id: String,
) -> Result<(), String> {
    let mut sidecar = app.shell().sidecar("mpv").map_err(|e| format!("Failed to resolve bundled mpv: {}", e))?;
    sidecar = sidecar.arg(format!("--force-media-title={}", window_title));
    if let Some(start) = start_seconds {
        sidecar = sidecar.arg(format!("--start={start}"));
    }
    let tracking = crate::mpv_progress::TrackingSetup::new();
    if let Some(setup) = &tracking {
        sidecar = sidecar.args(setup.mpv_args());
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        // mpv's default demuxer cache (150MiB forward / 50MiB back) is sized
        // for local files, not a multi-GB remote episode — that thin a
        // buffer underruns often on a real network stream, and mpv pauses
        // on underrun by default (--cache-pause), which is what actually
        // shows up as "slow"/stuttery playback. Also force caching on
        // explicitly rather than relying on mpv's own URL auto-detection.
        //
        // Separately: a total file size being small (e.g. ~1.8GB) doesn't
        // protect against a mid-stream stall — a third-party CDN/mirror
        // dropping or hanging the connection looks identical to a freeze
        // regardless of file size. mpv has no auto-reconnect by default, so
        // that stall never recovers on its own; ffmpeg's own reconnect
        // options (mpv's network demuxers run on ffmpeg) make it retry
        // instead of hanging.
        sidecar = sidecar.args([
            "--cache=yes",
            "--demuxer-max-bytes=500MiB",
            "--demuxer-max-back-bytes=150MiB",
            "--stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_at_eof=1,reconnect_delay_max=5",
        ]);
    }
    for sub in sub_files {
        sidecar = sidecar.arg(format!("--sub-file={}", sub));
    }
    sidecar = sidecar.arg(url);

    let (rx, _child) = sidecar.spawn().map_err(|e| format!("Failed to launch mpv: {}", e))?;
    crate::mpv_progress::track(app.clone(), rx, tracking, watch_id);
    Ok(())
}
```
(All existing doc comments above `spawn_mpv` stay as-is — only the signature and body change.)

- [ ] **Step 2: Update `play_stream`**

Replace (movplayer.rs:457-468):
```rust
pub async fn play_stream(
    app: AppHandle,
    resource_link: String,
    window_title: String,
    subject_id: String,
    resource_id: String,
) -> Result<String, String> {
    let client = get_client().await?;
    let sub_files = fetch_caption_urls(&client, &subject_id, &resource_id).await;
    spawn_mpv(&app, &resource_link, &window_title, &sub_files)?;
    Ok(format!("Playing \"{}\"", window_title))
}
```
with:
```rust
pub async fn play_stream(
    app: AppHandle,
    resource_link: String,
    window_title: String,
    subject_id: String,
    resource_id: String,
    start_seconds: Option<f64>,
    watch_id: String,
) -> Result<String, String> {
    let client = get_client().await?;
    let sub_files = fetch_caption_urls(&client, &subject_id, &resource_id).await;
    spawn_mpv(&app, &resource_link, &window_title, &sub_files, start_seconds, watch_id)?;
    Ok(format!("Playing \"{}\"", window_title))
}
```

- [ ] **Step 3: Update the `start_download` completion handler's `spawn_mpv` call**

At movplayer.rs:561, replace:
```rust
                if let Err(e) = spawn_mpv(&app, &path, &window_title, &sub_files) {
```
with:
```rust
                let watch_id = if season == 0 && episode == 0 { format!("movie:{title}") } else { format!("series:{title}") };
                if let Err(e) = spawn_mpv(&app, &path, &window_title, &sub_files, None, watch_id) {
```
(Download-and-play doesn't currently save a continue-watching entry at all — this just keeps the id convention consistent for if/when it does; `None` for `start_seconds` because downloaded local files always start from the beginning today.)

- [ ] **Step 4: Verify it compiles**

Run: `cd src-tauri && cargo check --message-format=short`
Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/movplayer.rs
git commit -m "Thread mpv progress tracking + resume-seek through play_stream

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 3: Wire `mpv_progress` into `fourkhdhub.rs`

**Files:**
- Modify: `src-tauri/src/fourkhdhub.rs:246-310` (`play_fourk_stream`)

**Interfaces:**
- Consumes: `mpv_progress::TrackingSetup`, `mpv_progress::track` (Task 1)
- Produces: `play_fourk_stream` gains the same two new params as `play_stream` — Task 4 depends on this signature matching Task 2's exactly (same param names/order convention).

- [ ] **Step 1: Update `play_fourk_stream`'s signature**

Change (fourkhdhub.rs:246):
```rust
pub async fn play_fourk_stream(app: AppHandle, releases: Vec<Release>, window_title: String) -> Result<String, String> {
```
to:
```rust
pub async fn play_fourk_stream(
    app: AppHandle,
    releases: Vec<Release>,
    window_title: String,
    start_seconds: Option<f64>,
    watch_id: String,
) -> Result<String, String> {
```

- [ ] **Step 2: Add the resume flag and tracking setup to the sidecar build**

Replace (fourkhdhub.rs:279-296):
```rust
    // mpv runs as a bundled sidecar (see movplayer.rs's spawn_mpv for why —
    // same reasoning applies to this provider).
    let mut sidecar = app.shell().sidecar("mpv").map_err(|e| format!("Failed to resolve bundled mpv: {}", e))?;
    sidecar = sidecar.arg(format!("--force-media-title={}", window_title));
    // See spawn_mpv in movplayer.rs for why: mpv's default demuxer cache is
    // sized for local files, not a multi-GB remote episode through a
    // third-party mirror, and underruns (mpv pauses on those by default)
    // show up as "slow"/stuttery playback. The reconnect option matters even
    // more here than for MovieBox — hubcloud/hubdrive mirrors are more prone
    // to mid-stream hiccups, and without it a dropped connection just hangs
    // instead of resuming, which looks like a freeze no matter how small the
    // file is.
    sidecar = sidecar.args([
        "--cache=yes",
        "--demuxer-max-bytes=500MiB",
        "--demuxer-max-back-bytes=150MiB",
        "--stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_at_eof=1,reconnect_delay_max=5",
    ]);
```
with:
```rust
    // mpv runs as a bundled sidecar (see movplayer.rs's spawn_mpv for why —
    // same reasoning applies to this provider).
    let mut sidecar = app.shell().sidecar("mpv").map_err(|e| format!("Failed to resolve bundled mpv: {}", e))?;
    sidecar = sidecar.arg(format!("--force-media-title={}", window_title));
    if let Some(start) = start_seconds {
        sidecar = sidecar.arg(format!("--start={start}"));
    }
    let tracking = crate::mpv_progress::TrackingSetup::new();
    if let Some(setup) = &tracking {
        sidecar = sidecar.args(setup.mpv_args());
    }
    // See spawn_mpv in movplayer.rs for why: mpv's default demuxer cache is
    // sized for local files, not a multi-GB remote episode through a
    // third-party mirror, and underruns (mpv pauses on those by default)
    // show up as "slow"/stuttery playback. The reconnect option matters even
    // more here than for MovieBox — hubcloud/hubdrive mirrors are more prone
    // to mid-stream hiccups, and without it a dropped connection just hangs
    // instead of resuming, which looks like a freeze no matter how small the
    // file is.
    sidecar = sidecar.args([
        "--cache=yes",
        "--demuxer-max-bytes=500MiB",
        "--demuxer-max-back-bytes=150MiB",
        "--stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_at_eof=1,reconnect_delay_max=5",
    ]);
```

- [ ] **Step 3: Replace the drain loop with `mpv_progress::track`**

Replace (fourkhdhub.rs:306-307):
```rust
    let (mut rx, _child) = sidecar.spawn().map_err(|e| format!("Failed to launch mpv: {}", e))?;
    tauri::async_runtime::spawn(async move { while rx.recv().await.is_some() {} });
```
with:
```rust
    let (rx, _child) = sidecar.spawn().map_err(|e| format!("Failed to launch mpv: {}", e))?;
    crate::mpv_progress::track(app.clone(), rx, tracking, watch_id);
```

- [ ] **Step 4: Verify it compiles**

Run: `cd src-tauri && cargo check --message-format=short`
Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/fourkhdhub.rs
git commit -m "Thread mpv progress tracking + resume-seek through play_fourk_stream

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 4: Frontend plumbing — API calls, event subscription, storage

**Files:**
- Modify: `src/js/api.js`
- Modify: `src/js/continueWatching.js`
- Modify: `src/js/main.js`

**Interfaces:**
- Consumes: `play_stream`/`play_fourk_stream`'s new `start_seconds`/`watch_id` params (Tasks 2, 3); the `playback-progress` event with `{watch_id, position_seconds, duration_seconds}` (Task 1)
- Produces: `continueWatching.js` entries gain `positionSeconds`/`durationSeconds` fields — Task 5's rendering depends on these exact field names.

- [ ] **Step 1: Add `startSeconds`/`watchId` to the two play API calls**

In `src/js/api.js`, replace:
```js
  playStream: (resourceLink, windowTitle, subjectId, resourceId) => invoke('play_stream', { resourceLink, windowTitle, subjectId, resourceId }),
```
with:
```js
  playStream: (resourceLink, windowTitle, subjectId, resourceId, startSeconds, watchId) =>
    invoke('play_stream', { resourceLink, windowTitle, subjectId, resourceId, startSeconds: startSeconds || undefined, watchId }),
```
and replace:
```js
  playFourkStream: (releases, windowTitle) => invoke('play_fourk_stream', { releases, windowTitle }),
```
with:
```js
  playFourkStream: (releases, windowTitle, startSeconds, watchId) =>
    invoke('play_fourk_stream', { releases, windowTitle, startSeconds: startSeconds || undefined, watchId }),
```

- [ ] **Step 2: Add the progress-merge helper to `continueWatching.js`**

Add to `src/js/continueWatching.js` (after `saveContinueWatching`):
```js
// Called when mpv reports how far playback actually got (see the
// playback-progress event in main.js) — merges into the existing entry
// without touching its other fields, and no-ops if the entry was removed
// (e.g. the user cleared it) while still watching.
export function updateContinueWatchingProgress(id, positionSeconds, durationSeconds) {
  const existing = getContinueWatching().find((e) => e.id === id);
  if (!existing) return;
  store.upsert({ ...existing, positionSeconds, durationSeconds }, 'updatedAt');
}
```

- [ ] **Step 3: Import the new helper in `main.js`**

Change (main.js:4):
```js
import { getContinueWatching, saveContinueWatching, removeContinueWatching } from './continueWatching.js';
```
to:
```js
import { getContinueWatching, saveContinueWatching, removeContinueWatching, updateContinueWatchingProgress } from './continueWatching.js';
```

- [ ] **Step 4: Compute and thread `watchId` through `startPlayback`'s movie/series branch**

In `main.js`, in `startPlayback`'s `else if (pendingPlay.kind === 'movie' || pendingPlay.kind === 'series')` branch, replace:
```js
  const windowTitle = pendingPlay.kind === 'movie'
    ? pendingPlay.title
    : `${pendingPlay.title} S${pendingPlay.season}E${pendingPlay.episode}`;
  statusEl.textContent = 'Starting playback...';
  const playViaOption = (opt) =>
    opt.fourk_release
      ? api.playFourkStream(
          [opt.fourk_release, ...currentStreamOptions.filter((o) => o !== opt && o.fourk_release).map((o) => o.fourk_release)],
          windowTitle
        )
      : api.playStream(opt.resource_link, windowTitle, opt.subject_id, opt.resource_id);
```
with:
```js
  const windowTitle = pendingPlay.kind === 'movie'
    ? pendingPlay.title
    : `${pendingPlay.title} S${pendingPlay.season}E${pendingPlay.episode}`;
  // Shared with saveContinueWatching below and threaded into the play calls
  // so the eventual playback-progress event (see initPlaybackProgressEvents)
  // can be matched back to this exact entry even if the user has since
  // navigated elsewhere by the time mpv actually closes.
  const watchId = pendingPlay.kind === 'movie' ? `movie:${pendingPlay.title}` : `series:${pendingPlay.title}`;
  statusEl.textContent = 'Starting playback...';
  const playViaOption = (opt) =>
    opt.fourk_release
      ? api.playFourkStream(
          [opt.fourk_release, ...currentStreamOptions.filter((o) => o !== opt && o.fourk_release).map((o) => o.fourk_release)],
          windowTitle,
          pendingPlay.resumeSeconds,
          watchId
        )
      : api.playStream(opt.resource_link, windowTitle, opt.subject_id, opt.resource_id, pendingPlay.resumeSeconds, watchId);
```
Then replace the existing `saveContinueWatching` call right after the try/catch block — the real file wraps the series branch's object across several lines (shown exactly below), not the single line an earlier draft of this plan condensed it to:
```js
  // id is kind-prefixed (unlike the bare-title anime id above) so a movie
  // and an anime that happen to share a title don't overwrite each other.
  saveContinueWatching(
    pendingPlay.kind === 'movie'
      ? { id: `movie:${pendingPlay.title}`, kind: 'movie', title: pendingPlay.title, year: pendingPlay.year, image_url: pendingPlay.image_url }
      : {
          id: `series:${pendingPlay.title}`,
          kind: 'series',
          title: pendingPlay.title,
          year: pendingPlay.year,
          season: pendingPlay.season,
          episode: pendingPlay.episode,
          tmdb_id: pendingPlay.tmdb_id,
          image_url: pendingPlay.image_url,
        }
  );
```
with (reusing `watchId` instead of recomputing the same string; the leading comment is unchanged and stays):
```js
  // id is kind-prefixed (unlike the bare-title anime id above) so a movie
  // and an anime that happen to share a title don't overwrite each other.
  saveContinueWatching(
    pendingPlay.kind === 'movie'
      ? { id: watchId, kind: 'movie', title: pendingPlay.title, year: pendingPlay.year, image_url: pendingPlay.image_url }
      : {
          id: watchId,
          kind: 'series',
          title: pendingPlay.title,
          year: pendingPlay.year,
          season: pendingPlay.season,
          episode: pendingPlay.episode,
          tmdb_id: pendingPlay.tmdb_id,
          image_url: pendingPlay.image_url,
        }
  );
```

- [ ] **Step 5: Thread a saved position through `resumeContinueWatching` → `selectMovieToWatch`/`selectSeriesEpisodeToWatch`**

Replace `selectMovieToWatch` (main.js:1574-1586) — keep its existing leading comment ("Dub/language picking...") unchanged:
```js
async function selectMovieToWatch(movie) {
  enterWatching(
    { kind: 'movie', title: movie.title, year: movie.year, image_url: movie.image_url },
    { backTarget: 'movie-details', crumbTitle: movie.title, metaHtml: `<h1>${movie.title}</h1>` }
  );
  // Dub/language picking is MovieBox-specific machinery, but MovieBox is
  // always one of the merged sources now, so this always runs. null (not
  // undefined) specifically means "MovieBox search just failed" — see
  // loadStreamOptions, which uses that to skip a second, identical,
  // guaranteed-to-fail MovieBox attempt instead of quietly re-running it.
  const subjectId = await prepareMovieboxSubject(movie.title, movie.year).catch(() => null);
  loadStreamOptions(movie.title, 0, 0, subjectId, movie.year);
}
```
with:
```js
async function selectMovieToWatch(movie, resumeSeconds) {
  enterWatching(
    { kind: 'movie', title: movie.title, year: movie.year, image_url: movie.image_url, resumeSeconds },
    { backTarget: 'movie-details', crumbTitle: movie.title, metaHtml: `<h1>${movie.title}</h1>` }
  );
  // Dub/language picking is MovieBox-specific machinery, but MovieBox is
  // always one of the merged sources now, so this always runs. null (not
  // undefined) specifically means "MovieBox search just failed" — see
  // loadStreamOptions, which uses that to skip a second, identical,
  // guaranteed-to-fail MovieBox attempt instead of quietly re-running it.
  const subjectId = await prepareMovieboxSubject(movie.title, movie.year).catch(() => null);
  loadStreamOptions(movie.title, 0, 0, subjectId, movie.year);
}
```
Replace `selectSeriesEpisodeToWatch` (main.js:1588-1607, including its leading `backOverride` doc comment) — keep the leading comment and add the `null` vs `undefined` comment matching `selectMovieToWatch`'s (currently reading "see selectMovieToWatch's identical line"):
```js
// backOverride: { target, label } — resumeContinueWatching passes this to
// keep Back pointed at Home across every subsequent Next/Prev Episode click
// too, not just the episode it resumed. Without threading it through goTo,
// each Next Episode click rebuilds this page via a plain selectSeriesEpisodeToWatch
// call with no override, which used to silently reset Back to Series Details.
async function selectSeriesEpisodeToWatch(series, season, episode, episodeNumbers, backOverride) {
  const goTo = (num) => selectSeriesEpisodeToWatch(series, season, num, episodeNumbers, backOverride);
  const nav = episodeNumbers ? buildEpisodeNav(episodeNumbers, episode, goTo) : sequentialNav(episode, goTo);
  enterWatching(
    { kind: 'series', title: series.title, season, episode, year: series.year, tmdb_id: series.tmdb_id, image_url: series.image_url },
    {
      backTarget: backOverride?.target || 'series-details',
      crumbTitle: backOverride?.label || series.title,
      metaHtml: `<span class="tag">Season ${season} · Episode ${episode}</span><h1>${series.title}</h1>`,
      nav,
    }
  );
  // null vs undefined: see selectMovieToWatch's identical line.
  const subjectId = await prepareMovieboxSubject(series.title, series.year).catch(() => null);
  loadStreamOptions(series.title, season, episode, subjectId, series.year);
}
```
with (note `goTo`, used by Next/Prev Episode nav, deliberately does NOT pass `resumeSeconds` — a resumed position is only for the exact episode the card pointed at):
```js
// backOverride: { target, label } — resumeContinueWatching passes this to
// keep Back pointed at Home across every subsequent Next/Prev Episode click
// too, not just the episode it resumed. Without threading it through goTo,
// each Next Episode click rebuilds this page via a plain selectSeriesEpisodeToWatch
// call with no override, which used to silently reset Back to Series Details.
async function selectSeriesEpisodeToWatch(series, season, episode, episodeNumbers, backOverride, resumeSeconds) {
  const goTo = (num) => selectSeriesEpisodeToWatch(series, season, num, episodeNumbers, backOverride);
  const nav = episodeNumbers ? buildEpisodeNav(episodeNumbers, episode, goTo) : sequentialNav(episode, goTo);
  enterWatching(
    { kind: 'series', title: series.title, season, episode, year: series.year, tmdb_id: series.tmdb_id, image_url: series.image_url, resumeSeconds },
    {
      backTarget: backOverride?.target || 'series-details',
      crumbTitle: backOverride?.label || series.title,
      metaHtml: `<span class="tag">Season ${season} · Episode ${episode}</span><h1>${series.title}</h1>`,
      nav,
    }
  );
  // null vs undefined: see selectMovieToWatch's identical line.
  const subjectId = await prepareMovieboxSubject(series.title, series.year).catch(() => null);
  loadStreamOptions(series.title, season, episode, subjectId, series.year);
}
```
Replace `resumeContinueWatching`'s movie/series branches (main.js:62-71):
```js
  if (kind === 'movie') {
    selectMovieToWatch({ title: entry.title, year: entry.year, image_url: entry.image_url });
  } else if (kind === 'series') {
    selectSeriesEpisodeToWatch(
      { title: entry.title, year: entry.year, tmdb_id: entry.tmdb_id, image_url: entry.image_url },
      entry.season,
      entry.episode,
      undefined,
      backOverride
    );
```
with:
```js
  if (kind === 'movie') {
    selectMovieToWatch({ title: entry.title, year: entry.year, image_url: entry.image_url }, entry.positionSeconds);
  } else if (kind === 'series') {
    selectSeriesEpisodeToWatch(
      { title: entry.title, year: entry.year, tmdb_id: entry.tmdb_id, image_url: entry.image_url },
      entry.season,
      entry.episode,
      undefined,
      backOverride,
      entry.positionSeconds
    );
```
(The anime `else` branch below is unchanged — anime is out of scope.)

- [ ] **Step 6: Subscribe to the `playback-progress` event**

In `main.js`, add a new function right after `initDownloadEvents`:
```js
// mpv reports position/duration once (see mpv_progress.rs's track()) when
// it closes — keyed by watch_id, the same id saveContinueWatching already
// used for this entry, not by "whatever pendingPlay currently is" (mpv can
// still be open, and later closed, well after the user has navigated
// elsewhere in the app).
function initPlaybackProgressEvents() {
  onEvent('playback-progress', (payload) => {
    if (payload.position_seconds == null || payload.duration_seconds == null) return;
    updateContinueWatchingProgress(payload.watch_id, payload.position_seconds, payload.duration_seconds);
    renderContinueWatching();
  });
}
```
Then find the existing `initDownloadEvents();` call in the `DOMContentLoaded` init sequence and add `initPlaybackProgressEvents();` right after it.

- [ ] **Step 7: Verify no syntax errors**

Run: `node --check src/js/main.js && node --check src/js/api.js && node --check src/js/continueWatching.js`
Expected: no output, exit code 0 for all three.

- [ ] **Step 8: Commit**

```bash
git add src/js/api.js src/js/continueWatching.js src/js/main.js
git commit -m "Thread resume position and playback-progress event into the frontend

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 5: Render the progress bar on Continue Watching cards

**Files:**
- Modify: `src/js/main.js:171-222` (`renderContinueWatching`)
- Modify: `src/styles.css` (near `.poster-reveal`, ~line 352)

**Interfaces:**
- Consumes: `e.positionSeconds`/`e.durationSeconds` on continue-watching entries (Task 4)

- [ ] **Step 1: Add the bar markup**

In `main.js`'s `renderContinueWatching`, replace the `.map(...)` block:
```js
  list.innerHTML = entries
    .map(
      (e, i) => `
      <div class="poster-card" data-index="${i}" tabindex="0" role="button" aria-label="Resume ${escapeAttr(e.title)}">
        <div class="poster-image-wrap">
          ${e.image_url ? `<img src="${e.image_url}" alt="${escapeAttr(e.title)}" loading="lazy" />` : ''}
          <button class="continue-remove" data-remove="${i}" aria-label="Remove" type="button">&times;</button>
          <div class="poster-reveal">
            <div class="poster-title">${e.title}</div>
            <div class="poster-sub">${continueWatchingSubtitle(e)}</div>
          </div>
        </div>
      </div>
    `
    )
    .join('') +
```
with:
```js
  list.innerHTML = entries
    .map((e, i) => {
      // Only rendered when both numbers are real — a missing duration (mpv
      // couldn't be IPC-queried, or the value hasn't been reported at all
      // yet) means no bar rather than a wrong/nonsensical one.
      const hasProgress = e.durationSeconds > 0 && e.positionSeconds != null;
      const progressBar = hasProgress
        ? `<div class="continue-progress"><div class="continue-progress-fill" style="width:${Math.min(100, (e.positionSeconds / e.durationSeconds) * 100).toFixed(1)}%"></div></div>`
        : '';
      return `
      <div class="poster-card" data-index="${i}" tabindex="0" role="button" aria-label="Resume ${escapeAttr(e.title)}">
        <div class="poster-image-wrap">
          ${e.image_url ? `<img src="${e.image_url}" alt="${escapeAttr(e.title)}" loading="lazy" />` : ''}
          <button class="continue-remove" data-remove="${i}" aria-label="Remove" type="button">&times;</button>
          ${progressBar}
          <div class="poster-reveal">
            <div class="poster-title">${e.title}</div>
            <div class="poster-sub">${continueWatchingSubtitle(e)}</div>
          </div>
        </div>
      </div>
    `;
    })
    .join('') +
```

- [ ] **Step 2: Add the CSS**

In `src/styles.css`, right after the `.poster-card .poster-reveal { ... }` rule (~line 352), add:
```css
.poster-card .continue-progress {
  position: absolute; left: 0; right: 0; bottom: 0; z-index: 3;
  height: 4px; background: rgba(0, 0, 0, 0.45);
}
.poster-card .continue-progress-fill {
  height: 100%; background: var(--color-accent);
}
```
(`z-index: 3` — one above `.poster-reveal`'s `z-index: 2` — keeps the bar visible even when the hover reveal panel is showing, not just on the plain poster.)

- [ ] **Step 3: Verify no syntax errors**

Run: `node --check src/js/main.js`
Expected: no output, exit code 0.

- [ ] **Step 4: Commit**

```bash
git add src/js/main.js src/styles.css
git commit -m "Render a progress bar on Continue Watching cards

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 6: Live end-to-end verification

No test framework covers mpv itself launching for real — this is manual, using the same live-verification approach used throughout this project's development (temporary diagnostic `println!`/test removed afterward if any are added, real playback runs otherwise).

**Files:** none (verification only)

- [ ] **Step 1: Full build**

Run: `cd src-tauri && cargo check --message-format=short`
Expected: `Finished`, no errors, no leftover unused-code warnings from Tasks 1-3.

- [ ] **Step 2: Play a movie/series episode to completion of the check**

Run the app (`cargo tauri dev` or equivalent), play any movie or series episode via 4KHDHub (MovieBox is currently broken upstream — see this session's earlier debugging — so use 4KHDHub for this check), let it play for at least 15-20 seconds so mpv has real playback position, then quit mpv cleanly (press `q` in the mpv window, or close it).

Before checking the card, first check the watch-later file itself — this is the one part of Task 1 built against an assumed format rather than a confirmed one (see the spec's Open Questions). Look in `%TEMP%\trela-watch-later\<hex-id>\` (it's deleted right after being read, so either check quickly after quitting mpv, or temporarily comment out the `std::fs::remove_dir_all` line in `mpv_progress::track` for this one check and put it back after). Confirm it contains a `start=<number>` line. If the real format differs (different key name, different location in the file), fix `parse_watch_later_position` in `mpv_progress.rs` to match, re-run that function's two unit tests, and re-verify.

Expected: within a second or two of mpv closing, the Home page's Continue Watching card for that title shows a progress bar roughly matching how far you watched.

- [ ] **Step 3: Verify resume actually seeks**

Click that same Continue Watching card, then Play.

Expected: mpv opens already past the beginning — not at 0:00. (If uncertain visually, `--start=<seconds>` was passed; this can be confirmed by checking mpv's own on-screen time display right after it opens.)

- [ ] **Step 4: Verify graceful degradation on an abrupt kill**

Start playing a different title, then kill mpv via Task Manager (not a clean quit) after a few seconds.

Expected: no error shown anywhere in the app, no stuck "Loading..." state, and that title's Continue Watching card either shows no bar (first time watching it) or keeps whatever bar it had from a previous *clean* session (not wiped, not corrupted).

- [ ] **Step 5: Verify a fresh Details-page launch does not resume**

Pick an episode that has a saved position via its card, but launch it instead from the Series Details page's episode grid (not the Continue Watching card).

Expected: starts from 0:00.
