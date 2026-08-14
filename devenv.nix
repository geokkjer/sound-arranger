{ pkgs, lib, config, inputs, ... }:

{
  # https://devenv.sh/reference/options/

  # --- Rust toolchain ---------------------------------------------------------
  # rustup-managed stable channel (same as tidal-lsp); reproducible and
  # independent of the host system's rustc.
  languages.rust.enable = true;
  languages.rust.channel = "stable";

  # --- Node + pnpm (Vue frontend) --------------------------------------------
  languages.javascript.enable = true;
  languages.javascript.package = pkgs.nodejs_22;   # nodejs_20 was removed from nixpkgs
  languages.javascript.pnpm.enable = true;

  # --- Tauri v2 (Linux/GTK) system deps --------------------------------------
  # Required by wry / webkit2gtk. See https://v2.tauri.app/start/prerequisites/
  # plus ALSA for cpal's backend (alsa-lib headers) and alsa-utils for smoke tests.
  packages = with pkgs; [
    webkitgtk_4_1
    gtk3
    libayatana-appindicator
    librsvg
    libsoup_3
    openssl
    pkg-config
    dbus

    alsa-lib
    alsa-utils
    # jack2           # enable if you turn on cpal's `jack` feature
  ];

  # --- Shell banner -----------------------------------------------------------
  enterShell = ''
    echo "sound-arranger dev shell"
    echo "  cargo: $(cargo --version)"
    echo "  rustc: $(rustc --version)"
    echo "  node:  $(node --version)   pnpm: $(pnpm --version)"
    echo "  (frontend not scaffolded yet — 'pnpm tauri dev' once it is)"
  '';
}
