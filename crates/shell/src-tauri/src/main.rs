#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// sound-arranger Tauri shell.
///
/// This is the thin transport adapter over the Host API (the core↔host seam in
/// `crates/host`). The bridge lives in `lib.rs` (the "Host-API bridge"): it owns
/// a live `host::HostSession` behind Tauri commands and serves the Vue frontend.
fn main() {
    sound_arranger_shell_lib::run();
}
