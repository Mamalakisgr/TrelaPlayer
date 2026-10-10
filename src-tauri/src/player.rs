use crate::procutil::{find_tool, tool_path_env, ANIME_SETUP_HINT};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Emitter, Manager};

fn ani_cli_path() -> String {
    // The extensionless "ani-cli" shim is a POSIX shell script and isn't
    // runnable by CreateProcess, so .cmd/.exe/.bat must come first.
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["ani-cli.cmd", "ani-cli.exe", "ani-cli.bat"]
    } else {
        &["ani-cli"]
    };
    find_tool(names).unwrap_or_else(|| names[0].to_string())
}

// Every ani-cli run goes through here so it can see the tools dir and the
// bundled mpv (see tool_path_env).
fn ani_cli_command() -> Command {
    let mut cmd = Command::new(ani_cli_path());
    cmd.env("PATH", tool_path_env());
    cmd
}

fn launch_error(action: &str, e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        ANIME_SETUP_HINT.to_string()
    } else {
        format!("Failed to {}: {}", action, e)
    }
}

// ani-cli's die() wraps its error text in ANSI color codes (via `printf
// "\033[1;31m%s\033[0m"`), which would otherwise show up as garbled escape
// sequences in the GUI.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip the ESC and the rest of the CSI sequence up to its final
            // byte (an ASCII letter) — e.g. "[2K", "[1;31m", "[0m".
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out.trim().to_string()
}

// `query`/`index` non-interactively replay a specific search result (-S
// index) against ani-cli's own search for `query`, resolved ahead of time by
// resolve_anime (anidb.rs) — trusting a blind `-S 1` on the raw AniList title
// is exactly what let mismatched/wrong episodes through for multi-season or
// ambiguous titles. `episode` is expected to already be in whatever
// numbering ani-cli's matched entry actually uses (also resolve_anime's job
// to work out — it can be global across seasons, not always 1..N).
#[tauri::command]
pub fn watch_episode(query: &str, index: u32, episode: &str, quality: &str) -> Result<String, String> {
    let index = index.to_string();
    let quality = if quality.is_empty() { "best" } else { quality };

    let mut child = ani_cli_command()
        // --exit-after-play: run the player in the foreground and quit
        // ani-cli as soon as it closes, instead of dropping into
        // ani-cli's own next/replay/quality terminal menu afterward.
        .args(["-S", &index, "-e", episode, "-q", quality, "--exit-after-play", query])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| launch_error("launch ani-cli", e))?;

    // ani-cli validates the search result and episode against its own
    // (different) source before it ever gets far enough to launch a player,
    // so a mismatch like "Invalid episode!" surfaces almost immediately.
    // Give it a short grace window to fail fast and report that clearly;
    // if it's still running past that, treat it as playing and let it run
    // detached for the rest of the (long) viewing session, same as before.
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    // Measured ani-cli's own fast-fail path (dependency check + search +
    // episode validation, no player launch) at a consistent ~3.8-4.0s on
    // this machine — 8s gives real margin over that before assuming success.
    let deadline = Instant::now() + Duration::from_secs(8);

    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let mut text = String::new();
            if let Some(s) = stderr.as_mut() {
                let _ = s.read_to_string(&mut text);
            }
            if text.trim().is_empty() {
                if let Some(s) = stdout.as_mut() {
                    let _ = s.read_to_string(&mut text);
                }
            }
            let text = strip_ansi(&text);

            return if status.success() {
                Ok(format!("Playing episode {}...", episode))
            } else if text.contains("Invalid episode") {
                Err(format!(
                    "ani-cli couldn't find episode {episode} for \"{query}\" (search result #{index}) — it may not \
                     be out on ani-cli's source yet, or the episode numbering resolved doesn't match what's actually there."
                ))
            } else if !text.is_empty() {
                Err(text)
            } else {
                Err(format!("ani-cli exited without playing (status {})", status))
            };
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }

    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(format!("Playing episode {}...", episode))
}

#[tauri::command]
pub fn update_ani_cli() -> Result<String, String> {
    // `ani-cli -U` self-updates: it diffs the running script against the
    // latest master and patches itself in place. It's fully non-interactive
    // (no prompts), so this is safe to block on and run straight from the UI.
    let output = ani_cli_command()
        .arg("-U")
        .output()
        .map_err(|e| launch_error("run ani-cli -U", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let text = [stdout, stderr].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
        return Err(if text.is_empty() { format!("ani-cli -U exited with status {}", output.status) } else { text });
    }

    Ok(if stdout.is_empty() { "ani-cli is up to date.".to_string() } else { stdout })
}

#[tauri::command]
pub fn get_ani_cli_version() -> Result<String, String> {
    let output = ani_cli_command()
        .arg("--version")
        .output()
        .map_err(|e| launch_error("run ani-cli", e))?;
    let text = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    if text.is_empty() {
        return Err("ani-cli --version returned no output".to_string());
    }
    Ok(text)
}

// Runs scripts/setup-anime.ps1 (bundled as a resource, see tauri.conf.json),
// streaming each line it prints to the About page as an "anime-setup-log"
// event: on a machine without Git it downloads ~65 MB, too long to sit on a
// silent spinner. The full log is also the return value, so the page ends on
// the complete transcript either way.
#[tauri::command]
pub async fn setup_anime_tools(app: AppHandle) -> Result<String, String> {
    let script = app
        .path()
        .resolve("setup-anime.ps1", BaseDirectory::Resource)
        .map_err(|e| format!("Couldn't locate the bundled setup script: {}", e))?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| format!("Failed to start PowerShell: {}", e))?;

        // Lossy, line by line: PowerShell writes in the console's code page,
        // and one non-UTF-8 byte (a localized error message) must not stop
        // the read and leave the script blocked on a full pipe.
        let mut log = String::new();
        if let Some(stdout) = child.stdout.take() {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            while reader.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                buf.clear();
                let _ = app.emit("anime-setup-log", &line);
                log.push_str(&line);
                log.push('\n');
            }
        }
        let mut stderr = Vec::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_end(&mut stderr);
        }
        log.push_str(&String::from_utf8_lossy(&stderr));

        let status = child.wait().map_err(|e| format!("Setup script didn't finish: {}", e))?;
        let log = log.trim().to_string();
        if status.success() {
            Ok(log)
        } else if log.is_empty() {
            Err(format!("Setup script exited with status {}", status))
        } else {
            Err(log)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

// moviebox-tui is compiled directly into this binary, not shelled out to, so
// its version isn't something a running process can query — it's baked in at
// compile time by build.rs (which reads the exact version Cargo.lock
// resolved, since Cargo.toml itself only pins a minimum).
#[tauri::command]
pub fn get_moviebox_tui_version() -> String {
    env!("MOVIEBOX_TUI_VERSION").to_string()
}

// Unlike ani-cli, moviebox-tui isn't shelled out to at runtime — it's a Rust
// crate compiled directly into this binary (see src-tauri/Cargo.toml). A
// running process can't hot-swap its own statically-linked code, so this
// can't make Trela use a new version by itself; what it *can* do is bump
// Cargo.lock to the newest version crates.io has, same as running this by
// hand. Cargo.toml pins "0.1.14" (a `^0.1.14` requirement — cargo will
// resolve any 0.1.x >= 14, which is where every release so far has landed),
// so plain `cargo update` already fetches whatever's newest without needing
// to query crates.io or edit Cargo.toml separately. Only works from a source
// checkout with cargo on PATH — there's no Cargo.toml to update once Trela
// is packaged/installed, and that failure mode is reported as a normal error.
//
// Verified live against this exact dependency: 0.1.14 -> 0.1.15 removed
// `releases_to_moviebox_json`, breaking fourkhdhub.rs, despite being a
// semver-"compatible" 0.x bump. A 0.x crate cannot be trusted not to do that
// again, so this checks the new version actually compiles before reporting
// success, and rolls Cargo.lock back to the version it started from if not —
// leaving the tree broken after a Settings-page button click is worse than
// just telling the user the new release needs code changes first.
#[tauri::command]
pub fn update_moviebox_tui() -> Result<String, String> {
    let cargo_exe = if cfg!(target_os = "windows") { "cargo.exe" } else { "cargo" };
    // Each Command below relies on cargo finding src-tauri/Cargo.toml from
    // the current directory — true when launched via `cargo tauri dev` from
    // src-tauri itself, but not guaranteed for every source-checkout launch
    // path (a desktop shortcut, an IDE run config, `tauri dev` from the repo
    // root). CARGO_MANIFEST_DIR is a compile-time constant that always points
    // at src-tauri regardless of the *running* process's cwd, so pin it
    // explicitly instead of trusting an inherited cwd this feature's whole
    // job is to not have to trust.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let update_output = Command::new(cargo_exe)
        .args(["update", "--package", "moviebox-tui"])
        .current_dir(manifest_dir)
        .output()
        .map_err(|e| format!("Failed to run cargo (this only works from a source checkout): {}", e))?;

    let update_text = strip_ansi(&String::from_utf8_lossy(&update_output.stderr));
    if !update_output.status.success() {
        return Err(if update_text.is_empty() { format!("cargo update exited with status {}", update_output.status) } else { update_text });
    }
    // Whether anything actually changed has to be read off the version line,
    // not off stderr being empty. A no-op `cargo update` still writes
    // "Updating crates.io index" and "Locking 0 packages to latest compatible
    // versions", so an emptiness test never fires — every no-op fell through
    // to the compile check below, and any unrelated failure there was then
    // reported as "the new version doesn't compile", about a version that was
    // never installed and with no previous version to roll back to.
    let versions = update_text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("Updating moviebox-tui ")?;
        let (old, new) = rest.split_once(" -> ")?;
        Some((old.trim_start_matches('v').to_string(), new.trim_start_matches('v').to_string()))
    });
    let Some((old_version, new_version)) = versions else {
        return Ok("moviebox-tui is already at the latest version Cargo.toml allows.".to_string());
    };

    // Keep cargo's output rather than reducing it to a success bit: "doesn't
    // compile" with nothing else left the user no way to see what broke, and
    // made a cargo that failed to launch indistinguishable from code that
    // genuinely doesn't build.
    let check = Command::new(cargo_exe)
        .args(["check", "--package", "trela"])
        .current_dir(manifest_dir)
        .output();
    if matches!(&check, Ok(o) if o.status.success()) {
        return Ok(format!("{}\n\nRebuild and restart Trela for this to take effect.", update_text.trim()));
    }
    let check_detail = match &check {
        Ok(o) => {
            let text = strip_ansi(&String::from_utf8_lossy(&o.stderr));
            // Keep the "-->" location lines alongside the "error" headlines:
            // "this method takes 2 arguments but 1 was supplied" is close to
            // useless without the file and line it happened in, which is what
            // the first report of this actually looked like.
            let errors: Vec<String> = text
                .lines()
                .filter(|l| l.starts_with("error") || l.trim_start().starts_with("--> "))
                .take(12)
                .map(str::to_string)
                .collect();
            if errors.is_empty() {
                let tail: Vec<&str> = text.lines().rev().take(10).collect();
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            } else {
                errors.join("\n")
            }
        }
        Err(e) => format!("cargo check could not be run: {e}"),
    };
    let rollback_ok = Command::new(cargo_exe)
        .args(["update", "--package", "moviebox-tui", "--precise", &old_version])
        .current_dir(manifest_dir)
        .output()
        .is_ok_and(|o| o.status.success());
    if rollback_ok {
        Err(format!(
            "moviebox-tui {new_version} doesn't compile against Trela's current code (breaking change in a \"compatible\" 0.x release) — rolled back to {old_version}. Code changes are needed before updating further.\n\n{check_detail}"
        ))
    } else {
        Err(format!(
            "moviebox-tui {new_version} doesn't compile against Trela's current code, AND rolling back to {old_version} failed too — run `cargo update -p moviebox-tui --precise {old_version}` manually in src-tauri.\n\n{check_detail}"
        ))
    }
}
