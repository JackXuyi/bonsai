//! L0 — Shell: tray icon + menu, lazy main window.
//!
//! Research constraints (report §3.2/§3.3):
//! - Boot to tray only; never pop a window at startup (top uninstall trigger).
//! - Tray left-click opens the window; the window is created on demand.
//!
//! Dock visibility follows the window: while the face is on screen the app is a
//! regular Dock app; closing the window sends it back to a menu-bar-only
//! resident (Accessory), so it keeps running in the background without a Dock
//! tile or Cmd-Tab entry.

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder,
};

pub fn setup(app: &mut App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show bonsai", true, None::<&str>)?;
    let redo = MenuItem::with_id(app, "redo-onboarding", "重新做一次引导", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide to tray", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &redo, &hide, &quit])?;

    TrayIconBuilder::with_id("bonsai-tray")
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/tray.png"
        ))?)
        .tooltip("bonsai — growing quietly")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "redo-onboarding" => {
                // Drop only the done flag; answers stay for pre-filling.
                use tauri::Manager;
                if let Some(store) = app.try_state::<crate::memory::MemoryStore>() {
                    if let Ok(conn) = store.inner().0.lock() {
                        let _ = crate::memory::db::remove_setting(&conn, "onboarding_done");
                    }
                }
                show_main_window(app);
            }
            "hide" => hide_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// Show the face and hand the app back its Dock tile.
pub fn show_main_window(app: &AppHandle) {
    // Dock policy has to switch before the window is built/raised, otherwise
    // macOS keeps the window out of the Dock and Cmd-Tab list.
    set_dock_visible(app, true);
    // Coming back from Accessory does not auto-activate, so the face would open
    // behind the frontmost app.
    #[cfg(target_os = "macos")]
    let _ = app.show();

    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        // The window-open breakpoint (plan-morph.md §3.5): the frontend
        // drains the pending growth announcements here, never proactively.
        use tauri::Emitter;
        let _ = app.emit("window:shown", ());
    } else {
        let _ = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
            .title("bonsai")
            .inner_size(520.0, 560.0)
            .resizable(false)
            .build();
    }
}

/// Closing the window means "run in the background": hide it and drop the Dock
/// icon so only the menu-bar tray remains. The process keeps running.
pub fn hide_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
    set_dock_visible(app, false);
}

/// Toggle the Dock tile / Cmd-Tab entry. No-op off macOS, where the Dock does
/// not exist and the tray alone is the background affordance.
#[cfg(target_os = "macos")]
pub fn set_dock_visible(app: &AppHandle, visible: bool) {
    let policy = if visible {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    let _ = app.set_activation_policy(policy);
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_visible(_app: &AppHandle, _visible: bool) {}
