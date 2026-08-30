const invoke = (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke)
  || (window.__TAURI__ && window.__TAURI__.invoke)
  || (window.__TAURI__ && window.__TAURI__.tauri && window.__TAURI__.tauri.invoke)
  || (() => { throw new Error('Tauri invoke not available'); });

const listen = (window.__TAURI__ && window.__TAURI__.event && window.__TAURI__.event.listen)
  || (() => { throw new Error('Tauri event listen not available'); });

// Download progress/completion/failure arrive as events, not an invoke()
// return value, since a download can run for minutes and the command
// returns as soon as it's started. Returns the unlisten function.
export const onEvent = (name, callback) => listen(name, (event) => callback(event.payload));

export const api = {
  // AniList-only metadata search for the command palette — not tied to
  // whatever ani-cli itself will resolve for a title (see watchEpisode).
  searchAnimeMeta: (query) => invoke('search_anime', { query }),
  getSeasonalAnime: (genre) => invoke('get_seasonal_anime', { genre: genre || undefined }),
  getSeasonalAnimeBy: (year, season, genre) => invoke('get_seasonal_anime_by', { year, season, genre: genre || undefined }),
  getSeasonList: () => invoke('get_season_list', {}),
  getTrendingAnime: () => invoke('get_trending_anime', {}),
  getRecentEpisodes: () => invoke('get_recent_episodes', {}),
  getAnimeInfo: (title) => invoke('get_anime_info', { title }),
  getRandomAnime: (genre) => invoke('get_random_anime', { genre: genre || undefined }),
  getAnimeGenres: () => invoke('get_anime_genres', {}),
  getAnimeRecommendations: (id) => invoke('get_anime_recommendations', { id }),
  // Verifies (at watch time only) which of ani-cli's own search results for
  // `title` is actually the right one, using AniList's known episode count
  // to disambiguate, and returns that entry's real episode numbering —
  // needed because ani-cli's source doesn't always split seasons/cours the
  // same way AniList does. See anidb.rs for why.
  resolveAnime: (title, expectedEpisodes, isReleasing) => invoke('resolve_anime', { title, expectedEpisodes: expectedEpisodes || undefined, isReleasing: !!isReleasing }),
  watchEpisode: (query, index, episode, quality) => invoke('watch_episode', { query, index, episode, quality }),
  updateAniCli: () => invoke('update_ani_cli', {}),
  // page starts at 1; returns { results, has_more } so the UI can offer
  // "Load more" instead of silently capping results at one page.
  searchMovies: (query, apiKey, page) => invoke('search_movies', { query, apiKey, page: page || 1 }),
  getGenres: (mediaType, apiKey) => invoke('get_genres', { mediaType, apiKey }),
  getTrendingMoviesTv: (apiKey) => invoke('get_trending_movies_tv', { apiKey }),
  // genreId/year come from <select> elements, so they arrive as strings even
  // though Rust expects u64/u32 — Tauri's IPC won't coerce "28" -> 28 itself.
  discoverMedia: (mediaType, genreId, year, apiKey, page) =>
    invoke('discover_media', { mediaType, genreId: genreId ? Number(genreId) : undefined, year: year ? Number(year) : undefined, apiKey, page: page || 1 }),
  getRandomMedia: (mediaType, genreId, apiKey) =>
    invoke('get_random_media', { mediaType, genreId: genreId ? Number(genreId) : undefined, apiKey }),
  getRecommendations: (mediaType, tmdbId, apiKey) => invoke('get_recommendations', { mediaType, tmdbId, apiKey }),
  getTvSeasons: (tmdbId, apiKey) => invoke('get_tv_seasons', { tmdbId, apiKey }),
  getSeasonEpisodes: (tmdbId, seasonNumber, apiKey) => invoke('get_season_episodes', { tmdbId, seasonNumber, apiKey }),
  // year (TMDB's release/first-air year, when known) disambiguates a title
  // that's indexed multiple times on the same provider under different
  // years (e.g. more than one "Spider-Man" animated series) — see
  // find_playable_subject/find_playable_release in the backend.
  getStreamOptions: (title, season, episode, subjectId, year) =>
    invoke('get_stream_options', { title, season, episode, subjectId, year: year ? Number(year) : undefined }),
  playStream: (resourceLink, windowTitle, subjectId, resourceId) =>
    invoke('play_stream', { resourceLink, windowTitle, subjectId, resourceId }),
  getMovieSubject: (title, year) => invoke('get_movie_subject', { title, year: year ? Number(year) : undefined }),
  getSubtitleOptions: (subjectId, resourceId) => invoke('get_subtitle_options', { subjectId, resourceId }),
  startDownload: (resourceLink, title, season, episode, windowTitle, subtitleUrl) =>
    invoke('start_download', { resourceLink, title, season, episode, windowTitle, subtitleUrl: subtitleUrl || undefined }),
  getDownloadsInfo: () => invoke('get_downloads_info', {}),
  clearDownloads: () => invoke('clear_downloads', {}),
  // Alternate source to MovieBox for movie/series playback.
  getFourkStreamOptions: (title, season, episode, year) =>
    invoke('get_fourk_stream_options', { title, season, episode, year: year ? Number(year) : undefined }),
  // releases: chosen quality first, then the other quality/codec options as
  // fallback — 4KHDHub's mirrors are third-party file hosts that go down
  // independently per-release, so the backend tries each in order.
  playFourkStream: (releases, windowTitle) => invoke('play_fourk_stream', { releases, windowTitle }),
};
