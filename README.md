# sound-arranger (working title)

An **audio platform where everything is a plugin**: a minimal core — clock · audio graph
interpreter · session event log · context plumbing — with every capability as a plugin, and
the product an **assembled profile**. Rust (engine core + media engine + host), with a Tauri v2
+ Vue 3 shell wired to the Host API (a script-bridge command on the same versioned text format
the CLI drives).

## Scope (focused core)

**The core is one thing: a *liquid* audio editor / sampler / arranger / mixer.** Record long
**generative runs** — a eurorack patch or algorithmic source unfolding evolving patterns over
drones or stretched audio — then cut, splice, rearrange and mix them into a finished piece:
clips-as-objects, with a tape/edit-as-composition heritage (musique concrète, dub, ACID). This
is the monorepo for that core **and
its sidecar plugins** (`plugins/`): VST/CLAP builds and the CDP offline-process sidecar.

Deliberately **out of scope** for this repo (kept as separate projects, indexed by the
`~/Projects/music` view): the personal composition-theory corpus, the jam/recording rig, and
the Tidal live-coding tool (`tidal-lsp`) — those are not on the clip-arranger path.

> **Working title.** The repo name names the *first profile*, not the platform; a rename
> ("audio" / "sound") is an open question — see RESEARCH.md §14.

**Status: pre-alpha.** The audio core works and is tested; there is no product **UI** yet (the
Tauri seed is wired to the Host API, but the Vue surface is empty). What exists today: the
minimal core, the media engine (disk streaming, recording, splicing, multi-channel capture), the
soft mixer (gain/pan/mute/solo + stereo master), **the clip editor (P1.3 — value, ACID ops,
arranger node, media pool with crash recovery, the engine's closed-core message dispatch, the
logged-command codec, host wiring, and a text-format that drives it from the CLI)**,
a **headless reference host** that proves the UI-as-plugin contract end-to-end, and the
**Tauri→Host script-bridge** (one command — proof of the shell wiring, not a product UI).
What doesn't exist yet: a real UI surface, live-edit audio re-wiring, stereo *sources/clips*
(the master is stereo and mono channels pan into it, but a genuine stereo take/clip is still
forthcoming), effects, MIDI/OSC implementations, CLAP hosting, and the drain/EOF phase for
tailed effects. The direction is locked
([umbrella-first note](.agents/notes/proposed/architecture/2026-08-15-umbrella-first-product-direction.md)):

- **Profile #1 — the clip arranger ("sound-arranger"):** record long **generative runs**, then
  cut, splice, rearrange and mix them into a finished piece. ACID-style clips-as-objects with a
  tape/edit-as-composition heritage (musique concrète, dub, ACID) as inspiration, and a
  generative/modular "performer" (a eurorack patch or algorithm) rather than a human jam.
  Chosen first because it stresses the substrate end-to-end: recording, editing, mixing, the
  realtime path.
- **Deferred, own profile — "sound sculptor":** offline/non-realtime processing (CDP8,
  PaulStretch, Csound offline, phase-vocoder) as `OfflineProcess` plugins.

## Where the code is

| Crate | What it is | Status |
|---|---|---|
| `crates/engine` | The minimal core: clock (tempo map + sample-accurate scheduler), patch-bay graph interpreter (typed ports, PDC), session event log, context plumbing — plus plugins: euclidean, scale, tone, **soft mixer** (gain/pan/mute/solo, stereo master fader, meters). Audio ports are **channel-aware** (mono default; the mixer's master is stereo). Std-only. It also carries the **closed-core plugin-message dispatch** (`Event::Arrangement` + `arrange_logged`): profile-level ops are logged as commands the core understands without knowing them. | works, tested |
| `crates/media` | The media engine (core-privileged, not a plugin): disk streaming, splice-during-playback, recording writer with crash recovery, **device-clock drift compensation wired into the capture**, multi-channel capture → **float-WAV media pool with live peak pyramids**, pool **enumeration + crash recovery** (finalize un-finalized takes, rebuild `.peaks`), the **clip editor's value + ACID ops + `ArrangerNode`** (renders a track from the value), and the **`ArrangeOp ↔ engine-command` codec**. | works, tested |
| `crates/host` | The **Host API contract** (commands = logged events, events, values) + the **headless reference host**: `run_script` assembles the profile and bounces byte-identically — now including the **clip-arrangement commands** (`pool`/`arrange`) in the versioned **text format**, so the CLI smoke binary drives the clip editor end-to-end. It is also a **persistent live session** (`execute()` for incremental edits, `arrangement()` for a serializable snapshot a shell reads). The UI-as-plugin seam — the Tauri shell implements the same contract; swapping shells swaps only the transport adapter. | works, tested |
| `crates/shell` | The Tauri v2 + Vue 3 shell (scaffold): a script-bridge command (`run_host_script`) that runs the Host API's versioned text format over a live `HostSession`. The Vue component is still a stub — it proves host↔shell wiring, not a product UI. | scaffold |
| `plugins/` | **Sidecar plugins** (placeholder): VST3/CLAP builds of core capabilities and the CDP / offline-process sidecar (`OfflineProcess`). Not engine crates — thin wrappers that expose a capability to a plug-in API. See [plugins/README.md](plugins/README.md). | placeholder |

161 tests across the workspace; the core's invariants (byte-identical replay, no-allocation
render, sample-accurate lifecycle) are tested, and the streaming soak + real hardware capture
run as `#[ignore]`d tests.

**Honest gaps** (deliberate, pre-alpha): the master is **stereo** (the mixer pans mono channels
into L/R and the bounce is 2-channel) but stereo *sources/clips* are still forthcoming — a
genuine stereo take/clip is the next sub-step, using the per-port channel-count seam;
control-side mutations apply on the render call stack (the real control→render handoff is
seeded by `flush_scheduled`, not finished); the recorder's `play`/`splice` media commands are
not yet logged events (the **arrangement** ops *are*); **live-edit-while-playing reader reuse**
(edits after a bounce are rebuilt and readers are re-positioned at the transport frame, so they
reach audio and removed tracks no longer ghost — but each rebuild re-warms readers, and the
shared-state `ArrangerNode` reuse is still deferred); the
**drain/EOF phase** for tailed effects (reverb/delay/codec) is a proposed note;
MIDI/OSC are declared seams, not implementations; **no effects, no stereo *sources/clips*, and
no product UI surface** yet (the shell bridge runs a script but draws nothing).

## Try it

```sh
cargo test --workspace        # everything that doesn't need hardware
cargo clippy --workspace --all-targets

# hardware-dependent tests (this machine: a Scarlett 2i2):
cargo test -p media -- --ignored
cargo test -p media --release -- --ignored soak   # 25-minute stream+record+splice+bounce
```

The headless smoke binary speaks the same versioned command script the Tauri shell bridge runs
(the contract a UI sends — the log is the command list). It drives the clip arrangement too:

```sh
# a simple tone from the synth chain:
printf 'host v1\nmount mixer channels=4 @0\nbounce 512 /tmp/out.wav\n' | cargo run -p host

# a clip arrangement from a pool source (the clip editor):
#   pool <dir>s1.wav must exist; then arrange clips on tracks and bounce.
printf 'host v1\nmount mixer channels=2 @0\npool /data/takes\narrange add_track t0 @0\narrange add_clip t0 c0 s1 0 48000 0 0 0 1.0 @0\nbounce 48000 /tmp/out.wav\n' | cargo run -p host
```

## Documentation map

**New here?** Read in this order: first operate something
([docs/FIRST_SESSION.md](docs/FIRST_SESSION.md) — fifteen minutes, no
prerequisites), then learn the language through the codebase
([docs/rust-course/](docs/rust-course/README.md)), then learn why it is shaped
that way ([docs/architecture-explainer.md](docs/architecture-explainer.md)),
then read the *theory* that makes the whole thing one program
([docs/theory-of-the-program.md](docs/theory-of-the-program.md) — the what/why;
the explainer is the how).
Only then do the research and decision records make sense. To build on the
engine — e.g. design your own soft-synth voices on fundsp — read
[docs/soft-synth-fundsp.md](docs/soft-synth-fundsp.md) as a follow-on.

- [docs/clip-arranger.md](docs/clip-arranger.md) — the *product* side: the arrangement value,
  the ACID editing ops, the render node, the media pool, and the host wiring that makes edits
  reach audio.
- [docs/theory-of-the-program.md](docs/theory-of-the-program.md) — the *theory* of the program,
  in Peter Naur's sense (1985): why the whole thing coheres, and how the "understand the
  codebase" debate applies here.
- [docs/design/](docs/design/) — the Tauri/Vue UI design: `ui-plan.md` (interaction spec),
  `design-system.md` (token contract), `mocks/` (live mockups).
- [RESEARCH.md](RESEARCH.md) — working research & architecture (verified crate versions,
  licensing matrix, latency notes, plugin-architecture research, DAW prior art)
- [.agents/notes/](.agents/notes/README.md) — decision records (Agent Notes); standing
  orders in [AGENTS.md](AGENTS.md)
- [docs/audio-latency.md](docs/audio-latency.md) — Linux kernel/userspace latency tuning
- [docs/soft-synth-fundsp.md](docs/soft-synth-fundsp.md) — building native soft-synth
  voices on fundsp: the opaque-tier design, the feature flag, and the release-mode gotcha.
- [research/](research/) — dated research inputs (gear notes, external reviews)

Docs that explain the code carry a `Last verified against commit …` banner; if
the code has moved on, trust the code and move the line forward.

The surrounding ecosystem (composition-theory corpus, jam rig, sibling projects) is indexed
by the [`~/Projects/music`](../music/README.md) symlink view.

## Dev environment

**Native host toolchain** (Arch/CachyOS). Dev builds use the system Rust, `node`/`pnpm`, and the
**host** GTK/WebKit/Mesa. The Nix/devenv setup is parked under [`nix/`](nix/): on this non-NixOS
host a Nix-built GUI binary cannot open a window (the Nix glvnd ships no EGL vendor, and host WebKit
needs `GLIBC_2.44` vs the Nix toolchain's 2.42). The diagnosis and the decision are in the
[native host dev toolchain note](.agents/notes/implemented/process/2026-09-10-native-host-dev-toolchain.md).

Install (Arch):
```sh
sudo pacman -S --needed base-devel rust nodejs npm pnpm \
  webkit2gtk-4.1 gtk3 libsoup3 librsvg libayatana-appindicator \
  alsa-lib openssl appmenu-gtk-module
```

One-time git setup — hooks live in-repo:

```sh
git config core.hooksPath .githooks
```

The pre-commit hook verifies the Agent Notes tree and refuses edits under
`.agents/notes/archived/`. It needs `node` on `PATH`; without node it warns and skips
rather than blocking the commit.

## License

GPL-3.0-or-later ([LICENSE](LICENSE)) — rationale and the dependency compatibility matrix
are in RESEARCH.md §12.
