# Parked: the Nix / devenv dev shell

**Status: parked (2026-09-10), not used for development.** Reproducibility is deferred; dev now
uses the host (Arch/CachyOS) toolchain. The decision and the full diagnosis are in the
[native host dev toolchain note](../.agents/notes/implemented/process/2026-09-10-native-host-dev-toolchain.md).

## Why it's parked

On this **non-NixOS** host a Nix-built GUI binary cannot open the window:

1. **No EGL vendor.** The Nix `glvnd` in the devenv closure ships no EGL vendor (`libEGL_mesa.so`
   is absent from the closure), so WebKit's EGL display creation aborts:
   `Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...` — a blank window. The
   host's own EGL is fine (verified with `eglinfo` and `weston-simple-egl` at 60 fps under niri).
2. **glibc too old.** Linking the *host* GTK/WebKit instead does not help: host
   `libwebkit2gtk-4.1.so.0` needs **`GLIBC_2.44`**, while the Nix toolchain's glibc is **2.42**. A
   Nix-loaded binary can therefore never load the host WebKit (`version 'GLIBC_2.44' not found`);
   and the Nix dynamic loader does not search `/usr/lib`, so it cannot even find `libgdk-3.so.0`.

A reproducible/packaged build will need a real answer to both (e.g. `nixGL` for the GL stack, or
building/packaging against the host stack, or shipping a self-contained bundle). Until then these
files are kept for reference, with the lock that was in use.

## Using it anyway

```sh
cd nix
devenv shell        # or: nix develop
```

There is no `.envrc` any more, so nothing auto-activates the devenv when you enter the repo.
