# Agent Note: UI as a plugin — the Host API contract, the headless reference host, and Tauri as the first rich shell

Status: proposed

## Problem

The project began as a tightly coupled UI sound-clip arranger; the everything-is-a-plugin discipline (composition-seams note) says UI is a capability too, not the substrate. Ardour's postmortems are the anti-pattern: session model and GUI grew together, and extracting the seams took years. The UI must be **interchangeable** — our Tauri app is a *reference implementation* of the host, not the host. The composition-seams note already demands the two halves of this: "the IPC command list is written down" and "a headless smoke binary records and bounces without the frontend." This note owns the decision: the seam and its first reference host.

## Proposal

1. **The Host API is the core↔host seam** — a typed, versioned contract of **commands, events, and values**. It is not a thick adapter and not a plugin; it is the host boundary the core exposes to *any* shell:
   - **Commands** — one per logged event, 1:1: `Mount`, `Patch`, `SetParam`, `SetTempo`, `Unmount`, plus the media/profile commands (`Play`, `Splice`, `Bounce`; `RecordStart`/`RecordStop` declared for the device path). *The log is the command list* — a command maps to a logged event (engine side) or a documented command type (media side until P1.3 merges them into the log).
   - **Events** — core → host: the append-only log stream, meters (`mixer.meters`), peaks, transport. The host *renders* events; it never computes them.
   - **Values** — declarative snapshots the host interprets: the graph value (nodes/ports/cords), `providers_of` (the patch UI's dropdown — already built), the pool index. The host never mutates shared state directly.
   - Versioned + schema'd from event #1 (minimal-core note's log rule). The engine's existing public API *is* the seed (`lib.rs`: "the seed of the core plugin contract").
2. **Two connection modes** (the answer to "layer or direct"): **engine plugins connect directly** (in-process, `Plugin` trait, `ctx`, the log — unchanged); **host/UI plugins connect through the Host API** (control-rate, cross-boundary). Rationale: the UI thread must never touch the render path, and a real boundary is what makes the shell interchangeable.
3. **The headless reference host first** (`crates/host`): a `HostCommand` list → `run_script` (assembles the profile: registers factories, mounts the mixer, wires players/capture, applies commands at their frames, renders, bounces) + a CLI binary reading a script file/stdin. It exercises the *entire* contract deterministically, CI-tested — the composition-seams "headless smoke binary" acceptance, and the proof that the contract is real (a contract no second host has exercised is a wish).
4. **Tauri v2 as the first rich reference shell** (after the headless host passes): the Tauri Rust side is a **thin transport adapter** (Tauri commands ⇄ Host API); the Vue frontend is the **host-side plugin runtime** — the declarative loader (rows + layered patches + reversible registrations, composition-seams note), the profile's UI plugins, and the renderers (canvas timeline ← pool sources + peaks; mixer panel ← meters; patch bay ← `providers_of`). `engine`/`media` never depend on `tauri`; the dependency points the other way. Swapping shells = swapping the transport adapter; the contract, the profile logic, and the core are untouched. The profile ("sound clip arranger mixer sampler editor") is an assembled composition of engine plugins + UI plugins on the contract — identical logic headless or graphical.

## Alternatives considered

- **Tauri-first (contract as the Tauri command surface)** — faster to a GUI, but "interchangeable" would rest on one shell. Rejected: the headless host is the cheaper proof of the contract and its deterministic test harness.
- **egui / iced in-process shell** — simpler, no IPC, but the boundary becomes voluntary (a UI thread could touch engine internals) and the cross-process plugin discipline weakens. A later second host, not the reference.
- **WASM + WebAudio browser shell** — the engine compiles to WASM; realtime constraints differ. A later host for sharing, not the reference.
- **Terminal (ratatui)** — a good later second reference (the headless CLI with a face).
- **Electron** — heavyweight, no reason over Tauri.

## Acceptance criteria

- `crates/host` runs a full script (chain mount/patch, play + splice, `set_param` mid-session, bounce) **twice → byte-identical output** (determinism across the contract).
- Every `HostCommand` has a test; the dumped log reflects exactly the commands applied; refused commands are refused identically on both runs.
- The contract declares the schema/versioning rule (from event #1); `engine`/`media` compile with no `tauri` dependency.
- The headless smoke binary records and bounces without a frontend (composition-seams acceptance) — exercised in CI.

## Risks

- **Contract churn** — versioning from event #1 + the log-as-command-list discipline contain it; the headless host's byte-identical replay tests make schema changes visible.
- **Ceremony without payoff** — the budget rule: the contract earns its keep via exactly two hosts (headless reference + Tauri); no more machinery than that.
- **Thick UI plugins** — the profile's *logic* must live in the shared contract layer, not in Vue components; the headless host running the same script is the guard.
