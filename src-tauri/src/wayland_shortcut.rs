//! Wayland global shortcut via the xdg-desktop-portal GlobalShortcuts interface.
//!
//! The `global-hotkey` crate (used by tauri-plugin-global-shortcut) grabs keys via
//! X11 `XGrabKey`, which a Wayland compositor refuses to honour globally. The
//! portal is the sanctioned way: we register a named shortcut with a suggested
//! trigger, the desktop binds it (and lets the user rebind in system settings),
//! and we get an `Activated` signal we turn into a window toggle.
#![cfg(target_os = "linux")]

use tauri::AppHandle;

/// Spawn the portal listener. Best-effort: if the desktop has no GlobalShortcuts
/// portal (older GNOME/KDE) it logs and gives up — Wayland users then bind a
/// custom shortcut to the app themselves.
pub fn spawn(app: AppHandle, configured: String) {
    tauri::async_runtime::spawn(async move {
        if let Err(err) = run(app, configured).await {
            log::warn!("Wayland global-shortcut portal unavailable: {err}");
        }
    });
}

async fn run(app: AppHandle, configured: String) -> ashpd::Result<()> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use futures_util::StreamExt;

    let shortcuts = GlobalShortcuts::new().await?;
    let session = shortcuts.create_session().await?;

    let trigger = to_portal_trigger(&configured);
    let new_shortcut =
        NewShortcut::new("toggle-quick-capture", "Open Myelin quick capture").preferred_trigger(trigger.as_deref());

    // Ask the desktop to bind it (may prompt the user the first time).
    shortcuts
        .bind_shortcuts(&session, &[new_shortcut], None)
        .await?;

    // Toggle the quick-capture window whenever the shortcut fires.
    let mut activated = shortcuts.receive_activated().await?;
    while let Some(event) = activated.next().await {
        if event.shortcut_id() == "toggle-quick-capture" {
            let app = app.clone();
            let _ = app.run_on_main_thread(move || {
                crate::toggle_quick_window(&app);
            });
        }
    }
    Ok(())
}

/// Best-effort conversion of our stored shortcut ("Ctrl+Space", "Ctrl+Shift+KeyT")
/// to the portal trigger syntax ("CTRL+space", "CTRL+SHIFT+t"). It's only a
/// suggestion — the desktop's portal lets the user pick the final binding.
fn to_portal_trigger(s: &str) -> Option<String> {
    let parts: Vec<String> = s
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| match p {
            "Ctrl" | "Control" => "CTRL".to_string(),
            "Alt" => "ALT".to_string(),
            "Shift" => "SHIFT".to_string(),
            "Super" | "Meta" | "Cmd" | "Command" => "LOGO".to_string(),
            other => {
                if let Some(rest) = other.strip_prefix("Key") {
                    rest.to_lowercase()
                } else if let Some(rest) = other.strip_prefix("Digit") {
                    rest.to_string()
                } else {
                    other.to_lowercase()
                }
            }
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("+"))
    }
}
