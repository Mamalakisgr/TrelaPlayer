use crate::models::{RecentEpisode, SeasonalAnime, SeasonYear};
use crate::procutil::{http_client, json_with_retry, random_below};
use serde_json::json;

// AniList is a free GraphQL API backed by its own anime database rather than
// a proxy over MyAnimeList's site, so it doesn't inherit MAL's frequent
// upstream 502/503/504s the way Jikan did. Single POST endpoint, ~90 req/min.
const ANILIST_API: &str = "https://graphql.anilist.co";

const MEDIA_FIELDS: &str = "
    id
    title { romaji english }
    coverImage { extraLarge large }
    episodes
    averageScore
    status
    genres
    description(asHtml: false)
    nextAiringEpisode { airingAt episode }
    startDate { year month day }
";

async fn anilist_query(query: &str, variables: serde_json::Value) -> Result<serde_json::Value, String> {
    let client = http_client();
    let body = json!({ "query": query, "variables": variables });

    json_with_retry("AniList", || client.post(ANILIST_API).json(&body).send()).await
}

fn humanize_status(status: &str) -> String {
    match status {
        "RELEASING" => "Currently airing",
        "FINISHED" => "Finished airing",
        "NOT_YET_RELEASED" => "Not yet aired",
        "CANCELLED" => "Cancelled",
        "HIATUS" => "On hiatus",
        other => return other.to_string(),
    }
    .to_string()
}

// AniList's `asHtml: false` still leaves stray <br> tags in some entries;
// strip any markup rather than pulling in a regex crate for this alone.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

// (year, month, day) -> days-since-epoch, Howard Hinnant's days_from_civil —
// the inverse of current_year_month_utc's algorithm below, kept hand-rolled
// for the same reason: one date conversion doesn't justify a date crate.
fn epoch_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400
}

// AniList only fills nextAiringEpisode once a show has a real broadcast
// schedule. Upcoming titles often have just an announced premiere date, and
// the furthest-out ones only a year ("2027") — which is nothing to count
// down to, so those get None rather than a misleading Jan 1st.
fn parse_next_airing(item: &serde_json::Value) -> (Option<i64>, Option<u64>) {
    if let Some(airing_at) = item["nextAiringEpisode"]["airingAt"].as_i64() {
        return (Some(airing_at), item["nextAiringEpisode"]["episode"].as_u64());
    }
    let start = &item["startDate"];
    match (start["year"].as_i64(), start["month"].as_i64(), start["day"].as_i64()) {
        (Some(y), Some(m), Some(d)) => (Some(epoch_from_civil(y, m, d)), None),
        _ => (None, None),
    }
}

fn parse_media_item(item: &serde_json::Value) -> Option<SeasonalAnime> {
    let id = item["id"].as_u64()?;
    let title = item["title"]["romaji"].as_str().or_else(|| item["title"]["english"].as_str())?.to_string();
    let image_url = item["coverImage"]["extraLarge"]
        .as_str()
        .or_else(|| item["coverImage"]["large"].as_str())
        .unwrap_or("")
        .to_string();
    let genres = item["genres"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|g| g.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let (next_airing_at, next_episode) = parse_next_airing(item);
    Some(SeasonalAnime {
        id,
        title,
        image_url,
        episodes: item["episodes"].as_u64(),
        // AniList scores anime 0-100; MAL-style display expects 0-10.
        score: item["averageScore"].as_f64().map(|s| s / 10.0),
        status: item["status"].as_str().map(humanize_status).unwrap_or_default(),
        genres,
        synopsis: item["description"].as_str().map(strip_tags),
        next_airing_at,
        next_episode,
    })
}

fn parse_media_list(data: &serde_json::Value) -> Vec<SeasonalAnime> {
    data["data"]["Page"]["media"].as_array().cloned().unwrap_or_default().iter().filter_map(parse_media_item).collect()
}

async fn fetch_media_list(query: &str, variables: serde_json::Value) -> Result<Vec<SeasonalAnime>, String> {
    let data = anilist_query(query, variables).await?;
    Ok(parse_media_list(&data))
}

fn season_query() -> String {
    format!(
        "query ($season: MediaSeason, $year: Int, $genre: [String]) {{
            Page(page: 1, perPage: 24) {{
                media(type: ANIME, season: $season, seasonYear: $year, genre_in: $genre, sort: POPULARITY_DESC) {{ {fields} }}
            }}
        }}",
        fields = MEDIA_FIELDS
    )
}

#[tauri::command]
pub async fn get_seasonal_anime(genre: Option<String>) -> Result<Vec<SeasonalAnime>, String> {
    let (year, month) = current_year_month_utc();
    let season = SEASONS[((month - 1) / 3) as usize];
    fetch_media_list(&season_query(), json!({ "season": season, "year": year, "genre": genre.map(|g| vec![g]) })).await
}

#[tauri::command]
pub async fn get_seasonal_anime_by(year: u32, season: &str, genre: Option<String>) -> Result<Vec<SeasonalAnime>, String> {
    fetch_media_list(
        &season_query(),
        json!({ "season": season.to_uppercase(), "year": year, "genre": genre.map(|g| vec![g]) }),
    )
    .await
}

// AniList has no dedicated "random anime" endpoint, so this fakes one: ask
// for the total number of anime matching the filter (perPage: 1 just to keep
// that probe cheap), then re-request a single random page within that count.
// Capping the pool keeps the pick recognizable rather than deep obscure
// search noise, and — unlike the Spotlight's old seasonal-only pick — draws
// from AniList's whole popularity-ranked catalog, not just the current season.
#[tauri::command]
pub async fn get_random_anime(genre: Option<String>) -> Result<Option<SeasonalAnime>, String> {
    const POOL_CAP: u64 = 500;
    let genre_list = genre.map(|g| vec![g]);
    let query = format!(
        "query ($genre: [String], $page: Int) {{
            Page(page: $page, perPage: 1) {{
                pageInfo {{ total }}
                media(type: ANIME, genre_in: $genre, sort: POPULARITY_DESC) {{ {fields} }}
            }}
        }}",
        fields = MEDIA_FIELDS
    );

    let probe = anilist_query(&query, json!({ "genre": genre_list, "page": 1 })).await?;
    let total = probe["data"]["Page"]["pageInfo"]["total"].as_u64().unwrap_or(0);
    if total == 0 {
        return Ok(None);
    }

    let page = 1 + random_below(total.min(POOL_CAP));
    let data = if page == 1 {
        probe
    } else {
        anilist_query(&query, json!({ "genre": genre_list, "page": page })).await?
    };
    Ok(parse_media_list(&data).into_iter().next())
}

// Static list of year/season combinations for the Browse season picker. AniList
// (unlike Jikan's /seasons) has no endpoint enumerating "seasons with data" —
// but main.js only ever shows the 7 most recent past seasons anyway, so a
// generated window covering the last few years is all the picker needs.
#[tauri::command]
pub fn get_season_list() -> Result<Vec<SeasonYear>, String> {
    let (current_year, _) = current_year_month_utc();
    Ok((current_year - 3..=current_year)
        .rev()
        .map(|year| SeasonYear {
            year: year as u32,
            seasons: vec!["winter".into(), "spring".into(), "summer".into(), "fall".into()],
        })
        .collect())
}

#[tauri::command]
pub async fn get_anime_info(title: &str) -> Result<Option<SeasonalAnime>, String> {
    let query = format!(
        "query ($search: String) {{
            Page(page: 1, perPage: 1) {{
                media(type: ANIME, search: $search) {{ {fields} }}
            }}
        }}",
        fields = MEDIA_FIELDS
    );
    Ok(fetch_media_list(&query, json!({ "search": title })).await?.into_iter().next())
}

// "Because you watched X" — AniList's own recommendation graph (user-submitted
// pairings, sorted by rating), keyed by the anime's numeric id since that's
// what Details already has once get_anime_info/get_seasonal_anime* resolved it.
#[tauri::command]
pub async fn get_anime_recommendations(id: u64) -> Result<Vec<SeasonalAnime>, String> {
    let query = format!(
        "query ($id: Int) {{
            Media(id: $id, type: ANIME) {{
                recommendations(sort: RATING_DESC, perPage: 12) {{
                    nodes {{ mediaRecommendation {{ {fields} }} }}
                }}
            }}
        }}",
        fields = MEDIA_FIELDS
    );
    let data = anilist_query(&query, json!({ "id": id })).await?;
    Ok(data["data"]["Media"]["recommendations"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|n| parse_media_item(&n["mediaRecommendation"]))
        .collect())
}

// AniList-only search for the command palette — this is metadata (title,
// synopsis, episode count), not a source of playable/searchable data for
// ani-cli. ani-cli does its own search when actually asked to play; trela
// doesn't try to independently replicate or verify that anymore.
#[tauri::command]
pub async fn search_anime(query: &str) -> Result<Vec<SeasonalAnime>, String> {
    let query_gql = format!(
        "query ($search: String) {{
            Page(page: 1, perPage: 8) {{
                media(type: ANIME, search: $search) {{ {fields} }}
            }}
        }}",
        fields = MEDIA_FIELDS
    );
    fetch_media_list(&query_gql, json!({ "search": query })).await
}

#[tauri::command]
pub async fn get_trending_anime() -> Result<Vec<SeasonalAnime>, String> {
    let query = format!(
        "query {{
            Page(page: 1, perPage: 10) {{
                media(type: ANIME, sort: TRENDING_DESC) {{ {fields} }}
            }}
        }}",
        fields = MEDIA_FIELDS
    );
    fetch_media_list(&query, json!({})).await
}

#[tauri::command]
pub async fn get_recent_episodes() -> Result<Vec<RecentEpisode>, String> {
    let query = "
        query ($now: Int) {
            Page(page: 1, perPage: 24) {
                airingSchedules(airingAt_lesser: $now, sort: TIME_DESC) {
                    episode
                    media {
                        id
                        title { romaji english }
                        coverImage { extraLarge large }
                    }
                }
            }
        }
    ";
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let data = anilist_query(query, json!({ "now": now })).await?;

    Ok(data["data"]["Page"]["airingSchedules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            let media = &item["media"];
            let id = media["id"].as_u64()?;
            let title = media["title"]["romaji"]
                .as_str()
                .or_else(|| media["title"]["english"].as_str())?
                .to_string();
            let image_url = media["coverImage"]["extraLarge"]
                .as_str()
                .or_else(|| media["coverImage"]["large"].as_str())
                .unwrap_or("")
                .to_string();
            let episode_title = format!("Episode {}", item["episode"].as_u64().unwrap_or(0));
            Some(RecentEpisode { id, title, image_url, episode_title })
        })
        .take(24)
        .collect())
}

// AniList's own genre list, fetched live rather than hardcoded — genre_in
// filters (get_seasonal_anime etc.) already accept any of these by name.
#[tauri::command]
pub async fn get_anime_genres() -> Result<Vec<String>, String> {
    let data = anilist_query("query { GenreCollection }", json!({})).await?;
    Ok(data["data"]["GenreCollection"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|g| g.as_str().map(str::to_string))
        .collect())
}

// ---------- Per-episode stills for the Watching strip ----------
//
// AniList has no episode-number field: numbers only exist inside
// streamingEpisodes' free-text titles, in Crunchyroll's "Episode 1 - Cruelty"
// shape. Worse, streamingEpisodes is franchise-level rather than entry-level,
// so a sequel's list is frequently the *prequel's* episodes — real images from
// the wrong season, which is worse than no image at all. Hence the trust rule
// in stills_if_complete: all-or-nothing per title.

fn parse_episode_number(title: &str) -> Option<u64> {
    let bytes = title.as_bytes();
    let start = bytes.iter().position(u8::is_ascii_digit)?;
    let end = start + bytes[start..].iter().take_while(|b| b.is_ascii_digit()).count();
    // "Episode 6.5 - Recap" is a recap sharing its integer part with a real
    // episode, so truncating to 6 would put the recap's image on episode 6.
    if bytes.get(end) == Some(&b'.') && matches!(bytes.get(end + 1), Some(c) if c.is_ascii_digit()) {
        return None;
    }
    title[start..end].parse().ok()
}

// Returns thumbnails, ordered by episode number, only when the parsed numbers
// are exactly the set {1..=episodes} with one thumbnailed entry each —
// otherwise the list belongs to some other entry in the franchise (or is
// partial), and every card would get a confidently wrong image. Length +
// range + no duplicates is that set check.
fn stills_if_complete(episodes: Option<u64>, streaming: &serde_json::Value) -> Vec<String> {
    let total = episodes.unwrap_or(0);
    let entries = streaming.as_array().map(Vec::as_slice).unwrap_or_default();
    if total == 0 || entries.len() as u64 != total {
        return vec![];
    }
    let mut numbered = Vec::with_capacity(entries.len());
    for entry in entries {
        let number = entry["title"].as_str().and_then(parse_episode_number);
        let thumbnail = entry["thumbnail"].as_str().unwrap_or("");
        match number {
            Some(n) if (1..=total).contains(&n) && !thumbnail.is_empty() => numbered.push((n, thumbnail.to_string())),
            _ => return vec![],
        }
    }
    numbered.sort_by_key(|(n, _)| *n);
    if numbered.windows(2).any(|w| w[0].0 == w[1].0) {
        return vec![];
    }
    numbered.into_iter().map(|(_, url)| url).collect()
}

// Empty vec is the one "no art" answer, whatever the cause (network error,
// malformed response, untrustworthy list) — the strip's cover-art fallback
// covers all of them identically, and a hard error would only cost the caller
// a try/catch to arrive at the same render.
#[tauri::command]
pub async fn get_anime_episode_stills(id: u64) -> Result<Vec<String>, String> {
    let query = "
        query ($id: Int) {
            Media(id: $id, type: ANIME) {
                episodes
                streamingEpisodes { title thumbnail }
            }
        }
    ";
    let Ok(data) = anilist_query(query, json!({ "id": id })).await else {
        return Ok(vec![]);
    };
    let media = &data["data"]["Media"];
    Ok(stills_if_complete(media["episodes"].as_u64(), &media["streamingEpisodes"]))
}

const SEASONS: [&str; 4] = ["WINTER", "SPRING", "SUMMER", "FALL"];

// Days-since-epoch -> (year, month) via Howard Hinnant's civil_from_days
// (http://howardhinnant.github.io/date_algorithms.html), used only to pick
// the current anime season without pulling in a date/time crate.
fn current_year_month_utc() -> (i32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86400) as i64;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32)
}

#[cfg(test)]
mod next_airing_tests {
    use super::*;
    use serde_json::json;

    fn media(extra: serde_json::Value) -> serde_json::Value {
        let mut base = json!({
            "id": 1,
            "title": { "romaji": "Test Show", "english": null },
            "coverImage": { "extraLarge": "http://img", "large": null },
            "episodes": 12,
            "averageScore": 80,
            "genres": ["Action"],
            "description": "Synopsis."
        });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    }

    // Tier 1: currently airing — AniList gives an exact timestamp + episode.
    #[test]
    fn releasing_uses_next_airing_episode() {
        let item = media(json!({
            "status": "RELEASING",
            "nextAiringEpisode": { "airingAt": 1790949600i64, "episode": 12 },
            "startDate": { "year": 2026, "month": 7, "day": 5 }
        }));
        let parsed = parse_media_item(&item).unwrap();
        assert_eq!(parsed.next_airing_at, Some(1790949600));
        assert_eq!(parsed.next_episode, Some(12));
    }

    // Tier 2: upcoming with a announced premiere date but no schedule entry —
    // count down to the start date instead, with no episode number.
    #[test]
    fn unreleased_falls_back_to_full_start_date() {
        let item = media(json!({
            "status": "NOT_YET_RELEASED",
            "nextAiringEpisode": null,
            "startDate": { "year": 2026, "month": 10, "day": 20 }
        }));
        let parsed = parse_media_item(&item).unwrap();
        // 2026-10-20T00:00:00Z
        assert_eq!(parsed.next_airing_at, Some(1792454400));
        assert_eq!(parsed.next_episode, None);
    }

    // Tier 3: upcoming, year only ("2027") — nothing to count down to.
    #[test]
    fn unreleased_year_only_start_date_yields_no_countdown() {
        let item = media(json!({
            "status": "NOT_YET_RELEASED",
            "nextAiringEpisode": null,
            "startDate": { "year": 2027, "month": null, "day": null }
        }));
        let parsed = parse_media_item(&item).unwrap();
        assert_eq!(parsed.next_airing_at, None);
        assert_eq!(parsed.next_episode, None);
    }

    #[test]
    fn missing_schedule_fields_entirely_yields_no_countdown() {
        let item = media(json!({ "status": "FINISHED" }));
        let parsed = parse_media_item(&item).unwrap();
        assert_eq!(parsed.next_airing_at, None);
        assert_eq!(parsed.next_episode, None);
    }

    #[test]
    fn epoch_from_civil_date_matches_known_timestamps() {
        assert_eq!(epoch_from_civil(1970, 1, 1), 0);
        assert_eq!(epoch_from_civil(2000, 3, 1), 951868800);
        assert_eq!(epoch_from_civil(2026, 10, 20), 1792454400);
    }
}

#[cfg(test)]
mod episode_stills_tests {
    use super::*;
    use serde_json::json;

    // AniList entries carry only a free-text title and a thumbnail URL.
    fn ep(title: &str) -> serde_json::Value {
        json!({ "title": title, "thumbnail": format!("http://img/{title}") })
    }

    // A full, well-formed list: "Episode 1 - Ep 1" .. "Episode n - Ep n".
    fn eps(range: std::ops::RangeInclusive<u64>) -> serde_json::Value {
        serde_json::Value::Array(range.map(|n| ep(&format!("Episode {n} - Ep {n}"))).collect())
    }

    // ---- parse_episode_number ----

    #[test]
    fn parses_crunchyroll_style_titles() {
        assert_eq!(parse_episode_number("Episode 1 - Cruelty"), Some(1));
        assert_eq!(parse_episode_number("Episode 24 - Finale"), Some(24));
        assert_eq!(parse_episode_number("Episode 01 - Cold Open"), Some(1));
    }

    // Episode 0 must parse, not vanish: a {0..24} list has to be *rejected* by
    // the set rule, not silently trimmed into an accepted {1..24}.
    #[test]
    fn parses_episode_zero() {
        assert_eq!(parse_episode_number("Episode 0 - Prologue"), Some(0));
    }

    // "Episode 6.5" is a recap. Truncating to 6 would collide with the real
    // episode 6 and put the recap's image on it.
    #[test]
    fn rejects_fractional_episode_numbers() {
        assert_eq!(parse_episode_number("Episode 6.5 - Recap"), None);
    }

    #[test]
    fn rejects_titles_without_a_number() {
        assert_eq!(parse_episode_number("OVA - Beach Special"), None);
        assert_eq!(parse_episode_number("Special"), None);
        assert_eq!(parse_episode_number(""), None);
    }

    #[test]
    fn first_number_in_the_title_wins() {
        assert_eq!(parse_episode_number("Episode 1 - Episode 2 of the Plan"), Some(1));
    }

    // ---- stills_if_complete: accepted ----

    #[test]
    fn accepts_a_list_covering_every_episode() {
        assert_eq!(stills_if_complete(Some(24), &eps(1..=24)).len(), 24);
        assert_eq!(stills_if_complete(Some(12), &eps(1..=12)).len(), 12);
    }

    // AniList's order is not the episode order (One Piece returns descending),
    // so the caller's stills[i] would otherwise be a real-but-wrong image.
    #[test]
    fn returns_stills_sorted_by_episode_number() {
        let list = json!([ep("Episode 3 - C"), ep("Episode 1 - A"), ep("Episode 2 - B")]);
        assert_eq!(
            stills_if_complete(Some(3), &list),
            vec!["http://img/Episode 1 - A", "http://img/Episode 2 - B", "http://img/Episode 3 - C"]
        );
    }

    // ---- stills_if_complete: rejected (franchise-level lists) ----

    // streamingEpisodes is franchise-wide, so a sequel's list is often the
    // prequel's episodes — real images from the wrong season.
    #[test]
    fn rejects_superset_of_this_entrys_episodes() {
        assert!(stills_if_complete(Some(23), &eps(1..=24)).is_empty()); // Jujutsu Kaisen S2
        assert!(stills_if_complete(Some(11), &eps(1..=26)).is_empty()); // Kimetsu Yuukaku-hen
        assert!(stills_if_complete(Some(16), &eps(1..=25)).is_empty()); // AoT Final
    }

    #[test]
    fn rejects_subset_of_this_entrys_episodes() {
        assert!(stills_if_complete(Some(500), &eps(1..=32)).is_empty()); // Naruto Shippuuden
    }

    // Tensura S2: exactly 24 entries, numbered 49-72. A count-only check passes
    // this and mislabels every card.
    #[test]
    fn rejects_right_count_with_wrong_numbers() {
        assert!(stills_if_complete(Some(24), &eps(49..=72)).is_empty());
    }

    #[test]
    fn rejects_zero_indexed_list() {
        assert!(stills_if_complete(Some(11), &eps(0..=24)).is_empty()); // Mushoku Tensei
    }

    #[test]
    fn rejects_duplicate_numbers_even_when_the_count_matches() {
        let list = json!([ep("Episode 1 - A"), ep("Episode 1 - B")]);
        assert!(stills_if_complete(Some(2), &list).is_empty());
    }

    // Length alone matches here (12 entries, 12 episodes) — only the entry that
    // carries no episode number rejects the list.
    #[test]
    fn rejects_list_with_an_unparseable_entry() {
        let mut list = eps(1..=11);
        list.as_array_mut().unwrap().push(ep("Special - Recap"));
        assert!(stills_if_complete(Some(12), &list).is_empty());
    }

    // All-or-nothing: one missing image is not a gap to fill, it's a list we
    // stop trusting — the fallback is per-title, not per-card.
    #[test]
    fn rejects_perfect_set_with_a_missing_thumbnail() {
        for missing in [serde_json::Value::Null, json!("")] {
            let mut list = eps(1..=3);
            list[1]["thumbnail"] = missing;
            assert!(stills_if_complete(Some(3), &list).is_empty());
        }
    }

    // Media.episodes is null while a show is still releasing (One Piece).
    #[test]
    fn rejects_unknown_or_zero_episode_count() {
        assert!(stills_if_complete(None, &eps(1..=1000)).is_empty());
        assert!(stills_if_complete(Some(0), &eps(1..=1000)).is_empty());
    }

    #[test]
    fn rejects_malformed_streaming_episodes() {
        assert!(stills_if_complete(Some(12), &serde_json::Value::Null).is_empty());
        assert!(stills_if_complete(Some(12), &json!([])).is_empty());
        assert!(stills_if_complete(Some(12), &json!({})["streamingEpisodes"]).is_empty());
        assert!(stills_if_complete(Some(1), &json!([{ "thumbnail": "http://img/1" }])).is_empty());
    }
}
