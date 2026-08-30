# ani-cli Setup Wizard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a fresh Windows install of Trela work for anime (via ani-cli) without the user manually hunting down and installing each dependency by hand, and remove the one remaining PATH-based dependency (curl-impersonate) entirely by bundling it.

**Architecture:** Two independent pieces. (1) `curl-impersonate` becomes a bundled Tauri sidecar (like `mpv` already is) — no setup step, no UI, just works. (2) A new Settings checklist drives Scoop (`scoop install ani-cli`, which already declares `fzf` + `mpv` as its own dependencies) through a guided flow that checks before acting at every step and only pauses for two real consent moments (installing Git for Windows, and Scoop's own execution-policy change) — everything else runs without extra prompts.

**Tech Stack:** Rust (Tauri v2 commands + events), vanilla JS (existing `main.js`/`api.js` patterns, no new frontend dependencies), PowerShell/winget/Scoop as external tools spawned via `std::process::Command`.

**Spec:** `docs/superpowers/specs/2026-08-30-anicli-setup-wizard-design.md`

## Global Constraints

- No existing test framework in this codebase — pure-logic functions get real `#[cfg(test)]` unit tests (Rust's built-in test runner, `cargo test`), anything that shells out to a real system tool (PowerShell/Scoop/winget) is verified manually per the spec's verification plan, not mocked.
- Never silently change system state without a visible consent step first — this applies specifically to installing Git for Windows and to Scoop's execution-policy change (`Set-ExecutionPolicy`); routine package installs (`scoop bucket add`, `scoop install ani-cli`) do not need their own extra prompt, since Scoop itself is already idempotent about re-running them.
- Match existing code conventions exactly: `Result<T, String>` for all command return types, `#[tauri::command]` functions grouped by responsibility into their own file (matching `tmdb.rs`/`anilist.rs`/`player.rs`), comments explain *why* not *what*, no unrequested abstractions.

---

### Task 1: Bundle curl-impersonate as a Tauri sidecar

**Files:**
- Create: `src-tauri/binaries/curl-impersonate-x86_64-pc-windows-msvc.exe`
- Modify: `src-tauri/tauri.conf.json`
- Modify: `src-tauri/src/anidb.rs`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `sidecar_path(name: &str) -> Result<std::path::PathBuf, String>` in `anidb.rs` — a small helper Task 3 does *not* need (setup.rs's own process spawns use plain PATH-resolved tools, not sidecars), listed here only because it's a new, independently reusable function other future sidecar work could call.

- [ ] **Step 1: Download and place the real curl-impersonate binary**

This is a verified, real release — not a placeholder. Run from `src-tauri/`:

```bash
curl -sL "https://github.com/lexiforest/curl-impersonate/releases/download/v2.2.0/curl-impersonate-v2.2.0.x86_64-win32.tar.gz" -o /tmp/ci.tar.gz
tar -xzf /tmp/ci.tar.gz -C /tmp ./curl-impersonate.exe
mkdir -p binaries
mv /tmp/curl-impersonate.exe binaries/curl-impersonate-x86_64-pc-windows-msvc.exe
rm /tmp/ci.tar.gz
```

Verify: `file binaries/curl-impersonate-x86_64-pc-windows-msvc.exe` should report `PE32+ executable (console) x86-64, for MS Windows` (confirmed during planning: this exact URL yields a 4,077,568-byte legitimate Windows binary, not one of the archive's `.bat` browser-wrapper scripts — those call this same exe with preset flags, which is *not* what's wanted here since `anidb.rs` already passes its own explicit flag set matching `curl_chrome116`).

- [ ] **Step 2: Register the sidecar in `tauri.conf.json`**

In `src-tauri/tauri.conf.json`, extend the existing `bundle.externalBin` array (currently `["binaries/mpv"]`):

```json
"externalBin": ["binaries/mpv", "binaries/curl-impersonate"],
```

- [ ] **Step 3: Add a synchronous sidecar-path resolver to `anidb.rs`**

`curl_impersonate_get` runs on a `spawn_blocking` native thread pool via `std::thread::scope` (see `resolve_anime_blocking`), with no `AppHandle` anywhere in its call chain — `tauri_plugin_shell`'s `Shell::sidecar()` API is async (`Command::output()` is `pub async fn`), so it can't be dropped in here without restructuring that concurrency model. Tauri's own sidecar resolution (`tauri_plugin_shell::process::relative_command_path`, which is `pub(crate)` and not callable from outside that crate) is simple enough to replicate directly: find the current exe's directory, join the sidecar name, add `.exe` on Windows if missing.

Replace the top of `src-tauri/src/anidb.rs`:

```rust
use crate::models::{Episode, ResolvedAnime};
use std::process::Command;
```

(drop `use crate::procutil::find_exe;` — no longer used in this file; `procutil.rs` itself is untouched, `player.rs` still uses `find_exe` for `ani-cli`)

Replace `curl_impersonate_path`:

```rust
// Mirrors tauri_plugin_shell's own sidecar resolution (relative_command_path)
// rather than using that crate's Shell::sidecar() API directly — that API is
// async (Command::output() is `pub async fn`), but curl_impersonate_get below
// runs synchronously on a spawn_blocking thread pool via std::thread::scope,
// with no AppHandle in its call chain at all. This needs no async runtime:
// a sidecar is just "the bundled binary sitting next to the app's own exe."
fn sidecar_path(name: &str) -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Failed to locate current executable: {}", e))?;
    let dir = exe.parent().ok_or_else(|| "Current executable has no parent directory".to_string())?;
    let mut path = dir.join(name);
    if cfg!(windows) && path.extension().is_none_or(|ext| ext != "exe") {
        path.as_mut_os_string().push(".exe");
    }
    Ok(path)
}

fn curl_impersonate_path() -> Result<String, String> {
    sidecar_path("curl-impersonate").map(|p| p.to_string_lossy().into_owned())
}
```

Update `curl_impersonate_get` to propagate the now-fallible path (only the first two lines change):

```rust
fn curl_impersonate_get(url: &str) -> Result<String, String> {
    let path = curl_impersonate_path()?;
    let output = Command::new(&path)
```

(the rest of the function — the full flag list, error handling, Cloudflare-block detection — is unchanged)

- [ ] **Step 4: Verify it compiles**

Run: `cd src-tauri && cargo check`
Expected: no errors. `find_exe` remains used in `player.rs` (ani-cli itself still resolves via PATH — only curl-impersonate is being bundled here), so `procutil.rs` needs no changes.

- [ ] **Step 5: Manual verification**

Build and run the app (`cargo tauri dev` or `cargo tauri build` + run the exe), open any anime's Details page (this triggers `resolve_anime` → `curl_impersonate_get`), and confirm it still successfully resolves against anidb.app. To specifically confirm it's using the *bundled* binary and not a stray PATH one, temporarily rename any system-installed `curl-impersonate.exe` (e.g. the one under `scoop\apps\msys2\current\mingw64\bin\` on the dev machine) and re-run the same check — it should still work.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/binaries/curl-impersonate-x86_64-pc-windows-msvc.exe src-tauri/tauri.conf.json src-tauri/src/anidb.rs
git commit -m "Bundle curl-impersonate as a Tauri sidecar instead of a PATH dependency"
```

(Note: `trela/` is not currently a git repository — see the note at the end of this plan.)

---

### Task 2: Rust backend for the ani-cli setup checklist

**Files:**
- Create: `src-tauri/src/setup.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `crate::procutil::find_exe` (existing, unchanged).
- Produces (for Task 3 to call via `invoke`):
  - `check_anicli_setup() -> bool`
  - `run_anicli_setup() -> Result<(), String>` (async; emits `setup-progress` events with payload `{ step: string, status: string, detail: string }` where `step` is one of `"git" | "scoop" | "bucket" | "ani-cli"` and `status` is one of `"checking" | "already-present" | "done" | "error" | "awaiting-consent"`)
  - `confirm_setup_consent(accepted: bool) -> Result<(), String>`

- [ ] **Step 1: Write the pure-logic unit tests first**

Create `src-tauri/src/setup.rs` with just the two path-check functions and their tests (no commands yet):

```rust
use std::path::PathBuf;

// ani-cli's scoop-generated shim hardcodes a call to the *official*
// Git-for-Windows bash.exe (confirmed on the dev machine: the shim reads
// `"C:\Program Files\Git\bin\bash.exe" ani-cli ...`) — a minimal/portable git
// on PATH doesn't provide this, so this checks for the real thing, not just
// "is any git.exe somewhere."
fn bash_exists() -> bool {
    let program_files = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".to_string());
    PathBuf::from(program_files).join("Git").join("bin").join("bash.exe").is_file()
}

fn scoop_shim_path() -> PathBuf {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    PathBuf::from(home).join("scoop").join("shims").join("scoop.cmd")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_exists_reflects_real_program_files_state() {
        // Not mocking the filesystem here — this just proves the function
        // doesn't panic and returns a plain bool either way, on whatever
        // machine actually runs the test.
        let _ = bash_exists();
    }

    #[test]
    fn scoop_shim_path_is_under_user_profile_scoop_shims() {
        std::env::set_var("USERPROFILE", "C:\\Users\\testuser");
        let path = scoop_shim_path();
        assert_eq!(path, PathBuf::from("C:\\Users\\testuser\\scoop\\shims\\scoop.cmd"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `cd src-tauri && cargo test --lib setup::`
Expected: both tests pass (these are real assertions on real logic, not placeholders — `scoop_shim_path` is pure string/path joining, directly testable without mocking anything external).

- [ ] **Step 3: Add the progress event type and consent state**

Append to `src-tauri/src/setup.rs`:

```rust
use serde::Serialize;
use std::sync::Mutex;
use tokio::sync::oneshot;

#[derive(Serialize, Clone)]
pub struct SetupProgress {
    pub step: String,
    pub status: String,
    pub detail: String,
}

// Only one setup run happens at a time (single-window desktop app), so one
// shared slot for "the consent this run is currently waiting on" is enough —
// no per-step keying needed.
#[derive(Default)]
pub struct SetupConsent(pub Mutex<Option<oneshot::Sender<bool>>>);
```

- [ ] **Step 4: Add the process-running helper**

```rust
use std::process::Command;

// -ExecutionPolicy Bypass here only affects *this one* powershell.exe
// process's ability to run its own -Command script — it does not persist
// or change any system/user setting. That's different from (and doesn't
// replace) the user-facing consent point later in run_anicli_setup, which
// gates Scoop's *own* installer running `Set-ExecutionPolicy ... -Scope
// CurrentUser`, a real persistent policy change.
fn run_ps(script: &str) -> Result<(), String> {
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script])
        .output()
        .map_err(|e| format!("Failed to launch PowerShell: {}", e))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(if stderr.trim().is_empty() {
        format!("PowerShell exited with status {}", output.status)
    } else {
        stderr.trim().to_string()
    })
}
```

- [ ] **Step 5: Add the commands**

```rust
use tauri::{AppHandle, Emitter, State};

fn emit_progress(app: &AppHandle, step: &str, status: &str, detail: &str) {
    let _ = app.emit(
        "setup-progress",
        SetupProgress { step: step.to_string(), status: status.to_string(), detail: detail.to_string() },
    );
}

#[tauri::command]
pub fn check_anicli_setup() -> bool {
    crate::procutil::find_exe(&["ani-cli.cmd", "ani-cli.exe", "ani-cli.bat"]).is_some()
}

async fn await_consent(consent: &State<'_, SetupConsent>) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    {
        let mut guard = consent.0.lock().map_err(|_| "Internal error: consent lock poisoned".to_string())?;
        *guard = Some(tx);
    }
    rx.await.map_err(|_| "Setup was cancelled.".to_string())
}

#[tauri::command]
pub fn confirm_setup_consent(accepted: bool, consent: State<'_, SetupConsent>) -> Result<(), String> {
    let mut guard = consent.0.lock().map_err(|_| "Internal error: consent lock poisoned".to_string())?;
    match guard.take() {
        Some(tx) => {
            let _ = tx.send(accepted);
            Ok(())
        }
        None => Err("No setup step is currently waiting for confirmation.".to_string()),
    }
}

#[tauri::command]
pub async fn run_anicli_setup(app: AppHandle, consent: State<'_, SetupConsent>) -> Result<(), String> {
    if check_anicli_setup() {
        emit_progress(&app, "ani-cli", "already-present", "ani-cli already resolves on PATH");
        return Ok(());
    }

    if scoop_shim_path().exists() {
        emit_progress(&app, "scoop", "already-present", "Scoop already installed");
    } else {
        if !bash_exists() {
            emit_progress(&app, "git", "awaiting-consent", "Git for Windows is required before Scoop can be installed.");
            if !await_consent(&consent).await? {
                return Err("Setup cancelled — Git for Windows is required.".to_string());
            }
            emit_progress(&app, "git", "checking", "Installing Git for Windows via winget...");
            tokio::task::spawn_blocking(|| {
                run_ps("winget install --id Git.Git -e --silent --accept-package-agreements --accept-source-agreements")
            })
            .await
            .map_err(|e| format!("Internal error installing Git: {}", e))??;
            if !bash_exists() {
                emit_progress(&app, "git", "error", "Git for Windows install finished but bash.exe still wasn't found.");
                return Err("Git for Windows install did not complete successfully.".to_string());
            }
            emit_progress(&app, "git", "done", "Git for Windows installed");
        }

        emit_progress(
            &app,
            "scoop",
            "awaiting-consent",
            "About to install Scoop, which sets a PowerShell script-execution policy for your user account.",
        );
        if !await_consent(&consent).await? {
            return Err("Setup cancelled before installing Scoop.".to_string());
        }
        emit_progress(&app, "scoop", "checking", "Installing Scoop...");
        tokio::task::spawn_blocking(|| run_ps("Set-ExecutionPolicy RemoteSigned -Scope CurrentUser -Force; irm get.scoop.sh | iex"))
            .await
            .map_err(|e| format!("Internal error installing Scoop: {}", e))??;
        if !scoop_shim_path().exists() {
            emit_progress(&app, "scoop", "error", "Scoop installer finished but scoop.cmd still wasn't found.");
            return Err("Scoop install did not complete successfully.".to_string());
        }
        emit_progress(&app, "scoop", "done", "Scoop installed");
    }

    emit_progress(&app, "bucket", "checking", "Adding the 'extras' bucket...");
    tokio::task::spawn_blocking(|| run_ps("scoop bucket add extras"))
        .await
        .map_err(|e| format!("Internal error adding the extras bucket: {}", e))??;
    emit_progress(&app, "bucket", "done", "'extras' bucket ready");

    // scoop install ani-cli pulls in fzf + mpv automatically — they're
    // declared as `depends` in ani-cli's own scoop manifest, not something
    // this needs to install separately.
    emit_progress(&app, "ani-cli", "checking", "Installing ani-cli (+ fzf, mpv)...");
    tokio::task::spawn_blocking(|| run_ps("scoop install ani-cli"))
        .await
        .map_err(|e| format!("Internal error installing ani-cli: {}", e))??;

    if check_anicli_setup() {
        emit_progress(&app, "ani-cli", "done", "ani-cli installed and resolves on PATH");
        Ok(())
    } else {
        emit_progress(&app, "ani-cli", "error", "Install finished but ani-cli still doesn't resolve on PATH.");
        Err("ani-cli install did not complete successfully.".to_string())
    }
}
```

- [ ] **Step 6: Register the module, commands, and managed state in `lib.rs`**

Add `mod setup;` to the `mod` list at the top of `src-tauri/src/lib.rs`, add `.manage(setup::SetupConsent::default())` to the builder chain (after `.plugin(tauri_plugin_shell::init())`), and add these three commands to the `tauri::generate_handler![...]` list:

```rust
setup::check_anicli_setup,
setup::run_anicli_setup,
setup::confirm_setup_consent,
```

- [ ] **Step 7: Verify it compiles and tests pass**

Run: `cd src-tauri && cargo check && cargo test --lib setup::`
Expected: clean compile, both unit tests from Step 2 still pass.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/setup.rs src-tauri/src/lib.rs
git commit -m "Add ani-cli setup checklist backend (Scoop-driven, consent-gated)"
```

---

### Task 3: Settings UI for the setup checklist

**Files:**
- Modify: `src/index.html`
- Modify: `src/js/api.js`
- Modify: `src/js/main.js`
- Modify: `src/styles.css`

**Interfaces:**
- Consumes: `check_anicli_setup`, `run_anicli_setup`, `confirm_setup_consent` from Task 2 (exact command names above); the existing `onEvent` helper from `api.js` (already used for download-progress events, same pattern here for `setup-progress`).
- Produces: nothing further consumed by other tasks — this is the top of the stack.

- [ ] **Step 1: Add the button + checklist container to Settings**

In `src/index.html`, inside the existing "ani-cli" `settings-section` (which already has the "Check for ani-cli Update" button), add directly below the existing `<pre id="update-ani-cli-status">`:

```html
<button id="setup-anicli-btn" type="button" class="btn btn-secondary">Set Up ani-cli</button>
<div id="setup-anicli-checklist" class="setup-checklist hidden"></div>
```

- [ ] **Step 2: Add API wrappers**

In `src/js/api.js`, add to the `api` object (near the other ani-cli-related entries):

```js
checkAniCliSetup: () => invoke('check_anicli_setup', {}),
runAniCliSetup: () => invoke('run_anicli_setup', {}),
confirmSetupConsent: (accepted) => invoke('confirm_setup_consent', { accepted }),
```

- [ ] **Step 3: Add the checklist rendering + init function**

In `src/js/main.js`, add near `initAniCliUpdate` (the existing "Check for ani-cli Update" wiring):

```js
const SETUP_STEP_LABELS = {
  git: 'Git for Windows',
  scoop: 'Scoop package manager',
  bucket: "'extras' bucket",
  'ani-cli': 'ani-cli (+ fzf, mpv)',
};
const SETUP_STATUS_ICONS = {
  checking: '⏳',
  'already-present': '✓',
  done: '✓',
  error: '✗',
  'awaiting-consent': '⏸',
};

function renderSetupStep({ step, status, detail }) {
  const list = document.getElementById('setup-anicli-checklist');
  list.classList.remove('hidden');
  let row = list.querySelector(`[data-step="${step}"]`);
  if (!row) {
    row = document.createElement('div');
    row.dataset.step = step;
    row.className = 'setup-step';
    list.appendChild(row);
  }
  row.classList.toggle('setup-step-error', status === 'error');
  row.innerHTML = `
    <span class="setup-step-icon">${SETUP_STATUS_ICONS[status] || ''}</span>
    <span class="setup-step-label">${escapeAttr(SETUP_STEP_LABELS[step] || step)}</span>
    <span class="setup-step-detail">${escapeAttr(detail)}</span>
  `;

  if (status === 'awaiting-consent') {
    const prompt = document.createElement('div');
    prompt.className = 'setup-consent';
    prompt.innerHTML = `
      <p>${escapeAttr(detail)}</p>
      <button type="button" class="btn btn-primary">Continue</button>
      <button type="button" class="btn btn-ghost">Cancel</button>
    `;
    const [continueBtn, cancelBtn] = prompt.querySelectorAll('button');
    continueBtn.addEventListener('click', () => {
      api.confirmSetupConsent(true);
      prompt.remove();
    });
    cancelBtn.addEventListener('click', () => {
      api.confirmSetupConsent(false);
      prompt.remove();
    });
    list.appendChild(prompt);
  }
}

function initAniCliSetup() {
  const btn = document.getElementById('setup-anicli-btn');
  onEvent('setup-progress', renderSetupStep);
  btn.addEventListener('click', async () => {
    btn.disabled = true;
    document.getElementById('setup-anicli-checklist').innerHTML = '';
    try {
      await api.runAniCliSetup();
      btn.textContent = 'ani-cli is set up';
    } catch (e) {
      btn.textContent = 'Set Up ani-cli';
      console.error('run_anicli_setup error', e);
    } finally {
      btn.disabled = false;
    }
  });
}
```

- [ ] **Step 4: Wire the init call**

In `src/js/main.js`'s `DOMContentLoaded` init block, add next to the existing `initAniCliUpdate();` call:

```js
initAniCliSetup();
```

- [ ] **Step 5: Add minimal styling**

In `src/styles.css`, near the existing `.update-status` rules:

```css
.setup-checklist { margin-top: var(--space-3); display: flex; flex-direction: column; gap: 4px; }
.setup-step { display: flex; align-items: center; gap: var(--space-2); font-size: 0.85em; color: var(--color-subtle); }
.setup-step-error { color: #ef4444; }
.setup-step-detail { color: var(--color-subtle); }
.setup-consent { margin-top: var(--space-2); padding: var(--space-3); border-radius: var(--radius-md); background: var(--color-surface); border: 1px solid var(--color-divider); display: flex; flex-direction: column; gap: var(--space-2); align-items: flex-start; }
```

- [ ] **Step 6: Syntax-check the JS**

Run (PowerShell): `Get-Content -Raw src\js\main.js | node --input-type=module --check` and the same for `src\js\api.js`.
Expected: both exit 0.

- [ ] **Step 7: Manual verification**

Build and run the app, open Settings, click "Set Up ani-cli":
- On a machine that already has everything (like the dev machine): confirm it reports "ani-cli already resolves on PATH" and stops immediately, no other rows appear.
- On a machine/user account missing ani-cli but with Scoop+Git present: confirm only the `bucket`/`ani-cli` rows run.
- On a machine missing Git entirely: confirm the consent prompt appears with the winget-install explanation, "Cancel" stops the flow cleanly with a clear error, "Continue" proceeds.

- [ ] **Step 8: Commit**

```bash
git add src/index.html src/js/api.js src/js/main.js src/styles.css
git commit -m "Add Settings UI for the ani-cli setup checklist"
```

---

## Note on git

`trela/` is not currently a git repository (confirmed earlier in this project: it shows as untracked even in the loose parent folder it sits in). The commit steps above are written as if it were, per this plan format's convention — if it's still not a repo when this plan is executed, skip the `git add`/`git commit` steps and just confirm each task's verification step instead.

## Self-review

**Spec coverage:** dependency chain findings → Task 1 (curl-impersonate) + Task 2/3 (ani-cli via Scoop); chosen approach (reuse Scoop) → Task 2; flow steps 1-5 → `run_anicli_setup`; both consent points → the `awaiting-consent` status + `confirm_setup_consent` command; UI (checklist rows, inline prompt, no modal) → Task 3; backend/events architecture → Task 2's `SetupProgress`/`setup-progress` event; curl-impersonate sidecar → Task 1 in full. Testing plan → covered by each task's manual-verification step plus the two real unit tests in Task 2. No spec section without a corresponding task.

**Placeholder scan:** no TBD/TODO; every step has real code or an exact, verified command; the curl-impersonate download step uses a URL and filename confirmed to exist and resolve to a real 4MB Windows PE executable during planning, not a guessed one.

**Type consistency:** `SetupProgress { step, status, detail }` (Task 2) matches the `{ step, status, detail }` destructuring in `renderSetupStep` (Task 3) and the event name `"setup-progress"` matches on both sides. `check_anicli_setup`/`run_anicli_setup`/`confirm_setup_consent` command names match exactly between `lib.rs` registration, `api.js`'s `invoke(...)` calls, and this plan's Interfaces sections. `sidecar_path`/`curl_impersonate_path` signatures in Task 1 match how they're called in the modified `curl_impersonate_get`.
