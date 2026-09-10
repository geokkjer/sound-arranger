# Agent Note: the shell window drops its native title bar on tiling compositors

Status: implemented

## Problem

The Tauri window opened with GTK server-side decorations (a native title bar), even though the shell
draws its own top bar (brand · transport · BPM/timecode · undo · snap · mixer · status). On **niri**
(a scrollable tiling Wayland compositor) that extra bar is redundant chrome and looks out of place.
A blanket removal is wrong, though: on a floating desktop the native title bar is how the user moves
and resizes the window.

## Decision

- `tauri.conf.json` creates the `main` window **undecorated** (`"decorations": false`): the shell's
  own top bar is the chrome, matching the ui-plan's "thin, stable chrome" intent.
- In `run()`'s `setup`, `prefer_undecorated()` decides whether to keep it that way. It is true on the
  tiling compositors we detect — niri (`NIRI_SOCKET` or `XDG_CURRENT_DESKTOP`), Hyprland
  (`HYPRLAND_INSTANCE_SIGNATURE`), sway (`SWAYSOCK`) — so the window stays undecorated; otherwise
  `set_decorations(true)` restores the native title bar so a floating WM can still move/resize.

## Alternatives considered

- **Always undecorated** (`decorations: false`, no runtime logic). Simplest, but on a floating
  desktop the window becomes impossible to move or resize by hand. Rejected.
- **Always decorated** (leave the default). That is the behaviour being complained about. Rejected.
- **A user preference now.** The eventual home is the ui-plan's `Preference` contribution; an env
  heuristic is the cheap 90% until that exists. Deferred.

## Consequences

- On niri (and the other detected tiling compositors) the window has no native title bar; the shell
  renders edge to edge.
- On a floating desktop the native title bar is restored at startup, so move/resize keep working.
- The heuristic is a hard-coded list of compositor env vars; a real preference (and more
  compositors) can replace it later.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-10.
