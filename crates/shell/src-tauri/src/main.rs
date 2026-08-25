#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// sound-arranger Tauri shell.
///
/// This is the thin transport adapter over the Host API (the core↔host seam in
/// `crates/host`). For the scaffold it just opens the window and serves the Vue
/// frontend; wiring the `HostCommand`/events/values contract comes next.
fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
