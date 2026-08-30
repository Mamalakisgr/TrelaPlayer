# ani-cli Setup Wizard — Design Spec

Status: approved by user, pending spec review pass
Date: 2026-08-30

## Problem

Trela is close to shippable as a real Windows executable (`tauri build` already
produces an NSIS installer/MSI for Trela itself — `bundle.active`/`targets` are
already set in `tauri.conf.json`). But on a fresh machine, Movies/Series works
immediately (native Rust talking to TMDB + MovieBox/4KHDHub, mpv already
bundled as a Tauri sidecar) while anime via ani-cli does not — it needs a real
dependency chain present on the target machine first.

## Dependency chain (as found on the dev machine, via Scoop)

- `ani-cli` (the actual upstream `pystardust/ani-cli` POSIX shell script) —
  installed via Scoop's `extras` bucket.
- Run through **Git for Windows' `bash.exe`** — the scoop-generated shim
  hardcodes a call to Git Bash to execute the shebang-less script. Scoop
  itself cannot install without Git already present (chicken-and-egg: Scoop's
  own bootstrap requires it).
- `fzf` and a standalone **`mpv`** — both declared as `depends` in ani-cli's
  own scoop manifest (`extras/ani-cli` → `"depends": ["fzf", "extras/mpv"]`),
  so `scoop install ani-cli` alone pulls both in automatically. This mpv
  instance is separate from Trela's own bundled sidecar mpv, which is wired
  up only for the movie/series playback path.
- `curl-impersonate.exe` — used directly by `anidb.rs` (TLS-fingerprint
  bypass for anidb.app's Cloudflare challenge) and internally by ani-cli
  itself for the same reason. On the dev machine this came from an msys2
  install, but msys2 itself is a ~500MB+ environment not worth requiring just
  for one binary — the upstream `curl-impersonate` project publishes
  standalone Windows binaries directly, so this is handled separately (see
  below), not through Scoop at all.

## Approach

Three approaches were considered:

1. **Fully self-contained installer** — bundle a portable POSIX shell runtime,
   fzf, curl-impersonate, and try to redirect ani-cli's own player choice at
   Trela's bundled mpv sidecar. Rejected: ani-cli's player selection is a
   small set of named choices resolved via PATH (or an `ANI_CLI_PLAYER`/
   `ANI_CLI_PLAYER_FLAGS` env var override — real, but still resolves a
   *named* player, not an arbitrary path guaranteed to work the same way),
   and bundling a bash runtime is real integration risk for a personal app.
2. **Hybrid, chained-installer** — bundle curl-impersonate + fzf as sidecars,
   silently chain the official Git-for-Windows installer if missing. More
   contained than (1) but still takes on installer-chaining risk for a
   dependency Scoop already manages well.
3. **Chosen: reuse Scoop.** Trela drives Scoop itself (already the proven,
   working path on the dev machine) through a guided, checklist-style setup
   flow: check each prerequisite, skip what's already present, only ever
   pause for genuine user input/consent (not routine automation). Lowest risk
   — it reuses exact package versions and a dependency graph (ani-cli →
   fzf + mpv) that Scoop's own manifest already resolves correctly, instead
   of Trela re-implementing that resolution itself.

curl-impersonate is handled independently of all three options above: it's
small and self-contained enough to just bundle as a Tauri sidecar, the same
way mpv already is. No Scoop involvement, no setup step, no UI for it at all.

## Design

### Flow

Runs as one guided sequence, checking before acting at every step so nothing
gets redundantly reinstalled:

1. **Check ani-cli already resolves** (same PATH lookup `player.rs` already
   does via `find_exe`). If found → done, nothing else runs.
2. **Check Scoop is installed** (`scoop.cmd` on PATH). If missing:
   - Check **Git** is on PATH (Scoop's own bootstrap requirement).
     - Present → proceed to bootstrap Scoop automatically (see Consent below
       — this step itself needs a consent pause).
     - Missing → **pause and ask**: offer a button to install Git for
       Windows via `winget install --id Git.Git -e` (winget ships with
       Windows 10 1709+/11; the silent install still surfaces Windows' own
       UAC prompt — that's the real consent moment, not something to hide
       or auto-accept).
3. **Check the `extras` bucket is added** (`scoop bucket list`). Add it if
   not (`scoop bucket add extras`).
4. **Check ani-cli is already in `scoop list`.** If not, `scoop install
   ani-cli` — this alone pulls in fzf + mpv per its own manifest's `depends`.
5. **Re-verify** ani-cli now resolves via the same PATH lookup as step 1;
   report success or a clear failure reason.

### Consent points (the two moments this is NOT silent)

- **Git missing entirely** — offer the winget install button; do not attempt
  any other silent bootstrap path. If winget itself isn't available (older
  Windows), show a direct link to Git for Windows' download page instead of
  attempting anything further automatically.
- **About to bootstrap Scoop** — Scoop's installer runs `Set-ExecutionPolicy
  RemoteSigned -Scope CurrentUser`, a real PowerShell security-policy change
  for the user's account. Show what's about to happen and require an explicit
  "Continue" click before running it, even though it is fully scriptable.

Every other step (bucket add, `scoop install ani-cli`) is routine package
management with no security/system-policy side effect, so those run without
an extra confirmation once the flow has started.

### UI

New button in the existing "ani-cli" Settings section (next to "Check for
ani-cli Update", which already has this exact live-`<pre>`-status pattern),
labeled "Set Up ani-cli". Clicking it expands a small checklist below the
button, one row per step:

```
Scoop package manager       ✓ already installed
'extras' bucket             checking...
ani-cli (+ fzf, mpv)        pending
```

Each row settles to one of: already-had-it, done, or failed (with the
specific reason surfaced, not just "failed"). The two consent points render
as an inline prompt in place of a checklist row — explanatory text plus one
button — never a modal (nothing else in this app uses modals besides the
command palette, which is a different kind of UI entirely).

### Backend

Package installs are slow (real downloads), so this can't be one blocking
request/response — same shape as the existing download-progress system
(`DownloadProgressEvent` in `movplayer.rs`, consumed via `onEvent` in the
frontend). New `src-tauri/src/setup.rs`:

- One `#[tauri::command]` that runs the checklist server-side, spawning
  `powershell.exe`/`scoop.cmd`/`winget` as child processes — the same
  `Command`-spawning pattern already used for `ani-cli`/`curl-impersonate` in
  `player.rs`/`anidb.rs`, nothing new there.
- Emits a `setup-progress` event per step:
  `{ step: "scoop" | "bucket" | "ani-cli", status: "checking" | "already-present" | "done" | "error", detail: String }`.
- A second command the frontend calls to signal consent at the two pause
  points (`confirm_setup_step(step)`), since the backend can't itself decide
  when the user has clicked "Continue" — the running setup command awaits a
  channel/oneshot that this command resolves.

### curl-impersonate sidecar

Separate, small addition, no relation to the wizard above:

- Add the upstream `curl-impersonate` Windows binary to `src-tauri/binaries/`
  (same convention as `mpv-x86_64-pc-windows-msvc.exe`) and register it in
  `tauri.conf.json`'s `externalBin`, alongside the existing `mpv` entry.
- `anidb.rs`'s `curl_impersonate_path()` (currently a `find_exe` PATH lookup)
  changes to resolve the bundled sidecar the same way `movplayer.rs`'s
  `spawn_mpv` already resolves the mpv sidecar (`app.shell().sidecar(...)`).
- No UI, no setup step — works out of the box on a fresh install.

## Error handling

- Each step's failure is specific and shown inline on that step's row (e.g.
  "Scoop install failed: <stderr>"), not a single generic "setup failed"
  message — the whole point of a checklist over the old
  "Check for ani-cli Update" single-line status is per-step visibility.
- A failed step stops the flow (later steps depend on earlier ones
  succeeding); the checklist stays visible so the user can see exactly where
  it stopped and retry from there rather than restarting the whole thing.
- Network/timeout failures during a `scoop install` surface Scoop's own error
  text rather than being swallowed — matches the existing pattern in
  `player.rs`/`anidb.rs` of surfacing the real underlying error instead of a
  generic one.

## Testing / verification plan

No existing test framework in this codebase (confirmed earlier this session)
— consistent with that, verification here is manual:

- Fresh Windows VM/user account with none of the dependencies present:
  confirm the full flow (including both consent pauses) completes and
  `ani-cli` resolves afterward.
- Machine with Scoop + Git already present but ani-cli not installed:
  confirm steps 1–2 report "already installed"/skip correctly and only
  steps 3–4 actually run.
- Machine with everything already present (like the dev machine): confirm
  step 1 alone reports success and nothing else runs.
- Git absent entirely: confirm the winget consent prompt appears and, on an
  older Windows without winget, the fallback download-page link appears
  instead of attempting anything further.
- curl-impersonate sidecar: confirm `anidb.rs` resolves and successfully
  calls it with zero prior Scoop/msys2 setup on a clean machine.

## Open questions / risks

- winget's exact silent-install flag behavior (UAC prompt shape, exit codes
  on cancel) should be re-verified against whatever Windows version this
  actually ships to — not verified live in this session.
- Scoop's own installer script could change its execution-policy requirement
  or bootstrap flow upstream; the consent-point copy should stay accurate to
  what Scoop's installer *currently* does, and may need revisiting later.
- This spec doesn't cover *uninstalling*/reversing what the wizard installs
  (Scoop, Git, ani-cli+deps) — out of scope for a first version; Scoop and
  Git both have their own normal uninstall paths already.
