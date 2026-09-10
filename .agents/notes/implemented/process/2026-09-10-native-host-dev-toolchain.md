# Agent Note: dev uses the host toolchain — the Nix/devenv shell is parked

Status: implemented

## Problem

Dev on this machine (Arch/CachyOS, **non-NixOS**) ran inside the Nix devenv shell (`devenv.nix` +
`.envrc`). The Tauri shell built fine but opened a **blank window**:
`Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...`. Every standard WebKit
workaround failed **identically** — `WEBKIT_DISABLE_DMABUF_RENDERER=1`, `GDK_BACKEND=x11`,
`WEBKIT_DISABLE_COMPOSITING_MODE=1`, `LIBGL_ALWAYS_SOFTWARE=1` — which meant the failure was below
the renderer choice, so the cause had to be measured rather than guessed.

Diagnosis (measured):

- The binary's `RUNPATH` pointed into the Nix store (`…-libglvnd-1.7.0/lib/libEGL.so.1`,
  `…-mesa-libgbm-…`), and that Nix glvnd ships **no EGL vendor**: its
  `share/glvnd/egl_vendor.d/` does not exist and the closure contains no `libEGL_mesa.so`. glvnd
  found no vendor at all, so `eglGetPlatformDisplay` failed. The host *does* have the vendor
  (`/usr/share/glvnd/egl_vendor.d/50_mesa.json` → `/usr/lib/libEGL_mesa.so.0`) and its EGL works
  (`eglinfo`; `weston-simple-egl` at 60 fps under niri).
- Linking the **host** GUI libs instead (dropping the Nix `webkitgtk_4_1` / `gtk3` / `libsoup_3` /
  `librsvg` / `libayatana-appindicator` and pointing `PKG_CONFIG` / `LIBRARY_PATH` at `/usr/lib`)
  got past EGL, then failed at load: `libgdk-3.so.0: cannot open shared object file` (the Nix
  dynamic loader does not search `/usr/lib`) — and forcing that path would hit
  `GLIBC_2.44' not found`, because host `libwebkit2gtk-4.1.so.0` requires **GLIBC_2.44** while the
  Nix toolchain's glibc is **2.42**. A Nix-linked binary therefore **cannot** load host WebKit.

So neither half of the Nix toolchain could drive the host GUI stack: the Nix GUI libs have no EGL
vendor, and the host GUI libs demand a newer glibc than the Nix toolchain provides.

## Decision

**Dev builds and runs with the host toolchain; the Nix/devenv config is parked.** There is no
`rust-toolchain.toml` (Arch's rolling `rust` = latest stable is the target) and no Nix GUI libs in
play. Concretely:

- `devenv.nix`, `devenv.lock`, `devenv.yaml`, and `flake.nix` move to **`nix/`** (kept for a future
  reproducible/packaged build), and **`.envrc` is removed**, so direnv no longer activates the
  devenv. `nix/README.md` records why it is parked.
- Dev deps are host packages: `rust` (rustc / cargo / rustdoc / rustfmt / clippy), `nodejs` / `npm`
  / `pnpm`, `base-devel` / `gcc` / `pkgconf`, `webkit2gtk-4.1`, `gtk3`, `libsoup3`, `librsvg`,
  `libayatana-appindicator`, `alsa-lib`, `openssl`, `appmenu-gtk-module`.
- The build is a plain host build — host `cargo` + host `cc` + host `pkg-config`. Verified: the
  shell binary's interpreter is `/lib64/ld-linux-x86-64.so.2`, it has **no `runpath`**, and
  `libwebkit2gtk-4.1` / `libgtk-3` / `libc.so.6` all resolve from `/usr/lib` (no `/nix/store`);
  `cargo test --workspace` passes under the host toolchain.

Reproducibility (a pinned toolchain, a Nix/container build, a shippable bundle) is **deferred** until
a real packaging need appears.

## Alternatives considered

- **Keep the Nix GUI libs and wrap with `nixGL`.** `nixGL` targets the GL driver stack; it does not
  resolve the toolkit/glibc split that produced the abort, and it adds a non-obvious wrapper to
  every run. Deferred, not chosen for the dev loop.
- **Keep the Nix toolchain and link host GUI libs (the `PKG_CONFIG` / `LIBRARY_PATH` / host-`cc`
  experiments).** Impossible on the evidence: host WebKit needs `GLIBC_2.44`, the Nix toolchain's
  glibc is 2.42. Rejected.
- **`rustup` + a pinned `rust-toolchain.toml`.** No pin is currently needed (no nightly features;
  edition 2024 is stable), and Arch's `rust` tracks stable with less machinery. Kept as an option
  if a pin becomes useful.
- **Keep `.envrc` and disable the devenv per-shell.** Fragile — it is easy to re-enter the Nix shell
  by accident and silently get the Nix toolchain again. Removing `.envrc` is unambiguous.

## Consequences

- `pnpm tauri dev` and `cargo test --workspace` run in a **plain terminal** (not a devenv shell);
  the GUI uses the host GTK/WebKit/Mesa and opens normally.
- The dev loop depends on host packages rather than a Nix closure; the README lists them.
- Reproducibility is unowned until packaging; `nix/` preserves the previous attempt and its lock.
- A different machine (or CI) would need the same host libs; the packaging story is the place to
  solve this properly.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-10.
