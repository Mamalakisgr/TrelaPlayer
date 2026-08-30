use crate::models::StreamOption;
use crate::movplayer::extract_stream_options;
use moviebox_tui::providers::fourkhdhub::{releases_to_moviebox_json, FourKHdHubClient};
use moviebox_tui::providers::models::{CatalogItem, MediaType, Release};
use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;

// Deliberately NOT cached as a long-lived singleton (unlike MovieBoxClient,
// which is worth caching because its .init() does a real auth handshake).
// FourKHdHubClient::new() is a plain, free constructor, and a client that
// sits alive for the whole app session pools/reuses TCP connections that can
// go stale (server closes it, a router/NAT drops an idle connection) —
// reusing a stale pooled connection to hubcloud/hubdrive can fail
// consistently for the rest of that session even though the same request
// from a fresh client succeeds immediately. A fresh client per call costs
// nothing here and sidesteps that entirely.
// As of moviebox-tui 0.1.14, new() is fallible (it wasn't in 0.1.8).
fn get_client() -> Result<FourKHdHubClient, String> {
    FourKHdHubClient::new().map_err(|e| format!("Failed to create 4KHDHub client: {:?}", e))
}

// CatalogItem.year is a plain year string (e.g. "1994"), unlike MovieBox's
// full releaseDate — parsed the same way for the same reason: an exact
// title match is common across unrelated shows sharing a name (e.g. more
// than one cartoon plainly titled "Spider-Man"), while a year mismatch (when
// both sides actually know a year) reliably points at the wrong show.
fn item_year(item: &CatalogItem) -> Option<i32> {
    item.year.as_deref()?.split('-').next()?.parse().ok()
}

fn rank_item(item: &CatalogItem, title: &str, expected_year: Option<u32>) -> (u8, u8) {
    let year_rank = match (expected_year, item_year(item)) {
        (Some(e), Some(a)) if (a - e as i32).abs() <= 1 => 0,
        (Some(_), Some(_)) => 2,
        _ => 1,
    };
    let title_rank = if item.title == title { 0 } else { 1 };
    (year_rank, title_rank)
}

fn normalize_words(s: &str) -> std::collections::HashSet<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).map(str::to_string).collect()
}

// 4KHDHub's own search does loose fuzzy text matching and, when it has
// nothing genuinely related, still returns whatever vaguely resembles the
// query string instead of an empty list — e.g. searching "Eastbound and
// Down" returns exactly one result, "Dekin no Mogura: The Earthbound Mole",
// an unrelated Japanese series that only superficially resembles "Eastbound"
// / "Earthbound". Blindly taking that "best available" candidate is exactly
// what silently played the wrong show. This requires a candidate to share at
// least half of the query's significant words (case/punctuation-insensitive)
// before it's considered a real match at all — loose enough to survive a
// colon/subtitle difference ("Spider-Man" vs "Spider-Man: The Animated
// Series"), tight enough to reject results that just don't relate.
fn title_plausibly_matches(candidate_title: &str, query_title: &str) -> bool {
    let query_words = normalize_words(query_title);
    if query_words.is_empty() {
        return true;
    }
    let candidate_words = normalize_words(candidate_title);
    query_words.intersection(&candidate_words).count() * 2 >= query_words.len()
}

// Mirrors search_candidates in movplayer.rs: a bare-title search only
// returns 4KHDHub's own single page of "most relevant" results, which can
// bury (or omit) the right listing when a title has been reused across
// unrelated shows — a year-qualified query re-ranks their own search
// instead. Only fired when the plain search doesn't already have a
// year-matching candidate, and pooled (deduped by id) rather than replacing
// the original results.
async fn search_candidates(client: &FourKHdHubClient, title: &str, expected_year: Option<u32>) -> Result<Vec<CatalogItem>, String> {
    let mut items = client.search(title).await.map_err(|e| format!("Search failed: {:?}", e))?;

    if let Some(year) = expected_year {
        let has_year_match = items.iter().any(|i| matches!(item_year(i), Some(a) if (a - year as i32).abs() <= 1));
        if !has_year_match {
            let year_query = format!("{} {}", title, year);
            if let Ok(more) = client.search(&year_query).await {
                let mut seen: std::collections::HashSet<String> = items.iter().map(|i| i.id.value.clone()).collect();
                items.extend(more.into_iter().filter(|item| seen.insert(item.id.value.clone())));
            }
        }
    }

    Ok(items)
}

// 4KHDHub can list the same title more than once too (e.g. different shows
// sharing a name, or a listing that turns out not to actually carry the
// requested season/episode) — previously this took the first exact-title
// match (or just the first result) and gave up if it had no releases for
// the requested episode. Now it ranks candidates the same way MovieBox's
// find_playable_subject does (year first, then title) and tries them in
// that order until one actually has releases — but unlike MovieBox, a
// candidate first has to clear three gates before it's even a candidate:
// the right media type (a season/episode request can't be satisfied by a
// Movie-typed entry), a plausible title (see title_plausibly_matches), and —
// when both sides know a year — a year that isn't a confident mismatch.
// MovieBox still treats a wrong year as a soft, last-resort fallback; here
// it's a hard exclusion, because 4KHDHub's fuzzy search has already been
// seen returning a completely unrelated show as its only result rather than
// an empty list (see title_plausibly_matches), so "the best of what's left"
// isn't a safe assumption the way it is on MovieBox.
async fn find_playable_release(
    client: &FourKHdHubClient,
    title: &str,
    season: usize,
    episode: usize,
    expected_year: Option<u32>,
) -> Result<(String, Vec<Release>), String> {
    let results = search_candidates(client, title, expected_year).await?;

    let wanted_type = if season > 0 || episode > 0 { MediaType::Series } else { MediaType::Movie };
    let mut ranked: Vec<&CatalogItem> = results
        .iter()
        .filter(|i| {
            i.media_type == wanted_type
                && title_plausibly_matches(&i.title, title)
                && !matches!((expected_year, item_year(i)), (Some(e), Some(a)) if (a - e as i32).abs() > 1)
        })
        .collect();
    ranked.sort_by_key(|i| rank_item(i, title, expected_year));

    let mut fallback: Option<(String, Vec<Release>)> = None;
    for item in ranked.into_iter().take(8) {
        let Ok(releases) = client.releases(&item.id.value, season, episode).await else { continue };
        if !releases.is_empty() {
            return Ok((item.id.value.clone(), releases));
        }
        if fallback.is_none() {
            fallback = Some((item.id.value.clone(), releases));
        }
    }

    fallback.ok_or_else(|| format!("4KHDHub doesn't have a matching source for \"{}\" — try switching to MovieBox.", title))
}

// moviebox-tui's own releases_to_moviebox_json() converts 4KHDHub releases
// into the exact same resource-list JSON shape MovieBox's API returns, so
// this reuses extract_stream_options from movplayer.rs instead of a second
// parallel parser — the two providers only really differ in how a chosen
// option is actually resolved to a playable URL (see play_fourk_stream).
#[tauri::command]
pub async fn get_fourk_stream_options(title: String, season: u32, episode: u32, year: Option<u32>) -> Result<Vec<StreamOption>, String> {
    let client = get_client()?;
    let (_, releases) = find_playable_release(&client, &title, season as usize, episode as usize, year).await?;

    let json = releases_to_moviebox_json(&releases);
    let list = json.as_array().cloned().unwrap_or_default();
    let options = extract_stream_options("", &list);
    if options.is_empty() {
        return Err("No playable source found for this title/episode.".to_string());
    }
    Ok(options)
}

// resolve_release swallows every per-mirror error internally and only ever
// reports the generic NoPlayableMirror, which isn't enough to tell "the
// mirror is genuinely down" apart from "something on this machine (firewall,
// antivirus, proxy) is blocking hubcloud/hubdrive specifically" — the two
// need very different fixes. This makes one plain, unrelated HTTP request
// straight to the first mirror to surface whatever the real underlying
// failure actually is (timeout, connection refused, TLS failure, DNS, or a
// real HTTP status) alongside the final error.
async fn diagnose_unreachable(url: &str) -> String {
    match reqwest::Client::new().get(url).timeout(std::time::Duration::from_secs(10)).send().await {
        Ok(resp) => format!("direct probe to {} got HTTP {} (so it's reachable — the failure is deeper inside 4KHDHub's own redirect/resolution chain)", url, resp.status()),
        Err(e) => format!("direct probe to {} failed: {} (this machine likely can't reach that host at all right now)", url, e),
    }
}

// Unlike MovieBox's resourceLink (already a direct video URL), a 4KHDHub
// release's link is a mirror/hosting page — resolve_release follows it
// (through hubcloud/hubdrive if needed) to find the real playable URL,
// which sometimes also needs specific headers to be accepted by the CDN.
//
// hubcloud.ist/hubdrive.tips are third-party file hosts, not something this
// app controls, and resolve_release doesn't retry — it tries a release's two
// mirrors once each and gives up. Those mirrors going down is common enough
// in practice that the fallback list matters: `releases` is the user's
// chosen quality first, followed by the other quality/codec options for the
// same episode, each with its own independent pair of mirrors, so one
// release's mirrors being temporarily down doesn't have to be a dead end.
#[tauri::command]
pub async fn play_fourk_stream(app: AppHandle, releases: Vec<Release>, window_title: String) -> Result<String, String> {
    if releases.is_empty() {
        return Err("No source selected".to_string());
    }

    let client = get_client()?;
    let mut last_err = String::new();
    let mut resolved = None;
    for release in &releases {
        match client.resolve_release(release).await {
            Ok(source) => {
                resolved = Some(source);
                break;
            }
            Err(e) => last_err = format!("{:?}", e),
        }
    }
    let source = match resolved {
        Some(source) => source,
        None => {
            let diagnostic = match releases.first().and_then(|r| r.mirrors.first()) {
                Some(mirror) => diagnose_unreachable(&mirror.resolver_url).await,
                None => "no mirrors to probe".to_string(),
            };
            return Err(format!(
                "Failed to resolve a playable link ({} option(s) tried): {}. {}",
                releases.len(),
                last_err,
                diagnostic
            ));
        }
    };

    // mpv runs as a bundled sidecar (see movplayer.rs's spawn_mpv for why —
    // same reasoning applies to this provider).
    let mut sidecar = app.shell().sidecar("mpv").map_err(|e| format!("Failed to resolve bundled mpv: {}", e))?;
    sidecar = sidecar.arg(format!("--force-media-title={}", window_title));
    // See spawn_mpv in movplayer.rs for why: mpv's default demuxer cache is
    // sized for local files, not a multi-GB remote episode through a
    // third-party mirror, and underruns (mpv pauses on those by default)
    // show up as "slow"/stuttery playback.
    sidecar = sidecar.args(["--cache=yes", "--demuxer-max-bytes=500MiB", "--demuxer-max-back-bytes=150MiB"]);
    if !source.headers.is_empty() {
        let header_str = source.headers.iter().map(|(k, v)| format!("{}: {}", k, v)).collect::<Vec<_>>().join(",");
        sidecar = sidecar.arg(format!("--http-header-fields={}", header_str));
    }
    if let Some(sub) = &source.subtitle {
        sidecar = sidecar.arg(format!("--sub-file={}", sub));
    }
    sidecar = sidecar.arg(&source.url);

    let (mut rx, _child) = sidecar.spawn().map_err(|e| format!("Failed to launch mpv: {}", e))?;
    tauri::async_runtime::spawn(async move { while rx.recv().await.is_some() {} });

    Ok(format!("Playing \"{}\"", window_title))
}
