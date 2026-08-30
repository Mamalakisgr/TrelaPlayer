use crate::models::{
    DownloadCompleteEvent, DownloadErrorEvent, DownloadProgressEvent, DownloadsInfo, LanguageOption, MovieSubject,
    StreamOption, SubtitleOption,
};
use moviebox_tui::download::{download, safe_file_stem, DownloadOutcome};
use moviebox_tui::providers::moviebox::client::MovieBoxClient;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::ShellExt;
use tokio::sync::Mutex;

// MovieBoxClient is cheap to clone (Arc-backed internals) and caching the
// one instance keeps the runtime auth token `init()` fetches instead of
// re-fetching it on every play.
static CLIENT: OnceLock<Mutex<Option<MovieBoxClient>>> = OnceLock::new();

async fn get_client() -> Result<MovieBoxClient, String> {
    let lock = CLIENT.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().await;
    if let Some(client) = guard.as_ref() {
        return Ok(client.clone());
    }

    let client = MovieBoxClient::new();
    client
        .init()
        .await
        .map_err(|e| format!("Failed to initialize MovieBox client: {:?}", e))?;
    *guard = Some(client.clone());
    Ok(client)
}

// The search response groups subjects by tab (movies/tv/anime/...) under
// `results[].subjects[]` rather than a flat top-level array.
fn extract_subjects(search_json: &serde_json::Value) -> Vec<serde_json::Value> {
    search_json["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|group| group["subjects"].as_array())
        .flatten()
        .cloned()
        .collect()
}

// MovieBox subjects carry their own `releaseDate` (e.g. "1994-11-19") — a
// much stronger disambiguator than title text alone when a title is indexed
// more than once for genuinely different shows (e.g. more than one cartoon
// titled plainly "Spider-Man" from different decades).
fn subject_year(s: &serde_json::Value) -> Option<i32> {
    s["releaseDate"].as_str()?.split('-').next()?.parse().ok()
}

// Lower is better. Year is checked first since an exact title match is
// common across unrelated shows sharing a name, while a year mismatch (when
// both sides actually know a year) is a strong signal of the wrong show
// entirely. +-1 tolerance covers TMDB/MovieBox disagreeing on which regional
// release or air date counts as "the" year. Title exactness is the
// secondary tiebreaker, same as before this used year at all.
fn rank_subject(s: &serde_json::Value, title: &str, expected_year: Option<u32>) -> (u8, u8) {
    let year_rank = match (expected_year, subject_year(s)) {
        (Some(e), Some(a)) if (a - e as i32).abs() <= 1 => 0,
        (Some(_), Some(_)) => 2,
        _ => 1,
    };
    let title_rank = match s["title"].as_str() {
        Some(t) if t == title => 0,
        Some(t) if !t.contains('[') => 1,
        _ => 2,
    };
    (year_rank, title_rank)
}

fn best_subject_id(subjects: &[serde_json::Value], title: &str, expected_year: Option<u32>) -> Option<String> {
    subjects
        .iter()
        .min_by_key(|s| rank_subject(s, title, expected_year))
        .and_then(|s| s["subjectId"].as_str())
        .map(|s| s.to_string())
}

// A bare-title search only returns MovieBox's own single page of "most
// relevant" results by its own ranking, which can bury — or simply omit —
// the right listing when a title has been reused across multiple decades of
// unrelated shows. E.g. searching plain "Spider-Man" doesn't surface the
// 1994 animated series' listing anywhere in its own top results at all, but
// a year-qualified query ("Spider-Man 1994") returns it as the very first
// result. So when a year is known and the plain-title search doesn't
// already contain a year-matching candidate, this re-queries with the year
// appended and pools both result sets (deduped by subjectId) before ranking.
async fn search_candidates(client: &MovieBoxClient, title: &str, expected_year: Option<u32>) -> Result<Vec<serde_json::Value>, String> {
    let search = client.search(title, 1).await.map_err(|e| format!("Search failed: {:?}", e))?;
    let mut subjects = extract_subjects(&search);

    if let Some(year) = expected_year {
        let has_year_match = subjects.iter().any(|s| matches!(subject_year(s), Some(a) if (a - year as i32).abs() <= 1));
        if !has_year_match {
            let year_query = format!("{} {}", title, year);
            if let Ok(more_search) = client.search(&year_query, 1).await {
                let mut seen: std::collections::HashSet<String> =
                    subjects.iter().filter_map(|s| s["subjectId"].as_str().map(str::to_string)).collect();
                subjects.extend(
                    extract_subjects(&more_search).into_iter().filter(|s| s["subjectId"].as_str().is_some_and(|id| seen.insert(id.to_string()))),
                );
            }
        }
    }

    Ok(subjects)
}

async fn find_subject_id(client: &MovieBoxClient, title: &str, expected_year: Option<u32>) -> Result<String, String> {
    let subjects = search_candidates(client, title, expected_year).await?;
    best_subject_id(&subjects, title, expected_year).ok_or_else(|| format!("No results found for \"{}\"", title))
}

// MovieBox's /resource endpoint accepts `se`/`ep` query params (what
// get_resources sends) but doesn't reliably honor them as a filter — for a
// multi-season show it silently returns the same season-1-heavy page
// regardless of which season/episode was requested. fetch_resource_page
// (no se/ep params, just plain pagination) returns the real unfiltered list,
// which does contain every season mixed together — so this pages through it
// and filters client-side for the target episode, the same way moviebox-tui's
// own interactive TUI resolves episode streams (see its Action::FetchEpisodeStreams
// handling — it does the identical page-and-filter loop, capped at 60 pages).
//
// Crucially, a single "all resolutions together" pagination stream stops as
// soon as *any* item matches the target episode — which can be a lower-
// resolution encode that just happened to be indexed earlier, hiding a
// higher-resolution copy of the same episode sitting a few pages further in.
// So each resolution tier is paged through separately (and in parallel,
// mirroring moviebox-tui's own per-resolution fetch), each stopping only
// once *it* finds the episode, instead of one shared stream stopping the
// moment anything at all matches.
async fn fetch_episode_resources(
    client: &MovieBoxClient,
    subject_id: &str,
    season: usize,
    episode: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let is_movie = season == 0 && episode == 0;

    if is_movie {
        let mut items = Vec::new();
        for page in 1..=10 {
            let (page_items, pager) = client
                .fetch_resource_page(subject_id, 0, page)
                .await
                .map_err(|e| format!("Failed to fetch sources: {:?}", e))?;
            let has_more = pager["hasMore"].as_bool().unwrap_or(false);
            items.extend(page_items);
            if !has_more {
                break;
            }
        }
        return Ok(items);
    }

    let resolutions = client.fetch_collection_resolutions(subject_id).await.unwrap_or_else(|_| vec![1080, 720, 480, 360]);

    let tasks: Vec<_> = resolutions
        .into_iter()
        .map(|resolution| {
            let client = client.clone();
            let subject_id = subject_id.to_string();
            tokio::spawn(async move {
                let mut items = Vec::new();
                for page in 1..=60 {
                    let Ok((page_items, pager)) = client.fetch_resource_page(&subject_id, resolution, page).await else { break };
                    let has_more = pager["hasMore"].as_bool().unwrap_or(false);
                    let found = page_items
                        .iter()
                        .any(|item| item["se"].as_u64() == Some(season as u64) && item["ep"].as_u64() == Some(episode as u64));
                    items.extend(page_items);
                    if found || !has_more {
                        break;
                    }
                }
                items
            })
        })
        .collect();

    let mut items = Vec::new();
    for task in tasks {
        items.extend(task.await.unwrap_or_default());
    }

    let mut seen = std::collections::HashSet::new();
    Ok(items
        .into_iter()
        .filter(|item| item["se"].as_u64() == Some(season as u64) && item["ep"].as_u64() == Some(episode as u64))
        .filter(|item| seen.insert(item["resourceId"].as_str().unwrap_or_default().to_string()))
        .collect())
}

// Long-running shows are sometimes indexed multiple times on MovieBox — e.g.
// two dead-end "House of the Dragon" listings that only have Season 1
// (one of them even `subjectType`d as a movie, which can never have
// episodes), a "House of the Dragon [Hindi] S1-S3" dub, and finally the
// plain "House of the Dragon S1-S3" listing that's the one that actually
// has season 3. Picking just the first exact-title match can land on a
// listing that's simply missing the requested episode — or, worse, on an
// alternate-language dub or an unrelated same-titled show before ever
// reaching the right listing — so this ranks candidates (year match first
// when known, then exact title, then non-dub titles, then bracketed dub
// variants like "[Hindi]" last) and tries them in that order until one
// actually has resources for season/episode.
async fn find_playable_subject(
    client: &MovieBoxClient,
    title: &str,
    season: usize,
    episode: usize,
    expected_year: Option<u32>,
) -> Result<(String, Vec<serde_json::Value>), String> {
    let mut subjects = search_candidates(client, title, expected_year).await?;
    if subjects.is_empty() {
        return Err(format!("No results found for \"{}\"", title));
    }

    // subjectType 1 is a movie — it can never satisfy a season/episode request.
    if season > 0 || episode > 0 {
        subjects.retain(|s| !matches!(s["subjectType"].as_i64(), Some(1)));
    }

    subjects.sort_by_key(|s| rank_subject(s, title, expected_year));

    let mut fallback: Option<(String, Vec<serde_json::Value>)> = None;
    for subject in subjects.iter().take(8) {
        let Some(subject_id) = subject["subjectId"].as_str() else { continue };
        let Ok(items) = fetch_episode_resources(client, subject_id, season, episode).await else { continue };
        if !items.is_empty() {
            return Ok((subject_id.to_string(), items));
        }
        if fallback.is_none() {
            fallback = Some((subject_id.to_string(), items));
        }
    }

    fallback.ok_or_else(|| format!("No results found for \"{}\"", title))
}

// MovieBox models each dub as its own subject (its own subjectId, own
// resource/episode list) rather than a language flag on one subject — so
// "picking a language" means switching which subject subsequent
// get_stream_options/get_subtitle_options calls target.
#[tauri::command]
pub async fn get_movie_subject(title: String, year: Option<u32>) -> Result<MovieSubject, String> {
    let client = get_client().await?;
    let subject_id = find_subject_id(&client, &title, year).await?;
    let details = client
        .get_details(&subject_id)
        .await
        .map_err(|e| format!("Failed to fetch details: {:?}", e))?;

    let dubs = details["dubs"].as_array().cloned().unwrap_or_default();
    let languages = if dubs.len() > 1 {
        dubs.iter()
            .filter_map(|d| {
                Some(LanguageOption {
                    subject_id: d["subjectId"].as_str()?.to_string(),
                    name: d["lanName"].as_str().unwrap_or("Unknown").to_string(),
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(MovieSubject { subject_id, languages })
}

fn format_size(bytes_str: &str) -> String {
    let bytes: f64 = bytes_str.parse().unwrap_or(0.0);
    let mb = bytes / 1024.0 / 1024.0;
    if bytes <= 0.0 {
        "Unknown size".to_string()
    } else if mb > 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{:.0} MB", mb)
    }
}

// Every distinct resource entry offers — not just the highest resolution —
// so the GUI can let the user pick between e.g. two 1080p encodes from
// different uploaders instead of silently taking whichever one a
// resolution-only comparison happens to return first. Shared with
// fourkhdhub.rs: moviebox-tui's own releases_to_moviebox_json() converts
// 4KHDHub releases into this exact same MovieBox resource-list JSON shape,
// so one extractor covers both providers instead of two.
pub(crate) fn extract_stream_options(subject_id: &str, items: &[serde_json::Value]) -> Vec<StreamOption> {
    let mut options: Vec<StreamOption> = items
        .iter()
        .filter_map(|item| {
            Some(StreamOption {
                resource_link: item["resourceLink"].as_str()?.to_string(),
                resolution: item["resolution"].as_u64().unwrap_or(0),
                codec: item["codecName"].as_str().unwrap_or("unknown").to_uppercase(),
                size: format_size(item["size"].as_str().unwrap_or("0")),
                uploader: item["uploadBy"].as_str().unwrap_or("Unknown").to_string(),
                subject_id: subject_id.to_string(),
                resource_id: item["resourceId"].as_str().unwrap_or("").to_string(),
                fourk_release: item.get("_fourk_release").cloned(),
            })
        })
        .collect();
    options.sort_by(|a, b| b.resolution.cmp(&a.resolution));
    options
}

// Best-effort: subtitles are a nice-to-have, so any failure here just means
// mpv opens without external sub tracks rather than failing playback.
async fn fetch_caption_urls(client: &MovieBoxClient, subject_id: &str, resource_id: &str) -> Vec<String> {
    if resource_id.is_empty() {
        return vec![];
    }
    client
        .get_ext_captions(subject_id, resource_id)
        .await
        .ok()
        .and_then(|json| json["extCaptions"].as_array().cloned())
        .into_iter()
        .flatten()
        .filter_map(|c| c["url"].as_str().map(|s| s.to_string()))
        .collect()
}

// mpv is bundled as a Tauri sidecar (see tauri.conf.json's externalBin and
// src-tauri/binaries/) rather than located on PATH, so movie/series playback
// works standalone on a machine that never installed mpv itself — unlike
// anime (ani-cli) and its own dependency chain, which still isn't bundled.
// mpv's d3d11 video output needs d3dcompiler_43.dll — bundled as a resource
// mapped to land in the same directory as the sidecar (verified against the
// actual generated NSIS installer.nsi: both install to $INSTDIR directly),
// so Windows' normal same-directory DLL search finds it with no extra code.

// Every subtitle track gets its own --sub-file so mpv adds them all as
// external sub tracks; mpv's own `j`/`J` cycles between them, so there's no
// need for a subtitle picker in the GUI.
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
        sidecar = sidecar.args(["--cache=yes", "--demuxer-max-bytes=500MiB", "--demuxer-max-back-bytes=150MiB"]);
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

#[tauri::command]
pub async fn get_stream_options(
    title: String,
    season: u32,
    episode: u32,
    subject_id: Option<String>,
    year: Option<u32>,
) -> Result<Vec<StreamOption>, String> {
    let client = get_client().await?;
    // A caller that already resolved a specific subject (e.g. after picking
    // a language) passes it directly and is trusted as-is; otherwise resolve
    // via the season/episode-aware search so a title with multiple/partial
    // MovieBox listings doesn't land on one that's missing this episode.
    let (subject_id, items) = match subject_id {
        Some(id) => {
            let items = fetch_episode_resources(&client, &id, season as usize, episode as usize).await?;
            (id, items)
        }
        None => find_playable_subject(&client, &title, season as usize, episode as usize, year).await?,
    };

    let options = extract_stream_options(&subject_id, &items);
    if options.is_empty() {
        return Err("No playable source found for this title/episode.".to_string());
    }
    Ok(options)
}

#[tauri::command]
pub async fn get_subtitle_options(subject_id: String, resource_id: String) -> Result<Vec<SubtitleOption>, String> {
    let client = get_client().await?;
    let captions = client
        .get_ext_captions(&subject_id, &resource_id)
        .await
        .map_err(|e| format!("Failed to fetch subtitles: {:?}", e))?;

    Ok(captions["extCaptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            let url = c["url"].as_str()?.to_string();
            let language = c["lanName"].as_str().unwrap_or("Unknown").to_string();
            Some(SubtitleOption { url, language })
        })
        .collect())
}

#[tauri::command]
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

// Some MovieBox resource links don't serve well as a direct stream (no Range
// support, throttled, etc.) — downloading the whole file first and playing
// the local copy is the reliable fallback moviebox-tui itself falls back to.
fn downloads_base_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Trela")
}

fn download_destination(title: &str, season: u32, episode: u32) -> PathBuf {
    let safe_title = safe_file_stem(title);
    let base = downloads_base_dir();
    let (target_dir, base_name) = if season == 0 && episode == 0 {
        (base.join("Movies"), safe_title.clone())
    } else {
        (
            base.join("Series").join(&safe_title).join(format!("Season {season}")),
            format!("{safe_title}_S{season:02}E{episode:02}"),
        )
    };
    let mut destination = target_dir.join(format!("{base_name}.mp4"));
    let mut counter = 2;
    while destination.exists() {
        destination = target_dir.join(format!("{base_name}_{counter}.mp4"));
        counter += 1;
    }
    destination
}

// Downloads run in the background and report progress/completion via events
// ("download-progress" / "download-complete" / "download-error") rather than
// blocking this command for the whole transfer — a movie download can run
// for minutes, and the frontend needs to stay responsive (and see progress)
// while it does. Only one download is ever in flight, so events carry no id.
#[tauri::command]
pub async fn start_download(
    app: AppHandle,
    resource_link: String,
    title: String,
    season: u32,
    episode: u32,
    window_title: String,
    subtitle_url: Option<String>,
) -> Result<(), String> {
    let client = get_client().await?;
    let http_client = client.http_client().clone();
    let destination = download_destination(&title, season, episode);

    tokio::spawn(async move {
        if let Some(parent) = destination.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                let _ = app.emit(
                    "download-error",
                    DownloadErrorEvent { message: format!("Cannot create download directory: {e}") },
                );
                return;
            }
        }

        let mut sub_files = Vec::new();
        if let Some(subtitle_url) = subtitle_url {
            let subtitle_path = destination.with_extension("srt");
            if let Ok(response) = http_client.get(&subtitle_url).send().await {
                if let Ok(bytes) = response.bytes().await {
                    if tokio::fs::write(&subtitle_path, &bytes).await.is_ok() {
                        sub_files.push(subtitle_path.to_string_lossy().into_owned());
                    }
                }
            }
            // A failed subtitle fetch isn't fatal — the video download still proceeds without it.
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let progress_app = app.clone();
        let result = download(&http_client, &resource_link, &destination, cancel, move |progress| {
            let _ = progress_app.emit(
                "download-progress",
                DownloadProgressEvent {
                    downloaded: progress.downloaded,
                    total: progress.total,
                    bytes_per_second: progress.bytes_per_second,
                },
            );
        })
        .await;

        match result {
            Ok(DownloadOutcome::Completed { .. }) => {
                let path = destination.to_string_lossy().into_owned();
                let _ = app.emit("download-complete", DownloadCompleteEvent { path: path.clone() });
                if let Err(e) = spawn_mpv(&app, &path, &window_title, &sub_files) {
                    let _ = app.emit("download-error", DownloadErrorEvent { message: e });
                }
            }
            Ok(DownloadOutcome::Paused { .. }) => {
                let _ = app.emit(
                    "download-error",
                    DownloadErrorEvent { message: "Download stopped before finishing.".to_string() },
                );
            }
            Err(e) => {
                let _ = app.emit("download-error", DownloadErrorEvent { message: e.to_string() });
            }
        }
    });

    Ok(())
}

// Recursively sums file count/size under `dir` — a missing/unreadable
// directory (nothing downloaded yet) just counts as empty rather than erroring.
async fn dir_stats(dir: &std::path::Path) -> (u32, u64) {
    let mut stack = vec![dir.to_path_buf()];
    let mut count = 0u32;
    let mut bytes = 0u64;
    while let Some(current) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&current).await else { continue };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if let Ok(metadata) = entry.metadata().await {
                if metadata.is_dir() {
                    stack.push(path);
                } else {
                    count += 1;
                    bytes += metadata.len();
                }
            }
        }
    }
    (count, bytes)
}

#[tauri::command]
pub async fn get_downloads_info() -> Result<DownloadsInfo, String> {
    let (file_count, total_bytes) = dir_stats(&downloads_base_dir()).await;
    Ok(DownloadsInfo { file_count, total_bytes })
}

#[tauri::command]
pub async fn clear_downloads() -> Result<String, String> {
    let base = downloads_base_dir();
    let (file_count, total_bytes) = dir_stats(&base).await;
    if file_count == 0 {
        return Ok("No downloads to clear.".to_string());
    }
    tokio::fs::remove_dir_all(&base)
        .await
        .map_err(|e| format!("Failed to clear downloads: {}", e))?;
    Ok(format!("Cleared {} file(s), {}.", file_count, format_size(&total_bytes.to_string())))
}
