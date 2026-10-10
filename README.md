# Trela

A Tauri desktop app for browsing and watching anime, movies and series. Trela
handles discovery and metadata itself (AniList for anime, TMDB for movies/series)
and hands playback off to **mpv**, launched via **ani-cli** for anime and
**MovieBox** / **4KHDHub** resolvers for movies and series — the app is a
browser and launcher, not a video player.

## Screenshots

<!-- Drop your own screenshots into docs/screenshots/ with these filenames,
     or update the paths below to match whatever you save. -->

| Home | Browse |
| --- | --- |
| ![Home](docs/screenshots/home.png) | ![Browse](docs/screenshots/anime.png) |

| Watching — Anime | Watching — Series |
| --- | --- |
| ![Watching, anime](docs/screenshots/anime.png) | ![Watching, series](docs/screenshots/series.png) |

## Features

- **Anime** — seasonal charts, trending, genre filters, and recommendations
  from AniList; playback resolved and launched through `ani-cli` with a
  Best/1080p/720p/480p/360p/Worst quality picker.
- **Movies & Series** — search, genre/year discovery, and trending from TMDB;
  streaming sources merged from **MovieBox** and **4KHDHub** into one list,
  sorted by quality and mirror count, with a language picker and a
  Download-and-play fallback for sources that won't stream reliably.
- **Watching page** — a main panel + sidebar layout: launch status, live
  playback settings (quality, source, language, subtitles), a horizontal
  episode strip for jumping between episodes without leaving the page, and
  "More like this" recommendations.
- **Continue Watching** — resumes exactly where mpv left off, anime, movies
  and series alike, with a progress bar on the Home page.
- **Wishlist & notifications** — track titles and get notified when a
  tracked anime/series has new episodes or a wishlisted movie releases.
- **Light/dark theme**, a command palette (`Ctrl+K`), and a backup/restore
  export for your wishlist, continue-watching list, and preferences.

## Tech stack

- [Tauri 2](https://tauri.app/) (Rust backend, vanilla HTML/CSS/JS frontend —
  no bundler, no framework)
- [ani-cli](https://github.com/pystardust/ani-cli) for anime resolution/playback
- [moviebox-tui](https://crates.io/crates/moviebox-tui) for MovieBox/4KHDHub
  resolution
- [mpv](https://mpv.io/) as the actual video player, launched externally
- [AniList](https://anilist.co/) and [TMDB](https://www.themoviedb.org/) APIs
  for metadata

## Install (Windows)

1. Download `trela_<version>_x64-setup.exe` from the
   [latest release](https://github.com/Mamalakisgr/TrelaPlayer/releases/latest)
   and run it. No admin rights are needed. Windows SmartScreen may warn
   about an unrecognised app because the installer isn't code-signed: click
   **More info → Run anyway**.
2. **Movies & Series** work straight away. mpv comes bundled. They need a
   free TMDB API key (themoviedb.org → Settings → API, no cost, no card),
   entered under **About** in the app.
3. **Anime** needs a one-time setup: open **About** and click
   **Set Up Anime Playback**. It downloads ani-cli and the few tools it
   depends on into `%LOCALAPPDATA%\Trela\tools`. That folder is all it
   touches: no admin rights, no PATH or system changes. If you already
   have `ani-cli` on your PATH (e.g. via Scoop), Trela uses that instead.

To remove Trela, uninstall it from Windows Settings → Apps, then delete
`%LOCALAPPDATA%\Trela` if you ran the anime setup.

## Building from source

Prerequisites: [Rust](https://www.rust-lang.org/tools/install) and the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) (Microsoft C++
Build Tools and WebView2), plus the Tauri CLI (`cargo install tauri-cli`).
The anime tools come from the same in-app setup as above, or from running
the script directly:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\setup-anime.ps1
```

```bash
cargo tauri dev
```

## Development

`src/index.html`, `src/js/*.js`, and `src/styles.css` are plain static files —
no build step, no bundler. The Rust/Tauri backend lives in `src-tauri/src`.
Run `cargo tauri dev` to launch with hot-reload on the frontend files.

## Building a release

Bump `version` in `src-tauri/tauri.conf.json`, then push a matching tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

The [Release workflow](.github/workflows/release.yml) builds the installer
and attaches it to a draft GitHub release for you to review and publish. To
build locally instead, run `cargo tauri build`; the installer lands in
`src-tauri/target/release/bundle/nsis/`.
