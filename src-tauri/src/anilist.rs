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
