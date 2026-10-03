# audio — an audio platform where everything is a plugin

> 🕒 Last verified against commit `2f6ad78` (2026-09-30). If the code has moved on,
> trust the code and move this line forward.

*(The repository is still named `sound-arranger`; the rename to **`audio`** is decided but not done —
[the profiles and umbrella-name note](.agents/notes/proposed/architecture/2026-09-27-profiles-and-the-umbrella-name.md).)*

A minimal core — **clock · audio-graph interpreter · session event log · context plumbing** — with
every capability as a **plugin**, so a product is an **assembled profile**. The session log is the
document: every edit is a line of `host v1` text, which is why a session replays byte-identically, a
seek is a rebuild, and the shells speak the same language the CLI does.

Three profiles are planned. One works today.

| Profile | What it is | State |
|---|---|---|
| **recorder** | send a clock to external gear, capture it, align the takes, master, export | **the focus** — capture, takes and MIDI clock-out work; alignment of takes is not built |
| **arranger** | record long live jams, then cut them into a finished piece (ACID-style clips) | works end to end |
| **sculptor** | offline, non-realtime transformation (CDP8, PaulStretch, phase vocoder) | deferred |

**Status: pre-alpha** (`v0.1.0-pre-alpha.1`). What the code does — and, more usefully, what it
deliberately does not — is [docs/capabilities.md](docs/capabilities.md), which keeps a
"known flaky and unfinished" list instead of a feature page.

## Try it

```sh
cargo test --workspace            # everything that needs no hardware
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p media -- --ignored  # hardware capture (needs a real input device)
```

A session **is** a script — the log is the command list:

```sh
# the generators through the mixer, bounced to a file:
printf 'host v1\nmount euclidean steps=8 pulses=5 @0\nmount scale root=0 @0\nmount tone @0\nmount mixer channels=2 @0\npatch euclidean.triggers scale.trigger @0\npatch scale.note tone.note @0\npatch tone.audio mixer.ch0 @0\nbounce 96000 /tmp/out.wav\n' | cargo run -p host

# the same language records, arranges and exports:
#   pool <dir>     adopt a directory of float-WAV sources (foreign rates are conformed once)
#   record <take>  capture the input device into the pool at the session rate
#   export <path>  the whole arrangement, f32 by default or dithered s16, never a clipped file
```

The shells are their own workspaces, so iced, wgpu and the terminal backend never enter the core
build. Both drive the same in-process host:

```sh
cd spikes/tui-shell  && cargo run     # the terminal shell — press ? for the keymap, q to quit
cd spikes/iced-shell && cargo run     # the iced shell (needs a display and an audio device)
```

Twenty minutes in the terminal workflow: [docs/tui-first-session.md](docs/tui-first-session.md).
Fifteen minutes of `host v1` with no UI at all: [docs/FIRST_SESSION.md](docs/FIRST_SESSION.md).

## What works, and what does not

**Works.** Recording a take from the input device into a float-WAV pool, with device-clock drift
compensation wired into the capture; import at any rate (conformed once to the session rate, the
original preserved); the clip editor — cut, copy, paste, move, trim, loop, chop, fade, gain, tracks,
snap grid with a bar/beat ruler, markers and clip names; the soft mixer (gain/pan/mute/solo, stereo
master) and a mastering chain (compressor plus lookahead brickwall limiter) on the bus; **MIDI
clock out** — a 24 PPQN, sample-accurate clock the engine renders and the host sends to a named
device (`--midi-out`), re-anchored on seek, with an offline bounce that never drives the gear;
export to f32 or dithered s16 that refuses to write a clipped file; sessions as directories with a
crash-safe per-gesture journal; undo/redo where one gesture is one entry; byte-identical replay,
save/load and export; and a jump into a 30-minute arrangement in about 0.1 s.

**Not yet.** The recorder's remaining defining capability — **alignment of takes** — is not built,
so the record → align loop has no second half yet (the clock is sent, but nothing aligns captured
material against it); there is no record *gesture* in the shells (a take is started from the `:`
command line); no effects (`fundsp` is an optional seam, off by default); OSC is a declared seam
with no implementation; no stereo *clip* (the master is stereo, stereo material is split into one
mono pool source per channel and panned); one sample rate per session; and a cold backward seek
into a long piece wants a checkpoint. Reasons, not just absences:
[docs/capabilities.md](docs/capabilities.md).

## Where the code is

| Crate | What it is |
|---|---|
| `crates/engine` | The minimal core: tempo map and sample-accurate scheduler, the typed-port patch-bay graph interpreter with plugin delay compensation, the session event log, context plumbing — plus the built-in plugins (euclidean, scale, tone, soft mixer, mastering `master`, MIDI `clock_out`). Std-only, allocation-free on the render path. |
| `crates/media` | The media engine (core-privileged, not a plugin): disk streaming, splice during playback, the recording writer with crash recovery, multi-channel capture, the float-WAV pool with peak pyramids, import/conform, a band-limited resampler, the clip model with its ACID ops and the arranger node, and the arrangement-command codec. |
| `crates/workflow` | The shell workflow defined once: the modal, key-driven editing model both shells implement, the `Action` vocabulary, the snap-grid state, and the keymap that generates `?` help. Toolkit-free, with a test proving every action claimed to be a log op is an op the host's parser accepts. |
| `crates/host` | The Host API contract — commands are logged events, plus events and values a shell reads — and the headless reference host: `run_script` assembles a profile and bounces byte-identically, while a live session takes incremental edits, gestures (one undo step each), sessions as directories, seeking at scale, recording, and export. |
| `crates/shell` | **Retired** (2026-09-22) — the Tauri + Vue shell, an approach tried and decided against. Frozen and excluded from the workspace, kept only until someone deletes it: [`crates/shell/RETIRED.md`](crates/shell/RETIRED.md). |
| `spikes/tui-shell` | The terminal shell (primary): arrangement view with braille envelopes, edits dispatched through the host's own `arrange` language, a console whose faders read back from the log, pool panel, mouse, deterministic `--dump`. |
| `spikes/iced-shell` | The iced shell (second): window, transport, meters following the audio, and a real mixer — `iced_audio` faders on a dB range, positions read from the host's parameter fold. |
| `plugins/` | External-integration home (placeholder): programs and devices we drive and record, not plugin binaries — see [plugins/README.md](plugins/README.md). |

## Documentation map

New here? **Operate the thing first** — [docs/tui-first-session.md](docs/tui-first-session.md), then
what it does and does not do ([docs/capabilities.md](docs/capabilities.md)) and the checklist a second
person runs ([docs/beta-acceptance.md](docs/beta-acceptance.md)). Then the host underneath it:
[docs/FIRST_SESSION.md](docs/FIRST_SESSION.md). Then learn the language through the codebase
([docs/rust-course/](docs/rust-course/README.md)), learn why it is shaped that way
([docs/architecture-explainer.md](docs/architecture-explainer.md)), and read the theory that makes it
one program ([docs/theory-of-the-program.md](docs/theory-of-the-program.md)). Only then do the
research and decision records read as consequences rather than claims.

- [docs/capabilities.md](docs/capabilities.md) — the capability and limitations statement, with the
  cuts that are decisions rather than omissions, and the measured numbers.
- [docs/clip-arranger.md](docs/clip-arranger.md) — the product side: the arrangement value, the
  editing ops, the render node, the pool, and the host wiring that makes edits reach audio.
- [docs/beta-acceptance.md](docs/beta-acceptance.md) — the checklist a second person runs from the
  docs alone.
- [.agents/notes/](.agents/notes/README.md) — decision records (Agent Notes), each with the
  alternatives that were rejected; standing orders in [AGENTS.md](AGENTS.md).
- [RESEARCH.md](RESEARCH.md) — working research: verified crate versions, the licensing matrix,
  latency notes, DAW prior art.
- [docs/design/](docs/design/) — the design of the shell we tried and decided against, kept as history. The Rust shells
  are the live UI.
- [docs/audio-latency.md](docs/audio-latency.md) — Linux kernel and userspace latency tuning;
  [docs/soft-synth-fundsp.md](docs/soft-synth-fundsp.md) — building native voices on fundsp.

Docs that explain the code carry a `Last verified against commit …` banner; if the code has moved on,
trust the code and move the line forward.

## Development

The toolchain is **rustup-managed** and declared in [`rust-toolchain.toml`](rust-toolchain.toml)
(`stable` plus rustfmt, clippy, rust-analyzer), so the compiler is a property of the repository rather
than of the distro package. On Arch:

```sh
sudo pacman -S --needed base-devel rustup alsa-lib    # rustup replaces the distro rust package
rustup default stable                                 # rust-toolchain.toml supplies the components
```

One-time git setup — the hooks live in the repository, so they need pointing at:

```sh
git config core.hooksPath .githooks
```

The pre-commit hook runs `cargo fmt --all --check` and then the Agent Note verifier, which checks the
notes' structure **and** that every cross-reference resolves. Each gate warns and skips when its tool
is missing, so a docs-only checkout can still commit — the authority is CI
([`.github/workflows/ci.yml`](.github/workflows/ci.yml): formatting, clippy with `-D warnings`, the
workspace tests, the notes verifier, and one job per shell spike, because `spikes/*` are separate
workspaces a root test run does not cover).

A change lands as a short-lived branch (`slice/…`, `fix/…`, `docs/…`) with a pull request, and merges
are **not squashed** — per-commit authorship and the `Assisted-by` trailers are this project's
attribution record. Docs and typo fixes may still go straight to `main`; releases are tags, never
branches ([the workflow note](.agents/notes/implemented/process/2026-09-27-git-and-ci-workflow.md)).

## How this is built

Decisions are recorded as dated [Agent Notes](.agents/notes/README.md), each stating the problem, the
decision, the **alternatives that were rejected and why**, and the consequences — including the
mistakes, which are recorded rather than tidied away. When two decisions conflict, **the later one
governs**: the theory of this program is being refined, so a superseded note is linked from the new
one rather than rewritten.

Development is model-assisted and disclosed per commit: the model is the **author**, the human is the
**committer**, and every agent commit carries an `Assisted-by` trailer naming the model and the
harness. Known-flaky tests are named in the capability page with what is known about them, rather than
quietly retried.

## Contributing

**Outside contributions are not accepted yet — not until there is a beta to contribute to.** This is a
deliberate policy rather than an oversight, so that the repository's own history says what it is before
anyone invests time in it.

- **Pull requests will be closed unmerged.** Not because the work is unwelcome, but because there is
  no stable API to build against: the architecture, the license and even the project's name are still
  moving. A patch against a surface that is about to change costs you more than it costs us.
- **Issues are welcome, and are the best way in.** [Bug reports and design
  objections](https://github.com/geokkjer/sound-arranger/issues) need no CLA and no commitment — a
  bug report against a pre-alpha build is genuinely useful, and it is the route by which a later
  contribution starts.
- **Fork it if you want to build on it.** The license already grants that, and nothing here
  restricts it. You simply cannot push back yet.
- **Why the gate exists, beyond focus:** while the human is the *sole copyright holder*, the project
  can still be relicensed in one step — and it currently intends to keep that option open. Accepting
  a first outside commit ends that permanently. So the gate lifts at beta, and a contribution policy
  (a lightweight DCO or CLA) will be chosen and written down *before* it lifts, not after the first
  PR arrives. The reasoning is in the [contributing note](.agents/notes/proposed/process/2026-09-30-no-outside-contributions-before-beta.md)
  and [RESEARCH.md](RESEARCH.md) §12.

## License

GPL-3.0-or-later ([LICENSE](LICENSE)) — rationale and the dependency compatibility matrix are in
[RESEARCH.md](RESEARCH.md) §12.
