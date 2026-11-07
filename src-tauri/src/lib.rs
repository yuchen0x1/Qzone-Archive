mod albums;
mod archive;
mod backup;
mod export;
mod qlogin;
mod qzone;
mod updater;

#[tauri::command]
fn exit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(archive::ArchiveState::new())
        .manage(albums::AlbumState::new())
        .manage(export::ExportState::new())
        .manage(qlogin::QLoginState::new())
        .manage(qzone::RecycleAuthState::default())
        .manage(updater::UpdateDownloadState::default())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            exit_app,
            updater::check_for_update,
            updater::download_update_asset,
            updater::cancel_update_download,
            updater::run_update_installer,
            qlogin::start_qr_login,
            qlogin::poll_qr_login,
            qlogin::get_login_status,
            qlogin::get_login_user,
            qlogin::logout_qzone,
            qlogin::open_web_login,
            qlogin::check_web_login,
            qlogin::sync_cookies_to_webview,
            qzone::fetch_first_feeds,
            qzone::fetch_more_feeds,
            qzone::open_recycle_password_window,
            qzone::check_recycle_password,
            qzone::close_recycle_password_window,
            qzone::list_recycle_albums,
            qzone::list_recycle_photos,
            qzone::load_recycle_photo_preview,
            qzone::list_qzone_albums,
            qzone::create_qzone_album,
            qzone::fetch_guestbook_page,
            qzone::fetch_friend_list,
            qzone::fetch_shuoshuo_page,
            qzone::recover_recycle_album,
            qzone::recover_recycle_photos,
            archive::start_feed_archive,
            archive::get_archive_progress,
            archive::cancel_feed_archive,
            archive::list_archive_skips,
            archive::clear_resolved_archive_skips,
            archive::retry_all_archive_skips,
            archive::retry_archive_skip,
            archive::list_archived_feeds,
            archive::list_on_this_day_feeds,
            archive::list_archived_media,
            archive::get_archived_feed,
            archive::count_archived_feeds,
            archive::export_archived_html,
            archive::load_archived_image,
            archive::load_archived_video,
            archive::get_archive_overview,
            archive::check_archive_health,
            archive::list_interactors,
            archive::get_interaction_ranking,
            archive::delete_archived_feeds,
            archive::clear_archived_feeds,
            archive::delete_all_app_data,
            archive::save_guestbook_page,
            archive::save_shuoshuo_page,
            archive::sync_friend_list,
            archive::list_friend_contacts,
            archive::export_archived_json,
            archive::export_archived_markdown,
            albums::start_album_backup,
            albums::get_album_progress,
            albums::cancel_album_backup,
            albums::list_remote_albums,
            albums::list_archived_albums,
            albums::list_archived_album_photos,
            albums::reset_failed_album_photos,
            albums::delete_archived_album,
            albums::load_album_photo,
            export::export_archive_package,
            export::get_export_progress,
            export::cancel_export,
            backup::backup_app_data,
            backup::restore_app_data,
            archive::global_search,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
