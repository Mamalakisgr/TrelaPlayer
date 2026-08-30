mod anidb;
mod anilist;
mod fourkhdhub;
mod models;
mod movplayer;
mod player;
mod procutil;
mod tmdb;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            anidb::resolve_anime,
            anilist::search_anime,
            anilist::get_seasonal_anime,
            anilist::get_seasonal_anime_by,
            anilist::get_season_list,
            anilist::get_trending_anime,
            anilist::get_recent_episodes,
            anilist::get_anime_info,
            anilist::get_random_anime,
            anilist::get_anime_genres,
            anilist::get_anime_recommendations,
            player::watch_episode,
            player::update_ani_cli,
            tmdb::search_movies,
            tmdb::get_genres,
            tmdb::get_trending_movies_tv,
            tmdb::get_tv_seasons,
            tmdb::get_season_episodes,
            tmdb::discover_media,
            tmdb::get_random_media,
            tmdb::get_recommendations,
            movplayer::get_stream_options,
            movplayer::play_stream,
            movplayer::get_movie_subject,
            movplayer::get_subtitle_options,
            movplayer::start_download,
            movplayer::get_downloads_info,
            movplayer::clear_downloads,
            fourkhdhub::get_fourk_stream_options,
            fourkhdhub::play_fourk_stream
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
