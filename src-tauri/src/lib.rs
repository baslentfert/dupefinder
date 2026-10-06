//! Tauri shell around the [`dupe_core`] engine.

mod commands;
mod state;
mod thumbnails;
mod tray;

use std::sync::OnceLock;
use std::time::Instant;

use tauri::{Manager, WindowEvent};

use state::AppState;

/// When the process began, so "time to interactive" can be reported rather
/// than guessed at.
static STARTED: OnceLock<Instant> = OnceLock::new();

pub fn elapsed_since_start() -> std::time::Duration {
    STARTED.get().map(|t| t.elapsed()).unwrap_or_default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = STARTED.set(Instant::now());

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dupefinder=info,dupe_core=info".into()),
        )
        .init();

    tauri::Builder::default()
        // A second launch should surface the running instance, not start a
        // rival scan against the same cache.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            tray::show_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol(thumbnails::SCHEME, |ctx, request, responder| {
            thumbnails::handle(ctx.app_handle(), request, responder);
        })
        .setup(|app| {
            let cache_path = app
                .path()
                .app_local_data_dir()
                .map(|dir| dir.join("hashes.redb"))
                .unwrap_or_else(|_| std::path::PathBuf::from("hashes.redb"));

            tracing::info!(
                ?cache_path,
                elapsed_ms = elapsed_since_start().as_millis() as u64,
                "setup"
            );
            app.manage(AppState::new(cache_path));

            tray::build(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Hide to the tray instead of exiting: a full-disk scan takes
                // minutes and the results only live in memory.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::frontend_ready,
            commands::system_info,
            commands::start_scan,
            commands::start_orphan_scan,
            commands::cancel_scan,
            commands::activity,
            commands::duplicate_page,
            commands::similar_page,
            commands::folder_page,
            commands::suggest_folder_plan,
            commands::folder_plan,
            commands::orphan_page,
            commands::suggest_duplicate_plan,
            commands::suggest_similar_plan,
            commands::check_plan,
            commands::check_orphan_plan,
            commands::restore_point_availability,
            commands::apply_plan,
            commands::apply_orphan_plan,
            commands::reveal_in_explorer,
            commands::resolved_strategy,
            commands::clear_results,
            commands::export_report,
            commands::save_report,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
