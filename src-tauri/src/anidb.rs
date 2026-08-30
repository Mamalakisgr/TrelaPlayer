use crate::models::{Episode, ResolvedAnime};
use crate::procutil::find_exe;
use std::process::Command;

// ani-cli itself searches hianime.at (as of ani-cli 5.1.1 — it has already
// switched providers twice before this: AllAnime -> anidb.app -> hianime.at,
// so this module's URL/parsing is tied to whatever ani-cli's own upstream
// script currently does, not a stable API contract; check
// https://github.com/pystardust/ani-cli/blob/master/ani-cli's search_api /
// episodes_api if this starts erroring again). AniList — used for all of
// trela's own anime browsing/metadata — models each season/cour of a show as
// its own entry with its own 1..N episode count, but the scraped provider
// doesn't always agree: the same show can be a single listing with GLOBAL
// episode numbers across every season (season 6 episode 1 is really "episode
// 111"), or split into multiple overlapping listings where a blind top
// search match lands on the wrong one entirely. Trusting ani-cli's own `-S 1`
// on an AniList title without checking either of those is exactly what
// causes "wrong show" or "right show, wrong episode" — this exists purely to
// catch that at watch time, one lookup per title, right before actually
// invoking ani-cli. It is NOT a general search/browse backend; Browse/Home/
// search stay AniList-only.
const BASE_URL: &str = "https://hianime.at";

fn curl_impersonate_path() -> String {
    find_exe(&["curl-impersonate.exe", "curl-impersonate"]).unwrap_or_else(|| "curl-impersonate.exe".to_string())
}

// The scraped provider can sit behind a Cloudflare managed challenge that
// blocks plain HTTP clients outright — this is the same reason ani-cli itself
// carries a curl-impersonate fallback (see its dep_ch_failover
// "curl_firefox135,curl_chrome136,curl_chrome116,..." list) for whichever
// site it's currently pointed at. Same cipher/header/HTTP2 fingerprint as
// ani-cli's own curl_chrome116 wrapper — a generic real-Chrome fingerprint,
// not tied to any one site, so it keeps working across ani-cli's provider
// switches without needing to change.
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
            "-w",
            "\n%{http_code}",
            url,
        ])
        .output()
        .map_err(|e| {
            format!(
                "Failed to run curl-impersonate ({}): {}. {} (what ani-cli itself searches) requires \
                 curl-impersonate to get past Cloudflare — the same tool ani-cli needs for this exact reason.",
                path, e, BASE_URL
            )
        })?;

    if !output.status.success() {
        return Err(format!("curl-impersonate exited with status {}", output.status));
    }
    // curl (no -f/--fail passed) exits 0 even on a 5xx/4xx HTTP response —
    // verified directly against anidb.app while it was down for maintenance
    // (same failure mode applies to whatever site BASE_URL currently points
    // at): curl's own process succeeds, and the maintenance page's HTML used
    // to get parsed as a normal (empty) search result, silently misreporting
    // a site outage as "no match found". -w appends the real status code
    // after the body so that case can be told apart from a genuine empty
    // result.
    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let (body, status) = raw.rsplit_once('\n').unwrap_or((raw.as_str(), ""));
    if body.contains("Just a moment") {
        return Err(format!("Blocked by Cloudflare ({}). Try updating curl-impersonate.", BASE_URL));
    }
    if !status.trim().starts_with('2') {
        return Err(format!(
            "{} returned HTTP {} — it looks like it's down or under maintenance right now, not that the title doesn't exist. Try again later.",
            BASE_URL, status.trim()
        ));
    }
    Ok(body.to_string())
}

fn html_unescape(s: &str) -> String {
    s.replace("&#039;", "'").replace("&quot;", "\"").replace("&amp;", "&")
}

// Search result cards look like:
// <div class="film-detail">...<h3 class="film-name"><a href="/watch/<slug>-<id>" title="<Title>">...
// (mirrors ani-cli's own hianime_search: split on film-detail blocks, then
// within each block require the <a href> to be the one immediately following
// <h3 class="film-name">, matching the plain string-scanning approach already
// used here rather than pulling in a full HTML parser for one pattern.)
fn parse_search_results(html: &str) -> Vec<(String, String)> {
    // Everything from the sidebar onward is unrelated page furniture, not
    // search results — mirrors ani-cli's own `sed '/id="main-sidebar"/,$d'`.
    let html = html.split("id=\"main-sidebar\"").next().unwrap_or(html);
    let mut results = Vec::new();
    for chunk in html.split("<div class=\"film-detail\">").skip(1) {
        let Some(after_h3) = chunk.split_once("<h3 class=\"film-name\">").map(|(_, rest)| rest) else { continue };
        let Some(after_href_marker) = after_h3.split_once("<a href=\"").map(|(_, rest)| rest) else { continue };
        let Some((href, after_href)) = after_href_marker.split_once('"') else { continue };
        // The trailing path segment (after the last '/') is the anime "id"
        // ani-cli itself threads through to the episodes API and the
        // eventual `/watch/<id>?ep=...` URLs — e.g. "one-piece-100".
        let Some(slug_id) = href.rsplit('/').next().filter(|s| !s.is_empty()) else { continue };
        let Some(after_title_marker) = after_href.split_once("title=\"").map(|(_, rest)| rest) else { continue };
        let Some((title, _)) = after_title_marker.split_once('"') else { continue };
        results.push((slug_id.to_string(), html_unescape(title)));
    }
    results
}

fn search_anime(query: &str) -> Result<Vec<(String, String)>, String> {
    let normalized_query = query.trim().replace(' ', "+");
    if normalized_query.is_empty() {
        return Ok(Vec::new());
    }
    let html = curl_impersonate_get(&format!("{}/search?keyword={}", BASE_URL, normalized_query))?;
    Ok(parse_search_results(&html))
}

// The response is JSON with the episode list markup embedded as an escaped
// string field. Dropping every backslash turns \" and \/ back into plain "
// and / (same as ani-cli's own `sed 's|\\||g'`), leaving ordinary
// <a class="ep-item" ... data-number="N" ...> markup that the same plain
// string-scanning approach as parse_search_results can read — only "number"
// is needed here, episode ids play no role in Trela's own playback path
// (that's ani-cli's job, by real episode number). Verified against a live
// 1177-episode response (One Piece) — every ep-item's data-number comes
// through with no gaps or duplicates.
fn parse_episode_html(raw: &str) -> Vec<Episode> {
    let unescaped = raw.replace('\\', "");
    let mut episodes = Vec::new();
    for chunk in unescaped.split("ep-item").skip(1) {
        let Some(after_marker) = chunk.split_once("data-number=\"").map(|(_, rest)| rest) else { continue };
        let Some((number, _)) = after_marker.split_once('"') else { continue };
        episodes.push(Episode { title: format!("Episode {}", number), number: number.to_string() });
    }
    episodes
}

fn get_episodes(anime_id: &str) -> Result<Vec<Episode>, String> {
    // anime_id is the full "<slug>-<numericId>" string search_anime returned;
    // the episodes API wants just the trailing numeric id.
    let numeric_id = anime_id.rsplit('-').next().unwrap_or(anime_id);
    let raw = curl_impersonate_get(&format!("{}/api/theme/episode/list/{}", BASE_URL, numeric_id))?;
    Ok(parse_episode_html(&raw))
}

fn max_episode_number(episodes: &[Episode]) -> u64 {
    episodes.iter().filter_map(|e| e.number.parse::<u64>().ok()).max().unwrap_or(0)
}

// A romaji arc/season subtitle from AniList doesn't necessarily textually
// resemble the scraped provider's English title for the same entry at all,
// so search relevance alone can't be trusted to put the right season/cour
// first. The AniList-reported episode count is a much more reliable
// disambiguator — each *finished* season/cour usually has a distinct, stable
// count — so this tries candidates in relevance order and prefers whichever
// one's real episode list actually matches that count.
//
// That exact match only works once a season is done airing, though: for a
// RELEASING (still-airing) entry, AniList reports the season's planned total
// episode count, not how many have actually aired yet — e.g. Bleach's
// "The Calamity" cour was listed as 10 planned episodes on AniList while the
// provider only had the 3 that had aired so far (numbered 41-43, continuing
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

#[cfg(test)]
mod hianime_parsing_tests {
    // Fixtures below are hand-trimmed excerpts modeled on real hianime.at
    // responses (fetched and verified against these exact parsers, including
    // a full 1177-episode live response, while porting this module off
    // anidb.app). Kept small/embedded so the tests are self-contained rather
    // than depending on live network access or files outside the repo.
    use super::*;

    const SEARCH_FIXTURE: &str = r#"
        <div class="film-detail">
            <h3 class="film-name">
                <a href="https://hianime.at/one-piece-1"
                    title="One Piece"
                    class="dynamic-name"
                    data-jname="One Piece">
                    One Piece
                </a>
            </h3>
            <div class="description">Gold Roger was known as the &quot;Pirate King&quot;...</div>
        </div>
        <div class="film-detail">
            <h3 class="film-name">
                <a href="https://hianime.at/one-piece-the-movie-4164"
                    title="One Piece Movie 1"
                    class="dynamic-name"
                    data-jname="One Piece Movie 1">
                    One Piece Movie 1
                </a>
            </h3>
        </div>
        <div id="main-sidebar">
            <div class="film-detail">
                <h3 class="film-name">
                    <a href="https://hianime.at/some-recommended-show-9"
                        title="Should Be Excluded">Should Be Excluded</a>
                </h3>
            </div>
        </div>
    "#;

    #[test]
    fn parses_search_cards_and_stops_at_sidebar() {
        let results = parse_search_results(SEARCH_FIXTURE);
        assert_eq!(
            results,
            vec![
                ("one-piece-1".to_string(), "One Piece".to_string()),
                ("one-piece-the-movie-4164".to_string(), "One Piece Movie 1".to_string()),
            ]
        );
    }

    #[test]
    fn episode_html_extracts_numbers_from_json_escaped_markup() {
        // Mirrors the real /api/theme/episode/list/<id> shape: JSON with the
        // episode markup embedded as an escaped "html" string field, real
        // attribute order (title, class="... ep-item", data-number, data-id,
        // href) confirmed against a live 1177-episode response.
        let raw = r#"{"status":true,"html":"<a title=\"Episode 1\" class=\"ssl-item ep-item\" data-number=\"1\" data-id=\"1\" href=\"\/watch\/one-piece-1?ep=1\"><\/a><a title=\"Episode 2\" class=\"ssl-item ep-item\" data-number=\"2\" data-id=\"2\" href=\"\/watch\/one-piece-1?ep=2\"><\/a>"}"#;
        let episodes = parse_episode_html(raw);
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].number, "1");
        assert_eq!(episodes[1].number, "2");
    }
}
