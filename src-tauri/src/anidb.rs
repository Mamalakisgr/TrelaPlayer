use crate::models::{Episode, ResolvedAnime};
use crate::procutil::find_exe;
use std::process::Command;

// ani-cli itself searches anidb.app (it switched providers upstream from
// AllAnime). AniList — used for all of trela's own anime browsing/metadata —
// models each season/cour of a show as its own entry with its own 1..N
// episode count, but anidb.app doesn't always agree: the same show can be a
// single listing with GLOBAL episode numbers across every season (season 6
// episode 1 is really "episode 111"), or split into multiple overlapping
// listings where a blind top search match lands on the wrong one entirely.
// Trusting ani-cli's own `-S 1` on an AniList title without checking either
// of those is exactly what causes "wrong show" or "right show, wrong
// episode" — this exists purely to catch that at watch time, one lookup per
// title, right before actually invoking ani-cli. It is NOT a general
// search/browse backend; Browse/Home/search stay AniList-only.
const BASE_URL: &str = "https://anidb.app";

fn curl_impersonate_path() -> String {
    find_exe(&["curl-impersonate.exe", "curl-impersonate"]).unwrap_or_else(|| "curl-impersonate.exe".to_string())
}

// anidb.app sits behind a Cloudflare managed challenge that blocks plain HTTP
// clients outright — this is exactly why ani-cli itself requires
// curl-impersonate (see its dep_ch_failover "curl_firefox135,curl_chrome136,
// curl_chrome116,..." list). Same cipher/header/HTTP2 fingerprint as ani-cli's
// own curl_chrome116 wrapper, verified working directly against anidb.app.
fn curl_impersonate_get(url: &str) -> Result<String, String> {
    let path = curl_impersonate_path();
    let output = Command::new(&path)
        .args([
            "--ciphers",
            "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:ECDHE-RSA-AES128-SHA:ECDHE-RSA-AES256-SHA:AES128-GCM-SHA256:AES256-GCM-SHA384:AES128-SHA:AES256-SHA",
            "-H",
            "sec-ch-ua: \"Chromium\";v=\"116\", \"Not)A;Brand\";v=\"24\", \"Google Chrome\";v=\"116\"",
            "-H",
            "sec-ch-ua-mobile: ?0",
            "-H",
            "sec-ch-ua-platform: \"Windows\"",
            "-H",
            "Upgrade-Insecure-Requests: 1",
            "-H",
            "User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Safari/537.36",
            "-H",
            "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
            "-H",
            "Sec-Fetch-Site: none",
            "-H",
            "Sec-Fetch-Mode: navigate",
            "-H",
            "Sec-Fetch-User: ?1",
            "-H",
            "Sec-Fetch-Dest: document",
            "-H",
            "Accept-Language: en-US,en;q=0.9",
            "--http2",
            "--http2-settings",
            "1:65536;2:0;3:1000;4:6291456;6:262144",
            "--http2-window-update",
            "15663105",
            "--http2-stream-weight",
            "256",
            "--http2-stream-exclusive",
            "1",
            "--compressed",
            "--tlsv1.2",
            "--alps",
            "--tls-permute-extensions",
            "--cert-compression",
            "brotli",
            "--tls-grease",
            "--tls-signed-cert-timestamps",
            "-sL",
            url,
        ])
        .output()
        .map_err(|e| {
            format!(
                "Failed to run curl-impersonate ({}): {}. anidb.app (what ani-cli itself searches) requires \
                 curl-impersonate to get past Cloudflare — the same tool ani-cli needs for this exact reason.",
                path, e
            )
        })?;

    if !output.status.success() {
        return Err(format!("curl-impersonate exited with status {}", output.status));
    }
    let body = String::from_utf8_lossy(&output.stdout).into_owned();
    if body.contains("Just a moment") {
        return Err("Blocked by Cloudflare (anidb.app). Try updating curl-impersonate.".to_string());
    }
    Ok(body)
}

fn html_unescape(s: &str) -> String {
    s.replace("&#039;", "'").replace("&quot;", "\"").replace("&amp;", "&")
}

// Anime cards look like:
// <a href="https://anidb.app/anime/<slug>-<id>" class="anime-card ..." title="<Title>">
// Extracted with plain string scanning (matching ani-cli's own sed-based
// approach) rather than pulling in a full HTML parser for one pattern.
fn parse_search_results(html: &str) -> Vec<(String, String)> {
    let mut results = Vec::new();
    for chunk in html.split("<a href=\"").skip(1) {
        let Some(url_end) = chunk.find('"') else { continue };
        let href = &chunk[..url_end];
        let Some(slug_id) = href.rsplit("/anime/").next() else { continue };
        if slug_id == href {
            continue; // no "/anime/" in this href — not an anime card
        }
        let Some(title_start) = chunk.find("title=\"").map(|i| i + "title=\"".len()) else { continue };
        let Some(title_len) = chunk[title_start..].find('"') else { continue };
        let title = html_unescape(&chunk[title_start..title_start + title_len]);
        results.push((slug_id.to_string(), title));
    }
    results
}

fn search_anime(query: &str) -> Result<Vec<(String, String)>, String> {
    let normalized_query = query.trim().replace(' ', "+");
    if normalized_query.is_empty() {
        return Ok(Vec::new());
    }
    let html = curl_impersonate_get(&format!("{}/browse?q={}", BASE_URL, normalized_query))?;
    Ok(parse_search_results(&html))
}

fn get_episodes(anime_id: &str) -> Result<Vec<Episode>, String> {
    // anime_id is the full "<slug>-<numericId>" string search_anime returned;
    // the episodes API wants just the trailing numeric id.
    let numeric_id = anime_id.rsplit('-').next().unwrap_or(anime_id);
    let json = curl_impersonate_get(&format!("{}/api/frontend/anime/{}/episodes", BASE_URL, numeric_id))?;
    let parsed: serde_json::Value = serde_json::from_str(&json).map_err(|e| format!("Failed to parse episode list: {}", e))?;

    Ok(parsed["episodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            let number = e["number"].as_u64()?.to_string();
            Some(Episode { title: format!("Episode {}", number), number })
        })
        .collect())
}

fn max_episode_number(episodes: &[Episode]) -> u64 {
    episodes.iter().filter_map(|e| e.number.parse::<u64>().ok()).max().unwrap_or(0)
}

// A romaji arc/season subtitle from AniList doesn't necessarily textually
// resemble anidb.app's English title for the same entry at all, so search
// relevance alone can't be trusted to put the right season/cour first. The
// AniList-reported episode count is a much more reliable disambiguator —
// each *finished* season/cour usually has a distinct, stable count — so this
// tries candidates in relevance order and prefers whichever one's real
// episode list actually matches that count.
//
// That exact match only works once a season is done airing, though: for a
// RELEASING (still-airing) entry, AniList reports the season's planned total
// episode count, not how many have actually aired yet — e.g. Bleach's
// "The Calamity" cour was listed as 10 planned episodes on AniList while
// anidb.app only had the 3 that had aired so far (numbered 41-43, continuing
// the show's global numbering from the previous cour). No count will ever
// exactly match mid-season, so for RELEASING titles the tiebreaker instead
// is which candidate's episodes are the most recent continuation of the
// story — the one with the highest real episode numbers — since a newly
// aired cour is, by construction, always later than earlier ones.
// Runs on a blocking-pool thread (see the async wrapper below) since every
// step here is a synchronous curl-impersonate subprocess call.
fn resolve_anime_blocking(title: String, expected_episodes: Option<u32>, is_releasing: bool) -> Result<Option<ResolvedAnime>, String> {
    let normalized_query = title.trim().replace(' ', "+");
    let candidates = search_anime(&title)?;
    if candidates.is_empty() {
        return Ok(None);
    }

    let Some(expected) = expected_episodes.filter(|&e| e > 0) else {
        let (id, _) = &candidates[0];
        let episodes = get_episodes(id).unwrap_or_default();
        return Ok(Some(ResolvedAnime { query: normalized_query, index: 1, episodes }));
    };

    // Each candidate's episode list is its own curl-impersonate process +
    // Cloudflare-gated round trip — the latency is set by that gate, not by
    // us, so fetching all (up to 6) candidates in parallel instead of one at
    // a time is the only lever available to cut this down.
    let take = candidates.len().min(6);
    let results: Vec<(u32, Vec<Episode>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = candidates[..take]
            .iter()
            .enumerate()
            .map(|(i, (id, _))| scope.spawn(move || ((i + 1) as u32, get_episodes(id).unwrap_or_default())))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| (0, Vec::new()))).collect()
    });

    // Preserve the original in-order preference: first candidate with an
    // exact count match; otherwise, among non-empty candidates, the first
    // one with the highest real episode numbers on a tie.
    if let Some((index, episodes)) = results.iter().find(|(_, eps)| eps.len() as u32 == expected) {
        return Ok(Some(ResolvedAnime { query: normalized_query, index: *index, episodes: episodes.clone() }));
    }

    let fallback = results.first().cloned();
    let mut latest: Option<(u32, Vec<Episode>)> = None;
    for (index, episodes) in &results {
        if !episodes.is_empty() && max_episode_number(episodes) > latest.as_ref().map(|(_, e)| max_episode_number(e)).unwrap_or(0) {
            latest = Some((*index, episodes.clone()));
        }
    }

    let chosen = if is_releasing { latest.or(fallback) } else { fallback };
    Ok(chosen.map(|(index, episodes)| ResolvedAnime { query: normalized_query, index, episodes }))
}

#[tauri::command]
pub async fn resolve_anime(title: String, expected_episodes: Option<u32>, is_releasing: bool) -> Result<Option<ResolvedAnime>, String> {
    tokio::task::spawn_blocking(move || resolve_anime_blocking(title, expected_episodes, is_releasing))
        .await
        .map_err(|e| format!("Internal error resolving anime: {}", e))?
}
