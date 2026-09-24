# sound-arranger (working title)

> 🕒 Last verified against commit `4bc44bd` (2026-09-24). If the code has
> moved on, trust the code and move this line forward.

An **audio platform where everything is a plugin**: a minimal core — clock · audio graph
interpreter · session event log · context plumbing — with every capability as a plugin, and
the product an **assembled profile**. Rust (engine core + media engine + host) and Rust shells over
the Host API — the same versioned text format the CLI drives. The shells are **iced** (GUI, with
`iced_audio` widgets) and **ratatui** (TUI), both in-process over the same host actor; the
**Tauri v2 + Vue 3 shell is retired** ([note](.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md),
[`crates/shell/RETIRED.md`](crates/shell/RETIRED.md)) — **the TUI is primary**, iced is the second
shell with one shared workflow. The interaction direction under evaluation: **modal editing — visual mode for clips —
with the `host v1` command language as the `:` prompt**, i.e. for audio what vim/helix/emacs is for
text ([note](.agents/notes/proposed/architecture/2026-09-21-modal-editing-model.md)).

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

**Status: pre-alpha, with a finish line.** The alpha scope, its order and what is deliberately cut are
in the [alpha finish-line note](.agents/notes/proposed/architecture/2026-09-23-alpha-finish-line.md),
reviewed by the co-work passes ([verbatim](research/architecture/2026-09-23-alpha-scope-co-work-reviews.md)).
Its most important finding: the substrate is further along than the capability list implies — but
**recording is not wired into the host** (`HostCommand::Record` is a stub), so the record → arrange loop
has no first half yet, and the session has no save/open.

The audio core works and is tested. **Recording works** and the pool reads 16/24-bit PCM and float WAVs, splitting a multi-channel import into one source per channel: `record <take_id>` captures the input device
into the pool at the session rate and `record stop` finalizes it ([note](.agents/notes/implemented/feature/2026-09-23-recording-into-the-host.md)).
What exists today: the minimal core,
the media engine (disk streaming, recording, splicing, multi-channel capture, pool rate
conformance), the soft mixer (gain/pan/mute/solo + stereo master), **the clip editor (P1.3 — value,
ACID ops, arranger node, media pool with crash recovery, the engine's closed-core message
dispatch, the logged-command codec, host wiring, and a text-format that drives it from the CLI)**,
a **headless reference host** that proves the UI-as-plugin contract end-to-end, and **two shell
spikes over it** — iced (transport, meters, a real `iced_audio` fader console) and ratatui (a
timeline with braille envelopes, clip edits through the host's own `arrange` language, modal
selection, a console whose faders read back from the log). The Tauri + Vue shell that carried the
four views, zoom/pan, clip editing and undo/redo is **retired and frozen** — it is the porting
reference, not the app.

What doesn't exist yet: effects (no `fundsp` effect plugins), MIDI/OSC implementations (declared
seams only), CLAP hosting, live-edit audio re-wiring, a genuine stereo *clip* (the master is
stereo, stereo **material** is split into one mono pool source per channel and panned, but one
clip is still one mono source), and **full UI wiring** — several surfaces are real
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
| `crates/engine` | The minimal core: clock (tempo map + sample-accurate scheduler), patch-bay graph interpreter (typed ports, PDC), session event log, context plumbing — plus plugins: euclidean, scale, tone, **soft mixer** (gain/pan/mute/solo, stereo master fader, meters) and **master** (a stereo-linked compressor + a 5 ms lookahead brickwall limiter on the bus, with peak and gain-reduction meters; `mount master` after the mixer, `patch mixer.audio master.audio`, then `set_param master threshold …`). Audio ports are **channel-aware**: a cord carries one channel count, the mixer's master is stereo, and a mismatch is refused rather than silently downmixed. Offline renders are **latency-aligned** (`render_with_drain_aligned` trims the graph's transit), so a lookahead effect does not push the bounce's head into silence. Std-only. It also carries the **closed-core plugin-message dispatch** (`Event::Arrangement` + `arrange_logged`): profile-level ops are logged as commands the core understands without knowing them. | works, tested |
| `crates/media` | The media engine (core-privileged, not a plugin): disk streaming, splice-during-playback, recording writer with crash recovery, **device-clock drift compensation wired into the capture**, multi-channel capture → **float-WAV media pool with live peak pyramids**, pool **enumeration + crash recovery** (finalize un-finalized takes, rebuild `.peaks`) **+ rate conformance** (`import`/`conform`: a source of any rate is resampled once to the session rate, original preserved — see the [session-rate note](.agents/notes/implemented/architecture/2026-09-22-session-rate-and-source-conversion.md)), a **band-limited resampler** (128-tap Kaiser polyphase), the **clip editor's value + ACID ops + `ArrangerNode`** (renders a track from the value), and the **`ArrangeOp ↔ engine-command` codec**. | works, tested |
| `crates/workflow` | **The shell workflow, defined once**: the modal, key-driven editing model both shells implement — modes, a toolkit-neutral `Key`, the `Action` vocabulary, the **snap-grid state** (which musical division an edit lands on; UI state, never logged), and the **keymap table that generates the `?` help**. It is UI-toolkit-free (its dependencies are `host`, whose `v1` vocabulary it is a keyboard face of, and `media`, whose grid math it names), and a test proves every action that claims to be a log op is an op the host's parser accepts — so a binding cannot drift from the log. See the [modal model](.agents/notes/proposed/architecture/2026-09-21-modal-editing-model.md) and the [shells note](.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md). | works, tested |
| `crates/host` | The **Host API contract** (commands = logged events, events, values — including `params`, the session's parameters folded from the log) + the **headless reference host**: `run_script` assembles the profile and bounces byte-identically — now including the **clip-arrangement commands** (`pool`/`arrange`) in the versioned **text format**, so the CLI smoke binary drives the clip editor end-to-end. It is also a **persistent live session** (`execute()` for incremental edits, `arrangement()` for a serializable snapshot a shell reads) with **gestures** (several arrangement ops in one `group begin` … `group end`, applied all-or-nothing and **one undo step**), **sessions as directories** (`save <dir>` writes the log as a `host v1` script plus its pool; the journal autosaves every gesture; `load <dir>` replays both) and **recording** (`record <take_id>` takes the input device into the pool at the session rate, `record stop` finalizes it) and **export** (`export <path> [f32|s16]`: the whole arrangement from frame 0, measured length, f32 by default, s16 through a fixed-seed TPDF dither, peak/RMS reported, and a refusal rather than a clipped file) — [gestures](.agents/notes/implemented/architecture/2026-09-23-compound-gestures-one-undo-step.md), [sessions](.agents/notes/implemented/architecture/2026-09-23-session-directory-save-and-journal.md), [recording](.agents/notes/implemented/feature/2026-09-23-recording-into-the-host.md). The UI-as-plugin seam — the iced and ratatui shells implement the same contract (the retired Tauri shell did too); swapping shells swaps only the transport adapter. | works, tested |
| `crates/shell` | **RETIRED (2026-09-22)** — the Tauri v2 + Vue 3 shell: the four views as components (`Surface`, `TimelineCanvas`, `SourcePool`, `MixerPanel`, `DetailView`), `bridge` (the Host API over Tauri), `transport`, `editor` (undo/redo replayed from the log) and the timeline viewport/editing maths with unit tests. Frozen and **excluded from the workspace** (no Tauri/npm in the default build); kept as the porting reference for the Rust shells. See [`crates/shell/RETIRED.md`](crates/shell/RETIRED.md). | retired / frozen |
| `spikes/iced-shell` | The **iced shell** (its own workspace, excluded from the core build): window, transport, channel/master meters following the audio, and **a real mixer** — `iced_audio 0.17` `VSlider` faders on a dB range, one per channel plus the master, positions read from the host's **parameter fold** (screenshot in its README). Drives the *same* `host::live::HostHandle` in-process: no IPC, no serde wire, no webview. Headless `--probe` asserts live meter signal; 2 tests pin the strip mapping and the dB range. iced 0.14 + `iced_audio` 0.17. | **second shell** (one workflow with the TUI; the GUI-only extras come later) |
| `spikes/tui-shell` | The **ratatui evaluation spike** (its own workspace): the terminal counterpart of the iced spike — transport, channel/master meters, position readout — over the same in-process `HostHandle`, plus an **arrangement view** (`--script <host script>` draws the engine's own clips and tracks: braille min/max envelopes per clip, per-track colour, boundaries, fades, ruler, zoom/scroll, playhead, **visual-mode selection**) with **edits dispatched through the host's own `arrange` text format** (`x` split, `d` delete, `n`/`N` clip motion, `u`/`Ctrl+r` undo/redo by log replay), a **right-hand console** whose faders are **read back from the log** (`params`), and **panel focus** (`Tab`). `--wave <file.wav>` imports the file into a session pool at the session rate, so a loaded 44.1 kHz file is **audible at the right pitch**. Mouse, per-command UI latency, a **scrollable `?` keymap**, deterministic `--dump`. **Move/trim by key**: `<`/`>` trim to the playhead, `t` trims to the selection, `H`/`L` nudge a grid step, `J`/`K` change track. **Snap grid**: `b` cycles off → bar → beat → 1/2 → 1/4, the ruler becomes numbered **bar/beat lines**, `[`/`]` step the playhead a grid line, and the frame an edit lands on is quantized through the session's tempo map before the command is built (a script snaps identically with `snap=<frames>`). **Clipboard**: `y`/`c` copy or cut the selection to a shell value (never logged), `p` pastes at the playhead and `P` appends after the track's last clip — one `group` of `add_clip`s with minted ids and 64-frame micro-fades on new boundaries, so a paste is one undo step. **Tracks**: `a` adds, `R` renames through the command line, `D` deletes a track with its clips as one gesture, `{`/`}` reorder — and because a track's position is its mixer channel, the console follows the order. **Pool panel**: the pool's sources are a panel under the timeline (entry/rate/length, crashed takes marked); `Enter` places the selected source on the active track at the playhead, creating the track when the session has none. **Utility gestures**: `V` reverses a clip (a clip property with a mirrored reader — split/trim/chop mirror their source arithmetic), `U` normalizes from the peak pyramid, `i` inverts polarity, `E` silences, `T` trims to the audible content. **Tempo match**: `: source_tempo <id> <bpm>` logs the tempo a take was performed at, and `W` **warps** the clip under the playhead to the session's tempo there — an offline WSOLA render into a new pool source (named `{source}.stretch.{src_start}_{src_len}.{num}_{den}` from the region actually read and the ratio — the region is in the name, so two clips of one take never overwrite each other, and the same warp twice reuses one file) followed by one logged `arrange stretch`, so the material is new but the clip stays a straight region read, pitch is preserved, and `u` restores the old reference. | **primary shell** (the workflow reference) |
| `plugins/` | **External-integration home** (placeholder, direction changed 2026-09-21): **no sidecar binaries and no plugin *host*** — capabilities that already exist as standalone programs (TidalCycles, VCV Rack, CDP8, sox, ffmpeg) are driven as *processes* and recorded into the pool, and hardware is synced and captured the same way. *Narrowed 2026-09-22:* exporting **our own instruments** as CLAP plugins is re-opened as a deferred option ([note](.agents/notes/proposed/architecture/2026-09-22-clap-export-via-nice-plug.md)). See [plugins/README.md](plugins/README.md) and the [no-sidecars note](.agents/notes/proposed/architecture/2026-09-21-external-programs-not-sidecars.md). | placeholder |

**The core workspace's Rust suites pass** (engine · media · host · workflow — 25 test binaries, 235
tests; the frontend tests retired with the shell, and both spike workspaces are outside the count:
`spikes/tui-shell` carries 29 view/input tests and `spikes/iced-shell` 5). The core's invariants
(byte-identical replay, no-allocation render, sample-accurate lifecycle) are tested, and the streaming soak + real hardware
capture run as `#[ignore]`d tests.

**Honest gaps** (deliberate, pre-alpha): the master is **stereo** (the mixer pans mono channels
into L/R and the bounce is 2-channel) and stereo *material* is kept whole — a multi-channel file is
split at import into `{id}.ch0`/`{id}.ch1` and placed on panned tracks — but a genuine stereo
*source/clip* (one source, two channels, the per-port channel-count seam) is still forthcoming;
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
#   pool <dir>: adopting it resamples any foreign-rate source to the session rate
#   (`s1.wav` must exist); then arrange clips on tracks and bounce.
printf 'host v1\nmount mixer channels=2 @0\npool /data/takes\narrange add_track t0 @0\narrange add_clip t0 c0 s1 0 48000 0 0 0 1.0 @0\nbounce 48000 /tmp/out.wav\n' | cargo run -p host

# Export the whole arrangement (f32 by default, `s16` for a dithered 16-bit file):
# the length is measured, peak/RMS are reported, and a mix that would clip is refused.
printf 'host v1\nmount mixer channels=2 @0\npool /data/takes\narrange add_track t0 @0\narrange add_clip t0 c0 s1 0 48000 0 0 0 1.0 @0\nexport /tmp/mix.wav s16\n' | cargo run -p host
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
**host** Mesa (and, only for the retired shell, GTK/WebKit); the **Rust toolchain is rustup-managed**, declared in
[`rust-toolchain.toml`](rust-toolchain.toml) (`stable` + rustfmt/clippy/rust-analyzer), so the
compiler is a property of the repo rather than of the distro package. The Nix/devenv setup is
parked under [`nix/`](nix/): on this non-NixOS host a Nix-built GUI binary cannot open a window (the
Nix glvnd ships no EGL vendor, and host WebKit needs `GLIBC_2.44` vs the Nix toolchain's 2.42). The
diagnosis and the decisions are in the
[host toolchain note](.agents/notes/implemented/process/2026-09-10-native-host-dev-toolchain.md)
and the [rustup note](.agents/notes/implemented/process/2026-09-21-rustup-managed-toolchain.md).

Install (Arch) — `rustup` **replaces** the distro `rust` package (they conflict); the retired
shell's GTK/WebKit and `pnpm` are only needed if it is ever resurrected:
```sh
sudo pacman -S --needed base-devel rustup alsa-lib
# for the retired Tauri shell only:
#   sudo pacman -S nodejs npm pnpm webkit2gtk-4.1 gtk3 libsoup3 librsvg libayatana-appindicator openssl appmenu-gtk-module

rustup default stable     # the repo's rust-toolchain.toml then supplies the components
```

The **shells** are their own workspaces, so iced, wgpu and the terminal backend never enter
the core build. Both drive the same in-process `HostHandle`:

```sh
cd spikes/iced-shell           # iced 0.14 + iced_audio 0.17 — a native window
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
