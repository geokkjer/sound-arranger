# Building our own soft-synth voices (fundsp)

> 🕒 Last verified against commit `2f6ad78` (2026-09-30). If the code has moved on, trust the code and move this line forward.

This is the human-facing guide to the platform's first **native soft synth**: how we build our own voices
in the Rust engine on top of the [**fundsp**](https://crates.io/crates/fundsp) DSP library, how to turn it
on and prove it works, and the **release-mode gotcha** that bit us the first time we built the engine in release.

No prior synthesizer DSP knowledge is assumed — only that you can read a little Rust and run a `cargo`
command. If you haven't yet, do the [15-minute tour](FIRST_SESSION.md) and skim the
[architecture explainer](architecture-explainer.md) first: this doc is a *builder's* topic, not an
entry point.

---

## What this is

We want the ability to design instruments — pick an oscillator (VCO), a filter, an envelope, an LFO — and
compose them into a synth that mounts in the patch bay like any other sound source. Originally we wondered
whether SuperCollider or Csound covered this. They don't, in the way we need: they're external **synthesis
servers/languages** (SuperCollider = a server you talk to over OSC; Csound = a C library whose instruments
are written in its own opcode language). Neither is a Rust library you import a VCO/filter from and compose
in our engine. So we adopt **fundsp** for the DSP blocks, and — this is the important part — keep **our own
node/graph model** as the abstraction, not fundsp's.

See [RESEARCH.md §10](../RESEARCH.md) for the full ecosystem survey and the decision
([native-soft-synth note](../.agents/notes/proposed/architecture/2026-08-30-native-soft-synth-building-blocks.md)).

### The shape of it (why fundsp is a *block* source, not our graph)

The engine's node model is two-tier: **declarative** nodes (serializable values: gain, playback, routing) and
**opaque** nodes (`NodeKind::Opaque(Box<dyn AudioNode>)` — trusted in-process code). fundsp lives entirely in
the opaque tier. Our patch stays a **loggable, diffable value** (that's a core invariant: the same session log
reproduces the same audio). fundsp's graph is the opposite — it's compiled into Rust types/closures, acyclic,
**not serializable as data**, and it has **no note/event type**. So we use fundsp for per-sample algorithms and
keep scheduling, envelopes, and voices in our own layer.

Concretely, [`crates/engine/src/plugins/fundsp_synth.rs`](../crates/engine/src/plugins/fundsp_synth.rs)
wraps one fundsp `Box<dyn AudioUnit>` graph — `sine() >> lowpass_hz(cutoff, q)` — as an opaque `AudioNode`.
fundsp owns the oscillator and filter DSP; **we** own:

- **Note scheduling** — a note arrives as a `NoteEvent` with an *offset within the block*; we start the voice
  at that exact sample (sample-accurate, never wall-clock).
- **The envelope** — a simple linear decay (this is a blip-era proof, not a finished synth).
- **Gain**, and the `set_param` surface (`gain`, `env_len`) that is logged like every other param.

The voice mounts as a patch-bay provider with ports `note` (in) and `audio` (out), like the existing `tone`.

---

## Turning it on and proving it works

fundsp is an **optional, feature-gated** dependency of the minimal core, so the core stays dep-lean by default.
`cargo test -p engine` (no feature) builds without fundsp at all.

```sh
# Run the fundsp-backed soft-synth test suite (9 tests) + the rest of the engine:
cargo test -p engine --features fundsp

# Same, optimized (important — see the gotcha below):
cargo test -p engine --features fundsp --release

# Lint with the feature on:
cargo clippy -p engine --features fundsp --all-targets
```

The 9 tests prove the claims we actually care about:

| Test | What it proves |
|---|---|
| `fundsp_voice_sounds` | the voice produces audio |
| `fundsp_pitch_follows_scale` | pitch flows through (an octave ≈ double the zero-crossings) |
| `fundsp_onset_is_sample_accurate` | no sound before the trigger frame |
| `fundsp_replay_is_byte_identical` | same log ⇒ same audio |
| `fundsp_replay_includes_set_param` | a mid-session `set_param` replays identically |
| `fundsp_set_param_is_logged_and_validated` | params are logged; undeclared params are refused |
| `fundsp_render_does_not_allocate` | the render path allocates nothing |
| `fundsp_latency_is_reported_for_pdc` | the node reports its latency for plugin-delay compensation |
| `fundsp_voice_tail_is_drained` | a decaying voice's tail is rendered past the last note, not cut |

The feature flag lives in [`crates/engine/Cargo.toml`](../crates/engine/Cargo.toml): `fundsp` is
`optional = true`, `default-features = false`, `features = ["std"]`. We use only two primitives today
(`sine()` and `lowpass_hz`); the flags keep the heavy optional deps (`symphonia`, `fft-convolver`) out.

---

## ⚠️ The release-mode gotcha (the first time we built release)

The very first `--release` run of the fundsp voice was **silent** — and it wasn't fundsp's fault. It was a
pre-existing engine bug that only shows in release, and it affects *every* mounted path, not just this voice.

**Root cause:** the mount arm of `Engine::apply_event`
([`crates/engine/src/render.rs`](../crates/engine/src/render.rs)) wrapped the apply in a `debug_assert!`:

```rust
SchedEvent::Mount { plugin, params } => {
    debug_assert!(self.apply_mount(plugin, &params).is_ok(), "...");
}
```

`debug_assert!` **doesn't evaluate its argument in release builds** — so in release, `apply_mount` was never
called. No scheduled mount was ever applied, so no node was ever added to the graph, and the engine rendered
silence on every mounted path. In debug it was hidden (the argument *is* evaluated). The pure tone chain
test `chain_trigger_is_sample_accurate` failed in release too — no DSP involved.

**Fix:** call `apply_mount` in both builds and assert on the result, matching the sibling
`Patch`/`SetParam`/`Arrangement` arms:

```rust
SchedEvent::Mount { plugin, params } => {
    if let Err(e) = self.apply_mount(plugin, &params) {
        debug_assert!(false, "scheduled mount must apply (log was validated): {e}");
    }
}
```

See the [bug-fix note](../.agents/notes/implemented/bug-fix/2026-08-30-scheduled-mounts-apply-in-release.md).

**How that arm has grown since.** The 2026-09-29 hardening pass changed what a
refusal there *means*: `apply_mount` is now transactional (a refused apply
undoes what it touched — the name is free again, the next mount succeeds), and
the refusal is **recorded, not asserted**: it becomes an `ApplyFault` whose
`String` is *moved* off the render path without allocating, visible in release
builds, and `apply_faults()` / `is_degraded()` surface it. The lesson below is
unchanged — but its enforcement is now data, not a `debug_assert!`.

**The lesson for anyone touching the engine:** always test **release**, not just debug. Debug-only `debug_assert!`
gating of a real operation is a silent release bug. A quick way to keep it honest:

```sh
cargo test -p engine --release
```

---

## What's next (not yet built)

This is a **proof of concept**, not a finished instrument. Recorded in the
[native-soft-synth note](../.agents/notes/proposed/architecture/2026-08-30-native-soft-synth-building-blocks.md):

- **Polyphony** — today it's monophonic (a new note retriggers the one voice). Next: a preallocated voice
  pool with deterministic voice-stealing and a per-voice envelope.
- **Click-free retrigger** — the spike hard-resets the fundsp filter on every onset, so rapid notes click.
  A crossfade on retrigger fixes it.
- **Cutoff automation** — `cutoff`/`q` are baked at mount and refused by `set_param`. A real synth wants a
  runtime cutoff control (fundsp `shared`/`var`).
- **Extract to a crate** — fundsp pulls ~12 crates even with `default-features = false`. The feature-gate is
  a fine bridge; long-term the voice belongs in a separate `crates/fundsp-synth` that depends on `engine`, so
  the minimal core stays truly dep-lean.

**One honest caveat — closed 2026-09-29:** this doc originally warned that "a
scheduled mount must apply" was enforced only by tests, because
`validate_mount` injected but never applied, so a plugin whose `apply` returned
`Err` would be silently skipped in release. The hardening pass closed it —
recorded as an update to
[the same bug-fix note](../.agents/notes/implemented/bug-fix/2026-08-30-scheduled-mounts-apply-in-release.md):
`apply_mount` is transactional, a refusal is recorded as an `ApplyFault` that is
loud in **every** build, and the euclidean plugin's `apply` re-checks its
`steps` bound — the door the caveat warned about is now alarmed.

---

## License & law

fundsp is MIT OR Apache-2.0 — permissive, and it composes cleanly with our GPL-3.0-or-later app
(see [RESEARCH.md §12](../RESEARCH.md)). The license matrix there lists fundsp as ✅.

*Authored with deepseek-v4-flash-vision-exp · DeepSeek Harness, 2026-08-30; re-verified
against `2f6ad78` with GLM-5.3 · OpenCode, 2026-09-30.*
