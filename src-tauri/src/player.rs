use crate::procutil::find_exe;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn ani_cli_path() -> String {
    // The extensionless "ani-cli" shim is a POSIX shell script and isn't
    // runnable by CreateProcess, so .cmd/.exe/.bat must come first.
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["ani-cli.cmd", "ani-cli.exe", "ani-cli.bat"]
    } else {
        &["ani-cli"]
    };
    find_exe(names).unwrap_or_else(|| names[0].to_string())
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
    let path = ani_cli_path();
    let index = index.to_string();
    let quality = if quality.is_empty() { "best" } else { quality };

    let mut child = Command::new(&path)
        // --exit-after-play: run the player in the foreground and quit
        // ani-cli as soon as it closes, instead of dropping into
        // ani-cli's own next/replay/quality terminal menu afterward.
        .args(["-S", &index, "-e", episode, "-q", quality, "--exit-after-play", query])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to launch ani-cli: {}", e))?;

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
    let path = ani_cli_path();

    let output = Command::new(&path)
        .arg("-U")
        .output()
        .map_err(|e| format!("Failed to run ani-cli -U: {}", e))?;

    let mut text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&stderr);
        }
        if text.is_empty() {
            text = format!("ani-cli -U exited with status {}", output.status);
        }
        return Err(text);
    }

    if text.is_empty() {
        text = "ani-cli is up to date.".to_string();
    }
    Ok(text)
}
