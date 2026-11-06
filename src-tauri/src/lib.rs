mod commands;
mod config_store;
mod credentials;
mod models;
mod timeshift;
mod twitch;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::StatusCache::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::get_statuses,
            commands::open_player,
            commands::report_player_state,
            commands::close_player,
            commands::get_credentials_status,
            commands::open_on_twitch
        ])
        .run(tauri::generate_context!())
        .expect("error while running Twitch Timeshift");
}
