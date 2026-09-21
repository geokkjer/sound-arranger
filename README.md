# sound-arranger (working title)

> 🕒 Last verified against commit `992c4d4` (2026-09-21). If the code has
> moved on, trust the code and move this line forward.

An **audio platform where everything is a plugin**: a minimal core — clock · audio graph
interpreter · session event log · context plumbing — with every capability as a plugin, and
the product an **assembled profile**. Rust (engine core + media engine + host), driven by a
Tauri v2 + Vue 3 shell over the Host API (the same versioned text format the CLI drives) — with
**iced** and a **ratatui TUI** under evaluation as in-process Rust shells that would remove the
webview, the IPC wire and TypeScript entirely ([RESEARCH §4.5–4.6](RESEARCH.md)). The interaction
direction under evaluation with them: **modal editing — visual mode for clips — with the `host v1`
command language as the `:` prompt**, i.e. for audio what vim/helix/emacs is for text
([note](.agents/notes/proposed/architecture/2026-09-21-modal-editing-model.md)).

## Scope (focused core)

**The core is one thing: a *liquid* audio editor / sampler / arranger / mixer.** Record long
**generative runs** — a eurorack patch or algorithmic source unfolding evolving patterns over
drones or stretched audio — then cut, splice, rearrange and mix them into a finished piece:
clips-as-objects, with a tape/edit-as-composition heritage (musique concrète, dub, ACID). This
is the monorepo for that core **and
its integrations**
(`plugins/`): external programs and devices we sync and record, not plugin binaries —
see the [no-sidecars note](.agents/notes/proposed/architecture/2026-09-21-external-programs-not-sidecars.md).

Deliberately **out of scope** for this repo (kept as separate projects, indexed by the
`~/Projects/music` view): the personal composition-theory corpus, the jam/recording rig, and
the Tidal live-coding tool (`tidal-lsp`) — those are not on the clip-arranger path.

> **Working title.** The repo name names the *first profile*, not the platform; a rename
> ("audio" / "sound") is an open question — see RESEARCH.md §14.

**Status: pre-alpha.** The audio core works and is tested, and the shell is no longer a stub —
it is a working timeline editor. What exists today: the minimal core, the media engine (disk
streaming, recording, splicing, multi-channel capture), the soft mixer (gain/pan/mute/solo +
stereo master), **the clip editor (P1.3 — value, ACID ops, arranger node, media pool with crash
recovery, the engine's closed-core message dispatch, the logged-command codec, host wiring, and
a text-format that drives it from the CLI)**, a **headless reference host** that proves the
UI-as-plugin contract end-to-end, and a **Tauri v2 + Vue 3 shell** carrying the four planned
views (source pool · timeline canvas · mixer panel · detail view), transport, playhead
following, timeline zoom/pan, clip editing (select · move · resize · razor), and undo/redo
replayed from the host's session log.

What doesn't exist yet: effects (no `fundsp` effect plugins), MIDI/OSC implementations (declared
seams only), CLAP hosting, live-edit audio re-wiring, stereo *sources/clips* (the master is
stereo and mono channels pan into it, but a genuine stereo take/clip is still forthcoming),
and **full UI wiring** — several surfaces are real
components with placeholder bodies (the Detail View's three contexts are tabs whose bodies are
sketched pending selection wiring; record is still chrome). The direction is locked
([umbrella-first note](.agents/notes/proposed/architecture/2026-08-15-umbrella-first-product-direction.md)):

- **Profile #1 — the clip arranger ("sound-arranger"):** record long **generative runs**, then
  cut, splice, rearrange and mix them into a finished piece. ACID-style clips-as-objects with a
  tape/edit-as-composition heritage (musique concrète, dub, ACID) as inspiration, and a
  generative/modular "performer" (a eurorack patch or algorithm) rather than a human jam.
  Chosen first because it stresses the substrate end-to-end: recording, editing, mixing, the
  realtime path.
- **Deferred, own profile — "sound sculptor":** offline/non-realtime processing (CDP8,
  PaulStretch, Csound offline, phase-vocoder) as `OfflineProcess` plugins. **This is a *scope*
  deferral, not a gate**: the trait is already defined, and building CDP8 as a *spoke* requires
  no profile change at all. Which profile surfaces it is decided when that work starts — see
  [the phase note](.agents/notes/proposed/process/2026-09-10-phase-sequence-is-a-plan-not-a-gate.md).

## Where the code is

| Crate | What it is | Status |
|---|---|---|
| `crates/engine` | The minimal core: clock (tempo map + sample-accurate scheduler), patch-bay graph interpreter (typed ports, PDC), session event log, context plumbing — plus plugins: euclidean, scale, tone, **soft mixer** (gain/pan/mute/solo, stereo master fader, meters). Audio ports are **channel-aware** (mono default; the mixer's master is stereo). Std-only. It also carries the **closed-core plugin-message dispatch** (`Event::Arrangement` + `arrange_logged`): profile-level ops are logged as commands the core understands without knowing them. | works, tested |
| `crates/media` | The media engine (core-privileged, not a plugin): disk streaming, splice-during-playback, recording writer with crash recovery, **device-clock drift compensation wired into the capture**, multi-channel capture → **float-WAV media pool with live peak pyramids**, pool **enumeration + crash recovery** (finalize un-finalized takes, rebuild `.peaks`), the **clip editor's value + ACID ops + `ArrangerNode`** (renders a track from the value), and the **`ArrangeOp ↔ engine-command` codec**. | works, tested |
| `crates/host` | The **Host API contract** (commands = logged events, events, values) + the **headless reference host**: `run_script` assembles the profile and bounces byte-identically — now including the **clip-arrangement commands** (`pool`/`arrange`) in the versioned **text format**, so the CLI smoke binary drives the clip editor end-to-end. It is also a **persistent live session** (`execute()` for incremental edits, `arrangement()` for a serializable snapshot a shell reads). The UI-as-plugin seam — the Tauri shell implements the same contract; swapping shells swaps only the transport adapter. | works, tested |
| `crates/shell` | The Tauri v2 + Vue 3 shell — now a **working timeline editor**, not a stub: the four planned views as components (`Surface`, `TimelineCanvas`, `SourcePool` with Project/Library contexts, `MixerPanel`, `DetailView`), plus `bridge` (the Host API over Tauri), `transport`, `editor` (undo/redo replayed from the host's session log) and the timeline viewport/editing maths with unit tests. The Detail View is present with its three context tabs, but its bodies are placeholders pending selection wiring. | works; UI wiring in progress |
| `spikes/iced-shell` | The **iced evaluation spike** (its own workspace, excluded from the core build): a minimal second shell — window, transport, channel/master meters following the audio — driving the *same* `host::live::HostHandle` as the Tauri bridge, but in-process: no IPC, no serde wire, no webview. Ships a headless `--probe` mode that asserts live meter signal. iced 0.14. | spike; decision open — delete the directory to drop the option |
| `spikes/tui-shell` | The **ratatui evaluation spike** (its own workspace): the terminal counterpart of the iced spike — transport, channel/master meters, position readout — over the same in-process `HostHandle`, plus an **arrangement view** (`--script <host script>` draws the engine's own clips and tracks: braille min/max envelopes per clip, per-track colour, boundaries, fades, ruler, zoom/scroll, playhead, **visual-mode selection**) with **edits dispatched through the host's own `arrange` text format** (`x` split, `d` delete, `n`/`N` clip motion) and **panel focus** (`Tab`). `--wave <file.wav>` gives the one-file view. Mouse, per-command UI latency, `?` keymap, deterministic `--dump`. | spike; decision open — delete the directory to drop the option |
| `plugins/` | **External-integration home** (placeholder, direction changed 2026-09-21): **no VST/CLAP builds and no CDP sidecar binary** — capabilities that already exist as standalone programs (TidalCycles, VCV Rack, CDP8, sox, ffmpeg) are driven as *processes* and recorded into the pool, and hardware is synced and captured the same way. See [plugins/README.md](plugins/README.md) and the [note](.agents/notes/proposed/architecture/2026-09-21-external-programs-not-sidecars.md). | placeholder |

**191 Rust tests + 38 frontend unit tests**, all passing (the two spike workspaces are outside that
count — `spikes/tui-shell` carries 5 view/input tests of its own); the core's invariants (byte-identical
replay, no-allocation render, sample-accurate lifecycle) are tested, and the streaming soak +
real hardware capture run as `#[ignore]`d tests.

**Honest gaps** (deliberate, pre-alpha): the master is **stereo** (the mixer pans mono channels
into L/R and the bounce is 2-channel) but stereo *sources/clips* are still forthcoming — a
genuine stereo take/clip is the next sub-step, using the per-port channel-count seam;
control-side mutations apply on the render call stack (the real control→render handoff is
seeded by `flush_scheduled`, not finished); the recorder's `play`/`splice` media commands are
now **logged events** (the [media-commands note](.agents/notes/implemented/architecture/2026-09-12-media-commands-logged.md));
`record` stays unwired; **live-edit-while-playing reader reuse**
(edits after a bounce are rebuilt and readers are re-positioned at the transport frame, so they
reach audio and removed tracks no longer ghost — but each rebuild re-warms readers, and the
shared-state `ArrangerNode` reuse is still deferred); the
the **drain/EOF phase** for tailed effects shipped (a bounce renders stateful nodes'
`has_tail()` tails and flushes in-flight PDC, with a `capped` fail-loud bound) — realtime
transport stop does not drain yet, and the drain is not a logged event;
MIDI/OSC are declared seams, not implementations; **no effects**; and in the UI, **selection is
not wired** — the Detail View's three contexts are tabs with placeholder bodies, and record
remains chrome.

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

**Native host toolchain** (Arch/CachyOS). Dev builds use the system `node`/`pnpm` and the
**host** GTK/WebKit/Mesa; the **Rust toolchain is rustup-managed**, declared in
[`rust-toolchain.toml`](rust-toolchain.toml) (`stable` + rustfmt/clippy/rust-analyzer), so the
compiler is a property of the repo rather than of the distro package. The Nix/devenv setup is
parked under [`nix/`](nix/): on this non-NixOS host a Nix-built GUI binary cannot open a window (the
Nix glvnd ships no EGL vendor, and host WebKit needs `GLIBC_2.44` vs the Nix toolchain's 2.42). The
diagnosis and the decisions are in the
[host toolchain note](.agents/notes/implemented/process/2026-09-10-native-host-dev-toolchain.md)
and the [rustup note](.agents/notes/implemented/process/2026-09-21-rustup-managed-toolchain.md).

Install (Arch) — `rustup` **replaces** the distro `rust` package (they conflict):
```sh
sudo pacman -S --needed base-devel rustup nodejs npm pnpm \
  webkit2gtk-4.1 gtk3 libsoup3 librsvg libayatana-appindicator \
  alsa-lib openssl appmenu-gtk-module

rustup default stable     # the repo's rust-toolchain.toml then supplies the components
```

Frontend checks, from `crates/shell`: `pnpm typecheck` · `pnpm test` · `pnpm build`
(and `pnpm tauri dev` to run the app).

The **spike shells** are their own workspaces, so iced, wgpu and the terminal backend never enter
the core build. Both drive the same in-process `HostHandle` the Tauri shell reaches over IPC:

```sh
cd spikes/iced-shell           # iced 0.14 — a native window
cargo run                      # the window (needs a display and an audio device)
cargo run -- --probe           # headless proof: host thread + transport + live meters

cd spikes/tui-shell            # ratatui 0.30 — a terminal
cargo run                      # the TUI (press ? for the keymap, q to quit)
cargo run -- --wave f.wav      # …with a timeline for a real audio file (v selects)
cargo run -- --dump            # render one deterministic frame as text, no TTY needed
cargo run -- --probe           # the same headless proof
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
