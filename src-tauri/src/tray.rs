//! The system tray icon.
//!
//! The app lives in the tray: closing the window hides it rather than exiting,
//! so a long scan keeps running and the results stay in memory. The only way
//! out is the tray's Quit item — which is also why Quit cancels any running
//! scan first, instead of tearing down a thread mid-write.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::state::AppState;

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Open Duplicate Finder", true, None::<&str>)?;
    let cancel = MenuItem::with_id(app, "cancel", "Stop the running scan", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &show,
            &PredefinedMenuItem::separator(app)?,
            &cancel,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().ok_or_else(|| {
            tauri::Error::AssetNotFound("the bundle has no default window icon".into())
        })?)
        .tooltip("Duplicate Finder")
        .menu(&menu)
        // The menu is for the right button; a left click should just open the
        // window, which is what every other tray app does.
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
        "show" => show_window(app),
        "cancel" => app.state::<AppState>().cancel(),
        "quit" => {
            // Let the engine unwind and flush its cache rather than killing
            // it mid-scan.
            app.state::<AppState>().cancel();
            app.exit(0);
        }
        _ => {}
    }
}

pub fn show_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}
