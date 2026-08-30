use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct SeasonalAnime {
    pub id: u64,
    pub title: String,
    pub image_url: String,
    pub episodes: Option<u64>,
    pub score: Option<f64>,
    pub status: String,
    pub genres: Vec<String>,
    pub synopsis: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct RecentEpisode {
    pub id: u64,
    pub title: String,
    pub image_url: String,
    pub episode_title: String,
}

#[derive(Serialize, Clone)]
pub struct SeasonYear {
    pub year: u32,
    pub seasons: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct Episode {
    pub number: String,
    pub title: String,
}

// The result of verifying an AniList title against ani-cli's own source
// before actually invoking ani-cli (see anidb.rs) — query/index are what
// non-interactively replay the matching search result via `-S <index>`, and
// episodes is that entry's *real* episode numbering (which can be global
// across seasons, not always 1..N per season like AniList models it).
#[derive(Serialize, Clone)]
pub struct ResolvedAnime {
    pub query: String,
    pub index: u32,
    pub episodes: Vec<Episode>,
}

#[derive(Serialize, Clone)]
pub struct MovieResult {
    pub tmdb_id: u64,
    pub title: String,
    // "movie" or "tv" — TMDB's /search/multi mixes both; this decides
    // whether Details shows a season/episode picker or a single Play button.
    pub media_type: String,
    pub image_url: String,
    pub year: Option<String>,
    // Full "YYYY-MM-DD", not just `year` — a wishlisted-but-unreleased movie
    // needs the real date to know when it's actually out (see
    // js/main.js checkNotifications).
    pub release_date: Option<String>,
    pub overview: Option<String>,
    pub rating: Option<f64>,
    pub genres: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct GenreOption {
    pub id: u64,
    pub name: String,
}

// A page of search/discover results plus whether TMDB has more pages left —
// lets the frontend show a "Load more" button instead of silently capping
// results at whatever the first page happened to contain.
#[derive(Serialize, Clone)]
pub struct SearchPage {
    pub results: Vec<MovieResult>,
    pub has_more: bool,
}

#[derive(Serialize, Clone)]
pub struct TvSeason {
    pub season_number: u32,
    pub name: String,
    pub episode_count: u32,
}

#[derive(Serialize, Clone)]
pub struct TvEpisode {
    pub episode_number: u32,
    pub name: String,
}

#[derive(Serialize, Clone)]
pub struct StreamOption {
    pub resource_link: String,
    pub resolution: u64,
    pub codec: String,
    pub size: String,
    pub uploader: String,
    // Needed to fetch this option's subtitle tracks on demand at play time
    // (MovieBox's captions endpoint is keyed by both, not just resourceId).
    pub subject_id: String,
    pub resource_id: String,
    // Set only for 4KHDHub options: resource_link there is a mirror page,
    // not a direct video URL, so playback needs the full Release handed
    // back to play_fourk_stream to resolve it first (see fourkhdhub.rs).
    pub fourk_release: Option<serde_json::Value>,
}

// A dubbed-language variant of a title — MovieBox treats each dub as its own
// subject with its own resource/episode list, not a flag on one subject.
#[derive(Serialize, Clone)]
pub struct LanguageOption {
    pub subject_id: String,
    pub name: String,
}

#[derive(Serialize, Clone)]
pub struct MovieSubject {
    pub subject_id: String,
    // Empty when the title only has one dub — the frontend skips the
    // language picker entirely in that (common) case.
    pub languages: Vec<LanguageOption>,
}

#[derive(Serialize, Clone)]
pub struct SubtitleOption {
    pub url: String,
    pub language: String,
}

#[derive(Serialize, Clone)]
pub struct DownloadProgressEvent {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub bytes_per_second: f64,
}

#[derive(Serialize, Clone)]
pub struct DownloadCompleteEvent {
    pub path: String,
}

#[derive(Serialize, Clone)]
pub struct DownloadErrorEvent {
    pub message: String,
}

#[derive(Serialize, Clone)]
pub struct DownloadsInfo {
    pub file_count: u32,
    pub total_bytes: u64,
}
