use crate::models::{GenreOption, MovieResult, SearchPage, TvEpisode, TvSeason};
use crate::procutil::{http_client, json_with_retry, random_below, urlencode};

// Official TMDB REST API (needs a free v3 API key from themoviedb.org) —
// used for search/browse/season+episode metadata, mirroring how AniList is
// used for anime. mov-cli itself scrapes themoviedb.org's HTML for this,
// which is more fragile than just using their real API.
const TMDB_API: &str = "https://api.themoviedb.org/3";
const IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w500";

fn genre_name(media_type: &str, id: u64) -> Option<&'static str> {
    if media_type == "tv" {
        Some(match id {
            10759 => "Action & Adventure",
            16 => "Animation",
            35 => "Comedy",
            80 => "Crime",
            99 => "Documentary",
            18 => "Drama",
            10751 => "Family",
            10762 => "Kids",
            9648 => "Mystery",
            10763 => "News",
            10764 => "Reality",
            10765 => "Sci-Fi & Fantasy",
            10766 => "Soap",
            10767 => "Talk",
            10768 => "War & Politics",
            37 => "Western",
            _ => return None,
        })
    } else {
        Some(match id {
            28 => "Action",
            12 => "Adventure",
            16 => "Animation",
            35 => "Comedy",
            80 => "Crime",
            99 => "Documentary",
            18 => "Drama",
            10751 => "Family",
            14 => "Fantasy",
            36 => "History",
            27 => "Horror",
            10402 => "Music",
            9648 => "Mystery",
            10749 => "Romance",
            878 => "Science Fiction",
            10770 => "TV Movie",
            53 => "Thriller",
            10752 => "War",
            37 => "Western",
            _ => return None,
        })
    }
}

async fn tmdb_get(path: &str, api_key: &str, extra_query: &str) -> Result<serde_json::Value, String> {
    if api_key.trim().is_empty() {
        return Err("No TMDB API key set. Add one in About → Movies & Series.".to_string());
    }

    let client = http_client();
    let sep = if path.contains('?') { "&" } else { "?" };
    let url = format!("{}{}{}api_key={}{}", TMDB_API, path, sep, api_key, extra_query);

    json_with_retry("TMDB", || client.get(&url).send()).await.map_err(|e| {
        if e.contains("401") {
            "TMDB rejected the API key — double check it in About → Movies & Series.".to_string()
        } else {
            e
        }
    })
}

fn parse_result_list(items: &[serde_json::Value], fallback_media_type: Option<&str>) -> Vec<MovieResult> {
    items
        .iter()
        .filter_map(|item| {
            let media_type = item["media_type"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| fallback_media_type.map(|s| s.to_string()))?;

            if media_type != "movie" && media_type != "tv" {
                return None; // skip "person" results from /search/multi
            }

            let tmdb_id = item["id"].as_u64()?;
            let title = item["title"].as_str().or_else(|| item["name"].as_str())?.to_string();
            let image_path = item["poster_path"].as_str().or_else(|| item["backdrop_path"].as_str());
            let image_url = image_path.map(|p| format!("{}{}", IMAGE_BASE, p)).unwrap_or_default();
            let date = item["release_date"].as_str().or_else(|| item["first_air_date"].as_str());
            let year = date.and_then(|d| d.split('-').next()).filter(|s| !s.is_empty()).map(|s| s.to_string());
            let release_date = date.filter(|s| !s.is_empty()).map(|s| s.to_string());

            let genres = item["genre_ids"]
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .filter_map(|g| g.as_u64())
                        .filter_map(|id| genre_name(&media_type, id))
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();

            Some(MovieResult {
                tmdb_id,
                title,
                media_type,
                image_url,
                year,
                release_date,
                overview: item["overview"].as_str().map(|s| s.to_string()),
                rating: item["vote_average"].as_f64(),
                genres,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn search_movies(query: &str, api_key: &str, page: u64) -> Result<SearchPage, String> {
    let page = page.max(1);
    let data = tmdb_get("/search/multi", api_key, &format!("&query={}&page={}", urlencode(query), page)).await?;
    let items = data["results"].as_array().cloned().unwrap_or_default();
    let total_pages = data["total_pages"].as_u64().unwrap_or(1);
    Ok(SearchPage { results: parse_result_list(&items, None), has_more: page < total_pages })
}

// TMDB's own genre catalog, fetched live rather than hardcoded — it's the
// same list TMDB uses for /discover's `with_genres` filter, so it can't
// drift out of sync with what's actually filterable.
#[tauri::command]
pub async fn get_genres(media_type: &str, api_key: &str) -> Result<Vec<GenreOption>, String> {
    if media_type != "movie" && media_type != "tv" {
        return Err("media_type must be \"movie\" or \"tv\"".to_string());
    }
    let data = tmdb_get(&format!("/genre/{}/list", media_type), api_key, "").await?;
    Ok(data["genres"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|g| Some(GenreOption { id: g["id"].as_u64()?, name: g["name"].as_str()?.to_string() }))
        .collect())
}

#[tauri::command]
pub async fn get_trending_movies_tv(api_key: &str) -> Result<Vec<MovieResult>, String> {
    let data = tmdb_get("/trending/all/week", api_key, "").await?;
    let items = data["results"].as_array().cloned().unwrap_or_default();
    Ok(parse_result_list(&items, None).into_iter().take(20).collect())
}

#[tauri::command]
pub async fn get_tv_seasons(tmdb_id: u64, api_key: &str) -> Result<Vec<TvSeason>, String> {
    let data = tmdb_get(&format!("/tv/{}", tmdb_id), api_key, "").await?;
    let seasons = data["seasons"].as_array().cloned().unwrap_or_default();

    Ok(seasons
        .iter()
        .filter_map(|s| {
            let season_number = s["season_number"].as_u64()? as u32;
            if season_number == 0 {
                return None; // "Specials" — not part of the normal episode flow
            }
            Some(TvSeason {
                season_number,
                name: s["name"].as_str().unwrap_or("").to_string(),
                episode_count: s["episode_count"].as_u64().unwrap_or(0) as u32,
            })
        })
        .collect())
}

// Genre/year browsing via TMDB's /discover endpoint (unlike /search, this
// takes structured filters instead of a text query) — the same source as
// search_movies/get_trending_movies_tv, just filtered and sorted by popularity.
#[tauri::command]
pub async fn discover_media(media_type: &str, genre_id: Option<u64>, year: Option<u32>, api_key: &str, page: u64) -> Result<SearchPage, String> {
    if media_type != "movie" && media_type != "tv" {
        return Err("media_type must be \"movie\" or \"tv\"".to_string());
    }

    let page = page.max(1);
    let data = tmdb_get(&format!("/discover/{}", media_type), api_key, &discover_query(genre_id, year, media_type, Some(page))).await?;
    let items = data["results"].as_array().cloned().unwrap_or_default();
    let total_pages = data["total_pages"].as_u64().unwrap_or(1);
    Ok(SearchPage { results: parse_result_list(&items, Some(media_type)), has_more: page < total_pages })
}

// tmdb_get already inserts "?api_key=..." (or "&api_key=..." if the path has
// its own "?"), so this — like every other extra_query builder in this file
// — must lead with "&", not "?".
fn discover_query(genre_id: Option<u64>, year: Option<u32>, media_type: &str, page: Option<u64>) -> String {
    let mut query = "&sort_by=popularity.desc".to_string();
    if let Some(id) = genre_id {
        query.push_str(&format!("&with_genres={}", id));
    }
    if let Some(y) = year {
        let key = if media_type == "movie" { "primary_release_year" } else { "first_air_date_year" };
        query.push_str(&format!("&{}={}", key, y));
    }
    if let Some(p) = page {
        query.push_str(&format!("&page={}", p));
    }
    query
}

// Same "probe the count, then re-fetch a random page" trick as
// anilist::get_random_anime, adapted to TMDB's page-based (not offset-based)
// pagination: /discover pages are 20 items each, so a random item within a
// random page gives more spread than always taking result #1 of that page.
#[tauri::command]
pub async fn get_random_media(media_type: &str, genre_id: Option<u64>, api_key: &str) -> Result<Option<MovieResult>, String> {
    const PAGE_CAP: u64 = 100; // TMDB itself refuses to paginate past page 500

    if media_type != "movie" && media_type != "tv" {
        return Err("media_type must be \"movie\" or \"tv\"".to_string());
    }

    let probe = tmdb_get(&format!("/discover/{}", media_type), api_key, &discover_query(genre_id, None, media_type, None)).await?;
    let total_pages = probe["total_pages"].as_u64().unwrap_or(0);
    if total_pages == 0 {
        return Ok(None);
    }

    let page = 1 + random_below(total_pages.min(PAGE_CAP));
    let data = if page == 1 {
        probe
    } else {
        tmdb_get(&format!("/discover/{}", media_type), api_key, &discover_query(genre_id, None, media_type, Some(page))).await?
    };

    let results = parse_result_list(&data["results"].as_array().cloned().unwrap_or_default(), Some(media_type));
    if results.is_empty() {
        return Ok(None);
    }
    let idx = random_below(results.len() as u64) as usize;
    Ok(results.into_iter().nth(idx))
}

// "Because you watched X" — same result shape as every other row
// (parse_result_list, MovieResult), so the frontend needs no new rendering.
#[tauri::command]
pub async fn get_recommendations(media_type: &str, tmdb_id: u64, api_key: &str) -> Result<Vec<MovieResult>, String> {
    if media_type != "movie" && media_type != "tv" {
        return Err("media_type must be \"movie\" or \"tv\"".to_string());
    }
    let data = tmdb_get(&format!("/{}/{}/recommendations", media_type, tmdb_id), api_key, "").await?;
    let items = data["results"].as_array().cloned().unwrap_or_default();
    Ok(parse_result_list(&items, Some(media_type)).into_iter().take(20).collect())
}

#[tauri::command]
pub async fn get_season_episodes(tmdb_id: u64, season_number: u32, api_key: &str) -> Result<Vec<TvEpisode>, String> {
    let data = tmdb_get(&format!("/tv/{}/season/{}", tmdb_id, season_number), api_key, "").await?;
    let episodes = data["episodes"].as_array().cloned().unwrap_or_default();

    Ok(episodes
        .iter()
        .filter_map(|e| {
            Some(TvEpisode {
                episode_number: e["episode_number"].as_u64()? as u32,
                name: e["name"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect())
}

