# Continue Watching Progress Bar — Design Spec

Status: approved by user, pending spec review pass
Date: 2026-09-05

## Problem

Continue Watching cards (`renderContinueWatching` in `src/js/main.js:171`) show a
poster, title, and season/episode subtitle — nothing about how far into the
episode/movie you actually got. Two things are missing:

1. A visual sense of progress (a bar, like every other streaming app's
   continue-watching row).
2. Resuming from a card (`resumeContinueWatching`, `main.js:50`) always
   restarts mpv from the beginning — it re-navigates to the same
   title/episode's Watching page, but nothing about *where in the video* you
   stopped is ever captured or replayed.

Neither is fixable by editing what's there — this app has never tracked
playback position or duration anywhere. `spawn_mpv` (`src-tauri/src/
movplayer.rs:369`) spawns mpv as a fire-and-forget sidecar: it doesn't even
keep the child handle (`let (mut rx, _child) = sidecar.spawn()...`), and the
event-draining loop that follows discards every event without inspecting it.
This is new plumbing, not a tweak.

Scope: movie/series only (mpv, which Trela launches and controls directly).
Anime is explicitly out — `watch_episode` (`player.rs`) hands off to ani-cli,
which picks its own player (observed launching a VLC dependency check on this
machine), so Trela has no reliable player process to talk to for anime at all.

"Already seen items" in this feature means the Continue Watching cards
themselves (partially-watched titles) — not the separate `watched.js` list
(fully-watched, manually marked), which has a much thinner data shape (no
image, no season/episode) and was never part of this request.

## Approach

Two approaches were considered for getting position + duration out of mpv:

1. **Continuous JSON-IPC polling** — hold an IPC connection open for the
   whole viewing session, poll `time-pos` every N seconds. Rejected as the
   sole mechanism: more moving parts (a live poll loop, reconnect-on-drop
   handling) for no benefit over the alternative below, since nothing in this
   app needs a *live* position while mpv is still playing — only the final
   position, once, when playback stops.
2. **Chosen: hybrid of mpv's native watch-later mechanism (position) + a
   one-shot IPC query (duration).**
   - Position: mpv already has a built-in "remember where I was" feature —
     `--save-position-on-quit` writes position to a small text file in
     `--watch-later-dir` when mpv quits (cleanly). No IPC needed for this
     half at all.
   - Duration: not something `--save-position-on-quit` records (it's a
     source-file property, not a restorable playback setting), and not
     available any other way without asking mpv directly — so one IPC
     round-trip, made once, shortly after mpv starts, then the connection is
     dropped. Not a running poll loop.

This avoids the complexity of approach 1 (no held-open connection, no
reconnect logic, no periodic timer) while still getting both numbers.

**mpv's own URL-based auto-resume will not fire on its own** and is not
relied on: it matches watch-later entries by the played URL, but 4KHDHub/
MovieBox stream URLs are freshly resolved (and expire) every play, so the
same title never presents the same URL twice. Trela reads the watch-later
file itself and passes `--start=<seconds>` explicitly on the next launch,
keyed by Trela's own title/season/episode id — not by mpv's URL match.

## Design

### 1. Fresh per-launch watch-later directory (no correlation needed)

`spawn_mpv` creates a **new, empty temp directory** for every launch (e.g.
`%TEMP%\trela-watch-later\<uuid>\`) and passes it as `--watch-later-dir`.
Because the directory is exclusive to this one mpv process, whatever single
file mpv writes into it on exit is unambiguously "this session's position" —
no hashing or matching against the played URL is needed. The directory is
deleted after being read.

### 2. mpv launch flags (movplayer.rs `spawn_mpv`, fourkhdhub.rs
`play_fourk_stream`'s own inline sidecar setup — both spawn mpv today and
both need the same flags)

Added, for every movie/series launch (not anime):
```
--save-position-on-quit
--watch-later-dir=<fresh temp dir>
--input-ipc-server=<platform pipe name, e.g. \\.\pipe\trela-mpv-<uuid>>
```
Existing `--start=<seconds>` is added only when resuming (see §5).

### 3. One-shot duration query

Immediately after `sidecar.spawn()`, a separate tokio task:
- Retries connecting to the named pipe (`tokio::net::windows::named_pipe::
  ClientOptions::new().open(...)`, gated by tokio's `net` feature — not yet
  enabled in `Cargo.toml`, needs adding to the existing `tokio` dependency's
  `features` list) every ~100ms for up to ~3s total, per tokio's own
  documented `ERROR_PIPE_BUSY`/`NotFound` retry pattern.
- Once connected, writes `{"command":["get_property","duration"]}\n` and
  reads one JSON response line. If mpv hasn't finished opening the file yet
  (`duration` not available), retries the query itself a few times within
  the same ~3s budget.
- Stores the result (or nothing, on timeout/any error) for the exit handler
  below to pick up. No polling after this; the connection is not kept open.

### 4. Reading the result on exit

`spawn_mpv`'s existing event-drain loop (`while rx.recv().await.is_some()
{}`) currently ignores every event. It now matches on `CommandEvent::
Terminated` (confirmed present in `tauri-plugin-shell` 2.3.5's
`CommandEvent` enum) to know exactly when mpv closes. At that point:
- Read the one file in the watch-later temp dir (simple `key=value` text
  format; the line of interest is `start=<seconds>`).
- Combine with the duration captured in §3 (if any).
- Delete the temp dir.
- `app.emit("playback-progress", PlaybackProgress { position, duration })`
  — a new event, added to `models.rs` next to the existing
  `DownloadProgressEvent`/`DownloadCompleteEvent`, following the same
  no-id-in-payload pattern (there's only ever one mpv instance playing, so
  the frontend already knows which title this belongs to from `pendingPlay`
  — same reasoning `DownloadCompleteEvent` already relies on).
- Emitted even when both position and duration are unavailable (as `null`)
  — see Error handling.

### 5. Frontend: storing progress and rendering the bar

- `continueWatching.js` entries gain two new optional fields:
  `positionSeconds`, `durationSeconds`.
- `main.js` subscribes to the new `playback-progress` event (via the
  existing `onEvent` helper in `api.js`, same pattern as download events)
  while `pendingPlay` is set for a movie/series, and merges
  `{positionSeconds, durationSeconds}` into that title's continue-watching
  entry via the existing `listStore` `upsert`.
- `renderContinueWatching` (`main.js:171`) adds a thin bar at the bottom of
  `.poster-image-wrap`, width = `positionSeconds/durationSeconds * 100%`,
  **only when both fields are present and durationSeconds > 0** — otherwise
  no bar at all (see Error handling; matches the existing graceful-
  degradation pattern for entries with no `image_url`).

### 6. Resume-seek

- `resumeContinueWatching` (`main.js:50`) reads `positionSeconds` off the
  entry and threads it into `pendingPlay` (a new `resumeSeconds` field)
  instead of discarding it.
- `startPlayback` passes `resumeSeconds` through to `api.playStream`/
  `api.playFourkStream` (both gain a new optional `startSeconds` param).
- `play_stream` (`movplayer.rs:457`) and `play_fourk_stream`
  (`fourkhdhub.rs:246`) — both `#[tauri::command]`s that already call
  `spawn_mpv`/build the mpv sidecar directly — pass `--start=<seconds>` to
  mpv when `start_seconds` is `Some`.
- Scoped to the Continue Watching card's "Resume" click specifically —
  picking the same episode fresh from Browse/Details does not auto-seek,
  even if a saved position exists. Avoids surprising behavior when someone
  deliberately wants to rewatch from the start.

## Error handling

Every failure mode in this feature is **best-effort, non-blocking**, per
explicit user direction: *"if you can't get the data, just don't display the
progress bar."* Concretely:
- IPC pipe never becomes connectable (mpv crashes instantly, or the
  `net` feature/pipe name is somehow wrong): duration stays unknown for that
  session; no error surfaced to the user, playback is entirely unaffected
  (the duration query runs in its own task, never blocking mpv or the
  existing event loop).
- `duration` query connects but never resolves within the retry budget:
  same as above.
- Watch-later file missing on exit (abrupt kill via Task Manager, power
  loss, mpv killed before writing) — position not updated this session; any
  **previously saved** position/duration on that entry is left untouched
  rather than being cleared, so an abrupt-kill session doesn't erase a
  perfectly good progress bar from an earlier, clean session.
- Either value present but the other missing, or `durationSeconds` is 0 —
  no bar rendered (avoids a divide-by-zero / nonsensical-percentage bar).
- None of this affects the existing "Ready to play"/error status text flow
  in `startPlayback` — progress tracking is entirely additive and silent on
  failure.

## Testing / verification plan

No test framework exists in this repo (consistent with the rest of the
codebase — verification here has been live, manual runs against real data
throughout this session, not an automated suite). Verification plan:
1. Play a movie/series episode long enough for mpv to report a real
   `duration` (check via a temporary diagnostic print during
   implementation, same throwaway-test-then-remove approach used earlier
   this session for `fourkhdhub.rs`/`movplayer.rs` live checks).
2. Quit mpv cleanly (press `q` or close the window) — confirm the
   watch-later file appears, confirm `playback-progress` fires with sane
   numbers, confirm the Continue Watching card shows a bar at roughly the
   right position.
3. Kill mpv via Task Manager mid-playback — confirm no bar update happens
   and nothing else breaks (no stuck "Loading..." state, no console error
   surfaced to the user).
4. Click "Resume" on a card with a saved position — confirm mpv actually
   opens with `--start=<seconds>` and visibly starts mid-episode.
5. Pick the same episode fresh from Details (not via the card) — confirm it
   starts from 0 despite a saved position existing.

## Open questions / risks

- **mpv watch-later file format assumption**: the `start=<seconds>` line
  format is standard/stable across mpv versions, but this hasn't been
  verified against the exact bundled mpv build's output yet — first real
  implementation step should be a throwaway run confirming the file's exact
  contents before wiring the parser to it.
- **IPC pipe-name collisions**: using a `<uuid>`-suffixed pipe name per
  launch avoids collisions if a previous mpv process is still shutting down;
  worth confirming Windows named pipe names have no length/character
  constraints that a UUID could violate (unlikely, but unverified).
- **Cleanup of the temp watch-later directories**: deleted after a normal
  read, but a crash between mpv exiting and Trela reading the file would
  leak one leftover directory under `%TEMP%\trela-watch-later\`. Not worth
  building a sweep/GC for a personal app's temp folder; noted as a known,
  accepted gap.
