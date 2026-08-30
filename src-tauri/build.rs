fn main() {
    tauri_build::build();

    // Cargo.toml only pins a minimum ("^0.1.14") for moviebox-tui — this
    // reads the exact version Cargo.lock actually resolved (e.g. after
    // `cargo update`), which is what's really compiled into this binary, and
    // exposes it via env!("MOVIEBOX_TUI_VERSION") for the About page.
    let lock = std::fs::read_to_string("Cargo.lock").unwrap_or_default();
    let version = lock
        .split_once("name = \"moviebox-tui\"")
        .and_then(|(_, rest)| rest.split_once("version = \""))
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(version, _)| version)
        .unwrap_or("unknown");
    println!("cargo:rustc-env=MOVIEBOX_TUI_VERSION={}", version);
    println!("cargo:rerun-if-changed=Cargo.lock");
}
