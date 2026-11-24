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
    App, AppHandle, Manager, PhysicalSize, Size, WebviewUrl, WebviewWindowBuilder,
};

/// Default Companion window (plan-chat-first-ui v2 Q5): 380×560.
const DEFAULT_SIZE: (f64, f64) = (380.0, 560.0);
/// Floor for drag-resize: Companion 窄窗下限。
const MIN_SIZE: (f64, f64) = (360.0, 480.0);
/// settings key holding the last user-chosen size, "WIDTHxHEIGHT" in
/// physical px (what `inner_size()` reports, DPI-independent round-trip).
const SIZE_SETTING: &str = "window_size_px";

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
                    if let Ok(conn) = store.inner().write_guard() {
                        let _ = crate::memory::db::remove_setting(&conn, "onboarding_done");
                    }
                }
                show_main_window(app);
            }
            "hide" => hide_main_window(app),
            "quit" => {
                // Last chance to remember the size: the window may still be
                // open when the user quits from the tray.
                persist_window_size(app);
                app.exit(0);
            }
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
            .inner_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1)
            // User-resizable now; the saved size is restored below.
            .resizable(true)
            .min_inner_size(MIN_SIZE.0, MIN_SIZE.1)
            .build();
        if let (Some(win), Some((w, h))) =
            (app.get_webview_window("main"), saved_window_size(app))
        {
            // Physical px: the stored size round-trips exactly regardless of
            // the display's scale factor.
            let _ = win.set_size(Size::Physical(PhysicalSize::new(w, h)));
        }
    }
}

/// Closing the window means "run in the background": hide it and drop the Dock
/// icon so only the menu-bar tray remains. The process keeps running.
pub fn hide_main_window(app: &AppHandle) {
    persist_window_size(app);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
    set_dock_visible(app, false);
}

/// Save the current window size (physical px) into the settings table so the
/// next window-open restores what the user dragged it to. Silent best-effort:
/// a locked DB or a missing window must never break hide/quit.
fn persist_window_size(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let Ok(size) = win.inner_size() else {
        return;
    };
    if let Some(store) = app.try_state::<crate::memory::MemoryStore>() {
        if let Ok(conn) = store.inner().write_guard() {
            let _ = crate::memory::db::set_setting(
                &conn,
                SIZE_SETTING,
                &format_size_setting(size.width, size.height),
            );
        }
    }
}

/// Restore target from the settings table, if any.
fn saved_window_size(app: &AppHandle) -> Option<(u32, u32)> {
    let store = app.try_state::<crate::memory::MemoryStore>()?;
    let conn = store.inner().write_guard().ok()?;
    let raw = crate::memory::db::get_setting(&conn, SIZE_SETTING).ok()??;
    parse_size_setting(&raw)
}

/// "WIDTHxHEIGHT" — the canonical format for the size setting.
fn format_size_setting(width: u32, height: u32) -> String {
    format!("{width}x{height}")
}

fn parse_size_setting(raw: &str) -> Option<(u32, u32)> {
    let (w, h) = raw.split_once('x')?;
    let w = w.trim().parse::<u32>().ok()?;
    let h = h.trim().parse::<u32>().ok()?;
    // Reject nonsense (zero / absurd) so a corrupt setting falls back to
    // the default instead of a 1×1 window.
    if w == 0 || h == 0 || w > 20_000 || h > 20_000 {
        return None;
    }
    Some((w, h))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_setting_round_trips() {
        let raw = format_size_setting(1560, 960);
        assert_eq!(raw, "1560x960");
        assert_eq!(parse_size_setting(&raw), Some((1560, 960)));
    }

    #[test]
    fn size_setting_rejects_garbage() {
        assert_eq!(parse_size_setting(""), None);
        assert_eq!(parse_size_setting("1560"), None);
        assert_eq!(parse_size_setting("axb"), None);
        assert_eq!(parse_size_setting("1560 x"), None);
        assert_eq!(parse_size_setting("0x960"), None);
        assert_eq!(parse_size_setting("999999x960"), None);
        // Whitespace around numbers is tolerated.
        assert_eq!(parse_size_setting(" 1560 x 960 "), Some((1560, 960)));
    }
}
