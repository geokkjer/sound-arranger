# Agent Note: UI as a plugin — the Host API contract, the headless reference host, and Tauri as the first rich shell

Status: implemented

## Problem

The project began as a tightly coupled UI sound-clip arranger; the everything-is-a-plugin discipline (composition-seams note) says UI is a capability too, not the substrate. Ardour's postmortems are the anti-pattern: session model and GUI grew together, and extracting the seams took years. The UI must be **interchangeable** — the Tauri app is a *reference implementation* of the host, not the host. The composition-seams note already demanded the two halves: "the IPC command list is written down" and "a headless smoke binary records and bounces without the frontend." This step ships the seam and its first reference host.

## Decision

1. **The Host API is the core↔host seam** — a typed, versioned contract of **commands, events, and values** (`crates/host`):
   - **Commands** — [`HostCommand`]: engine commands carry `at_frame: Option<u64>` (applied at that frame by rendering up to it; the log records the same frame — *the log is the command list*), plus the media commands `Play`/`Splice`/`Bounce` (media-side command types until P1.3 merges them into the log — the host reports them separately) and `Record` (declared, device-gated in the reference host).
   - **Events** — the log stream, meters (`meters()`), and the media command count; the host renders these, never computes them. `peaks`/`transport` land with the first shell.
   - **Values** — `providers_of` (`providers()`), the graph value, the log; the host interprets them.
   - **Versioned from day one** — `HOST_API_VERSION` (1); the text format's first line must be `host v{N}`, mismatches refused.
2. **Two connection modes**: engine plugins connect directly (in-process, `Plugin` trait, `ctx`, the log — unchanged); host/UI plugins connect through the Host API (control-rate, cross-boundary — the UI thread never touches the render path, and a real boundary is what makes the shell interchangeable).
3. **The headless reference host** — `run_script` assembles the profile (factories, mixer, player wiring, chords at frames) and executes a command list deterministically; the CLI (`src/main.rs`) is the composition-seams smoke binary (file or stdin, distinct exit codes for parse vs runtime failure, byte-identical bounces across runs — tested through the binary itself).
4. **Tauri v2 as the first rich reference shell** (next step, after the headless host passed): the Tauri Rust side is a thin transport adapter (Tauri commands ⇄ Host API); the Vue frontend is the host-side plugin runtime (the declarative loader + the profile's UI plugins + the renderers). `engine`/`media` never depend on `tauri`; swapping shells swaps only the transport adapter; the text format is the wire schema the adapter's commands validate against.

## Alternatives considered

- **Tauri-first** — faster to a GUI, but "interchangeable" would rest on one shell. Rejected: the headless host is the cheaper proof of the contract and its deterministic test harness.
- **egui / iced in-process shell** — the boundary becomes voluntary. A later second host, not the reference.
- **WASM + WebAudio browser shell** — realtime constraints differ; a later host for sharing.
- **Terminal (ratatui)** — a good later second reference (the CLI with a face).
- **Electron** — heavyweight, no reason over Tauri.

## Consequences

- **Verified by tests (6 reference-host, incl. the smoke binary):** the same full script (chain + play + splice + mid-session gain change + bounce) on fresh sessions bounces **byte-identically** with identical logs (compared by content) — determinism no longer rests on a thread race (players warm synchronously at apply); the splice applies sample-accurately (`deferred == 0`) and its effect is in the bounced window (the spliced bounce differs from an unspliced control); the `SetParam` is logged at its real frame (`at_frame: 2000`); refused commands (type-mismatched patch, play-without-mixer, splice-without-play, play-twice) are rejected identically, never logged; `unmount`/`set_tempo` run and are fail-loud (engine-side: unknown plugin / non-positive tempo refused — the log never records things that never happened); the versioned text format parses and refuses version/name/channel errors; the smoke binary records, bounces, is deterministic via stdin, and exits 2 on a bad script. Engine gained `flush_scheduled()` (the control→render handoff's seed) — the host wires cords after it, so **no audio is discarded** materializing mounts.
- **kimi review (2026-08-18)**: two criticals + ten majors, all folded — see [the review archive](research/architecture/2026-08-18-kimi-review-host-api.md).
- **Known limitations (reference-host scope):** one clip at a time (no `Stop`/transport yet — the shell adds it); `Play`/`Splice`/`Bounce` are not logged events (the host reports the media command count; P1.3 merges them into the session log); the registry (`HOST_PLUGINS`/`HOST_PORTS`/`HOST_PARAMS`) is a closed per-slot vocabulary hand-maintained next to `register_factory` (derive from factory metadata when the surface grows); the text format is the wire schema (serde arrives with the Tauri adapter); `HostSession::engine()` is documented test scaffolding — a real host speaks `HostCommand`.
- **Next:** the Tauri v2 shell — thin adapter over the Host API + the Vue plugin runtime + the profile's UI plugins (canvas timeline ← pool + peaks, mixer panel ← meters, patch bay ← `providers_of`).
