mod commands;
mod ffmpeg;
mod merge;
mod probe;
mod ytdl;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        // A second launch just focuses the running window (avoids two setups/downloads fighting).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(merge::MergeState::default())
        .manage(ytdl::YtState::default())
        .invoke_handler(tauri::generate_handler![
            commands::tool_status,
            commands::detect_encoders,
            commands::pick_files,
            commands::pick_folder,
            commands::pick_dir,
            commands::pick_output,
            commands::probe_files,
            commands::thumbnail,
            commands::check_fast_path,
            commands::unique_path,
            commands::reveal_path,
            commands::open_path,
            merge::start_merge,
            merge::cancel_merge,
            ytdl::yt_info,
            ytdl::yt_setup,
            ytdl::yt_resolve,
            ytdl::yt_download,
            ytdl::yt_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AIO Merger");
}
