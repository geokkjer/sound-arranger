# Theory of the program: sound-arranger

> 🕒 Last verified against commit `2f6ad78` (2026-09-30). If the code has
> moved on, trust the code and move this line forward.

> *Authored with DeepSeek-V4-Flash · DeepSeek Harness, 2026-09-05.*

This is an attempt to write down **the theory of this program** in Peter Naur's
sense ([*Programming as Theory Building*, 1985](https://pages.cs.wisc.edu/~remzi/Naur.pdf)).
It is not a duplicate of the architecture explainer (which walks *how the code
is written*) or of the decision notes (which record *what was decided and why*).
It is the layer above both: **the coherent mental model that makes this
monorepo one thing rather than a pile of crates**, plus the reasons each part is
shaped the way it is.

A caveat up front, drawn from the paper itself: this is a theory *of* the
program, reconstructed from its artifacts at a single commit. It is *not* the
same theory that lives in the original author's head — the thought processes
that produced the code are not in the code, and Naur (via Ryle) would insist
they are not recoverable from the text. What follows is a faithful, useful
reconstruction; it is deliberately not presented as the only one.

---

## 0. What a "theory of the program" means (the 1985 paper in one page)

Naur's claim is that programming is not the production of program text; it is
the building up, in the minds of the programmers, of a **theory** — an
explanatory model of how the program handles some part of the world. The code,
the docs, the specs are **by-products**. The theory is primary.

"Theory" here is Gilbert Ryle's sense (Naur cites *The Concept of Mind*): a
possession of *know-how* as well as *know-that*. Someone with the theory can:

1. **explain how the solution relates to the world** it handles;
2. **explain why each part of the program is what it is** (justify the text);
3. **respond constructively to modification demands** — and crucially, judge
   whether a proposed change *fits* the theory or is an unintegrated patch.

The theory is largely **not expressible as rules**. That is why, Naur says,
restoring the theory from documentation alone is strictly impossible, and why a
program "dies" when the team that holds its theory dissolves — the code still
runs, but no one can intelligently answer a modification demand. His (contested)
prescription: in preference to revival, discard the text and let a team solve
the problem afresh.

The two blog posts you read are a live argument about this:

- **Sean Goedecke** ("In defense of not understanding your codebase") says the
  theory can in practice be rebuilt from code; that large systems force partial
  understanding (everyone holds an *incorrect* theory); that Naur's "scrap and
  rewrite" is unworkable at scale; and that "maintain a theory" is one value
  among many, tradeable for speed, compliance, politics.
- **Jani Hartikainen** ("In defense of understanding the theory of the program")
  replies that recovering the *original* theory is philosophically impossible
  (it's know-how, not rules); that theory building is not source-code-level only
  (versions, dependencies, accessibility, the lunch conversation are all part
  of it); and therefore that you cannot simply "skip" or "trade off" it, because
  those other values *are* part of the theory.

Keep that debate in your back pocket. This program is a small, pre-alpha
codebase with an unusually deliberate *written* theory — so Goedecke's
large-scale objection doesn't bite here (the whole thing fits in one head), and
Hartikainen's point is exactly what the repo's note-and-banner discipline is an
attempt to live by. See §7.

---

## 1. The affairs of the world this program handles (the problem theory)

Before the code, there is a claim about what the musician needs. That claim is
the "theory of the problem," and everything in the code is in service of it.

> **The problem.** Record **long generative runs** — a eurorack patch or
> compositional algorithm unfolding **evolving patterns over drones or
> stretched audio**, 30, 45, even 90+ minutes of continuous *system*
> output. Then, in the studio, **cut, splice, rearrange, and reshape** that
> raw material into a finished piece. The editorial model is
> **clips-as-objects** with a **tape/edit-as-composition heritage** (musique
> concrète tape-splicing, king-tubby-style dub, ACID): you make *objects* out
> of the continuous flow and then compose with them. The "performer" is the
> generated system, not a human musician.

From that single statement, several properties follow that the code *must*
satisfy — and these are the seeds of the whole architecture:

- **It's audio, not MIDI.** The unit of composition is a region of *recorded
  sound*, not a note event. There is no score.
- **The source is long and continuous.** You cannot hold it in memory and you
  cannot keep it in a tiny buffer; it has to stream from disk.
- **Editing is non-destructive.** Cut/splice/rearrange must be undoable and
  re-doable without ever touching the original take. The raw take is sacred;
  the arrangement is a *value* layered on top of it.
- **It has to be replayable and deterministic.** If you edit, bounce, listen,
  and change one thing, you want the rest to be bit-for-bit the same, and you
  want to be able to reproduce any past state. This is what makes non-destructive
  editing actually usable.
- **The real-time path must not glitch.** When this ships as a live instrument
  (record-while-monitoring, edit-while-playing), a single missed deadline is an
  audible click.

The whole program can be read as: *what does it take to satisfy those five, and
what is the smallest structure that satisfies them without hard-coding the
product?* The answer is §2.

---

## 2. The solution theory: a composition is a log

This is **the** theory of the program — the single insight that explains
nearly every line. State it as a hypothesis:

> **Audio software is best built by making the event log the single source of
> truth, and everything else a derivation from it.** Time is a clock you *read*,
> not a thing anyone owns. Wiring is a *graph value*. History is a *log*.
> Capability is a *plugin*. The product is an *assembled profile*.

The core (`engine`) is deliberately **model-free**: it knows nothing about
tracks, clips, or mixers. It knows four things, and four things only
(`crates/engine/src/lib.rs`):

| core piece | answers | file |
|---|---|---|
| **clock** | *when* — absolute frames + editable tempo map + sample-accurate scheduler | `clock.rs` |
| **graph** | *how it's wired* — a patch bay of typed ports, with PDC | `graph.rs` |
| **log** | *what happened* — the append-only event log; `model-visible means logged` | `log.rs` |
| **ctx** | *what services exist* — a service registry + inject/dependency + reversible registration | `ctx.rs` |

Everything a product *is* — cut/splice/rearrange (the clip editor), the mixer,
a soft-synth — is a plugin mounted onto those four. A **profile** is just a
named assembly of plugins plus a thin host.

On top of the four pieces, two **governing invariants** do all the work
(`render.rs`):

> **I1 — Every mutation is logged at call time with its absolute frame, then
> applied by the render loop when the clock reaches that frame. A refused
> mutation is never logged.**
>
> **I2 — Rendering is a pure function of the log: no wall clock, no randomness —
> the same log renders byte-identical audio, including mid-session changes.**

One edge I1 gained in the 2026-09 hardening pass: validation is now strong
enough that a logged patch is a patch the graph will make, and the rare refusal
that still surfaces at apply time is recorded as data — an `ApplyFault`,
bounded, never cleared — while the log stands, so replay reproduces the same
refusal. A refusal still never corrupts history; its intent is simply allowed
to coexist with its fault.

Keep those two lines in your head; every design decision below is in service of
them, and every "cons" entry in §6 is a place where they are only *partially*
held. This is the theory-of-the-solution matched to the theory-of-the-problem:
the `same-log ⇒ same-audio` property is precisely what makes non-destructive
editing (problem claim 4) real, and the sample-accurate scheduler + no-alloc /
no-block render path (problem claim 5) is what makes it a real-time tool.

---

## 3. Why each part is what it is

Here is the "justify the text" layer: given the theory, why is each module the
shape it is?

### 3.1 The clock — no plugin owns time

`clock.rs`. The value `frame: u64` (absolute samples) is canonical; musical
position (beats) is **derived** through a `TempoMap`, never stored. Why? Because
a tempo edit must never corrupt stored positions. If you stored "event at beat
17," changing tempo mid-song shifts every downstream beat. Storing frames and
*deriving* beats means a tempo change is just another logged event that changes
future interpretation — the frames don't move. This is the "log's time-basis
rule," and it's the reason the timebase is seconds/samples rather than
bars-and-beats.

### 3.2 The graph — a typed patch bay, and the two tiers

`graph.rs`. "A signal" is not one thing. There are four `SignalKind`s — audio
(buffer), control (one value/block), trigger (an onset), note (pitched event) —
and a patch cord may only connect ports of the same kind. That gives you type
checking *at the wiring level*: you cannot patch a knob into an audio input.

The crucial architectural decision is the **two node tiers**:

- **Declarative nodes** — serializable data (a gain, a sine, a *clip
  reference*). Diffable, loggable, IPC-safe.
- **Opaque nodes** — `Box<dyn AudioNode>`, arbitrary code that runs on the audio
  thread. This is the extension point for `fundsp` composites and (later) CLAP.

This tier split is the resolution of a real tension in the theory: I1 says the
whole thing must be a loggable *value*, but audio capability is, in the end,
arbitrary code. The answer: *existence and params* are values in the graph; the
*code* is trusted in-process. The declarative tier is what keeps the log
deterministic; the opaque tier is what keeps the graph capable.

### 3.3 The log and the closed event vocabulary

`log.rs`. The event enum is deliberately tiny: `Mount`, `ScheduleUnmount`,
`Patch`, `SetTempo`, `SetParam`, and `Arrangement`. There is no `Clip`, no
`Track`, no `MixerMove` — those are plugins' business. Three things worth
noticing:

1. **`Arrangement` is the "closed core, open plugins" mechanism in the log
   itself.** A plugin sends an *op name* plus a field list of
   `value::Value`s; a handler the plugin registered interprets them. The core
   never grows one variant per plugin op — it has *one* generic carrier.
2. **`SetParam` is a discrete value, not a curve.** Automation curves are
   unshipped; a fader ride is a staircase of `SetParam`s or nothing. This is an
   explicit, accepted gap (see §6).
3. **`Value` keeps the core std-only and plugin-agnostic** (`value.rs`): a
   closed set of scalars (`Str`/`U64`/`I64`/`U32`/`F32`) with ids interned to
   `&'static str`. The core never sees `ArrangeOp`.

### 3.4 The engine — validate now, apply later

`render.rs`. Every public mutator is **two-phase**: validate synchronously and
fail-loud (return `Err`), then log + schedule. A **refused mutation is never
logged** — so bad input can't corrupt history. (A refusal discovered on the
render path is recorded as an `ApplyFault` — data, not a log entry.) This is
the "event-sourcing" discipline made concrete, and it's why `same log ⇒ same
audio` holds even for failed commands.

The render loop is the clever bit. Events arrive at *arbitrary sample frames*,
not block boundaries, so `render_block` **splits each 512-frame block around
every due event**, renders a sub-chunk, applies the event, continues. That is
sample-accurate lifecycle: a `SetParam` at frame 17,000 takes effect between
17,000 and 17,001, not at a block boundary.

Two deliberate safety valves, both in service of "never panic on the audio
thread" and "nothing logged is ever dropped":

- **Arrangement ops are parked, not dropped.** A media op reconciles readers
  (threads, file I/O), so it must *not* run on the render stack. If a host
  renders without flushing first, the op is parked and drained by the next
  `flush_scheduled()` — never silently lost.
- **A master-width change (mixer mount/unmount) is parked to the call
  boundary**, because a mid-call stereo↔mono change would silently split the
  frame count.

### 3.5 The media layer — "never block the audio thread" meets reality

`crates/media`. The engine is a pure, in-memory, deterministic thing; `media` is
what touches disks, devices, and threads. Every file here is the same idea: **the
render path only pops/pushes an SPSC ring; all thread-spawning, file-opening,
and ring-warming happen on the control side at command-issue time.**

- **`ring.rs`** — a lock-free single-producer/single-consumer ring, std-only,
  allocation-free after construction, with `UnsafeCell`, an `unsafe impl Sync`,
  and cache-line padding to avoid false sharing. It's the one deliberately unsafe
  core, wrapped in a safe API.
- **`wav.rs`** — *a take is crash-recoverable from the first byte*: the header
  is written with placeholder sizes before any audio, then patched on
  `finalize()`. `Drop` best-effort handles the forgotten-`finalize` case;
  `recover()` handles the crash case.
- **`drift.rs`** — the input device runs at 48,001 Hz, the session at 48,000;
  over 20 minutes that's thousands of samples of drift. A fractional-accumulator
  linear-interpolation resampler reconciles device clock to session clock.
- **`peaks.rs`** — a min/max waveform pyramid so a 25-minute file can be drawn
  without shipping audio samples to the UI.
- **`capture.rs`** — multi-channel capture: interleaved frames → per-channel SPSC
  rings for monitoring + per-channel float-WAV pool sources + peaks.

The core/shape finding worth knowing: **media mounts bypass the core log** in
this spike, because the flat `&'static str → f32` `Mount` params can't carry a
file handle. So determinism is *engine* determinism plus *media* determinism kept
in sync by discipline, not one system. §6 treats this as a real gap.

### 3.6 The arrangement value — clips-as-objects, as a pure value

`crates/media/timeline.rs`. This is the product theory: `Timeline` / `Track` /
`Clip` is an **immutable value**, and the ACID ops are **pure transforms**
`apply(&self, op) -> Result<Self>`.

The theory-critical properties:

- **Ids are carried *in* the ops** — never random/UUID/time. So replay
  reproduces the *identical* value; nothing here depends on a clock or a hash.
- **A `Clip` is a window into an immutable pool source.** It references a source
  by pool id (the source's file stem — never a path, never a hash), with a source
  window `[src_start, src_start+src_len)`, placed at `at_frame`. It can have
  per-clip fades, gain, an optional baked `loop_len`, a one-word name, and a
  `reversed` property.
- **Clips layer and overlap** (they sum at render); a gap is silence. A track's
  clips are kept sorted by `at_frame` so the render node can binary-search the
  active window and never depend on insertion order.
- **Every op is fail-loud and never partial**: a refused op returns `Err` and —
  per the engine contract — is never logged. (The looped-clip refusals in
  `razor_split`/`trim`/`chop` are nice examples: a loop's phase at the cut is
  not representable, so the op refuses *rather than* silently changing audio.)

The seam that connects the value to the log is `clip_editor.rs`: `encode_op`
turns an `ArrangeOp` into the engine's op+fields; `decode_op` turns it back; a
handler the editor registered applies it to the shared `Timeline` (a pure
`apply`), and the op is logged via `arrange_logged`. That is the **log-visibility
carve-out**: the arrangement node's *state* is the logged value.

### 3.7 The host — the product is a contract, and the UI is a plugin

`crates/host`. The central claim of the UI-as-plugin note: **our Tauri app is a
reference implementation of the host, not the host.** So a *headless* host is
built first and drives the whole thing deterministically via a versioned text
script (`parse_script`, `host v1`). The contract has three carefully-separated
parts:

- **Commands** — one per logged event, 1:1. *The log is the command list.*
- **Events** — core → host (log stream, meters, peaks). *The host renders
  events; it never computes them.*
- **Values** — declarative snapshots (graph value, `providers_of`, pool index).
  *The host never mutates shared state directly.*

This keeps the *profile* logic out of Vue by construction: a headless host must
be able to run it, so it can't hide in a component. And it makes the shell
interchangeable — swap the transport adapter, the contract stays.

---

## 4. The nested theories

There isn't one theory; there are several, layered. Recognizing this is itself
a Naur move (a program's theory is usually a *family* of nested theories, and
part of the skill is keeping them consistent):

1. **The platform theory** — "audio software as a minimal model-free core
   (clock · graph · log · ctx), everything else a plugin, the product an
   assembled profile." This is the deepest and the simplest.
2. **The first-profile theory** — "the clip arranger: clips-as-objects, tape
   heritage, non-destructive, deterministic." This is where the *product*
   semantics live (timeline value, ACID ops, media pool).
3. **The runtime-theory** (the constraints the above must respect) — "never
   allocate or block on the render path; render is a pure function of the log."
   This is the audio-engineering hardening.
4. **The process theory** — how the theory is *sustained* across a team (see §7).

The tension the project is constantly managing is between #1 and #3: the
minimal-core discipline says *don't grow the core*; the runtime discipline says
*you can't allocate on the hot path*; the product says *I need a clip editor, a
mixer, effects.* They are reconciled by keeping #3 in `engine` as invariants and
#2 in `media` as values/plugins.

---

## 5. [Optional] Test as a reading of the theory

The tests are where the theory is *enforceable*, not just stated. A few worth
knowing by name:

- **byte-identical bounce** (bounce twice, compare bytes) — the test form of I2.
- **counting-allocator** — install a global allocator, render a block, assert the
  count didn't grow — the test form of "no allocation on the render path."
- **`pdc_delays_the_fast_path`** — a 3-sample-latency node pushes the fast path
  back by 3.
- **`drift_preserves_pitch`** — 10 s of 440 Hz through the compensator,
  count zero-crossings to prove pitch didn't shift.
- **`every_op_round_trips_exactly`** and **`pure_apply_is_deterministic_and_replays`**
  — the logged-command codec and the value's determinism.

The architecture explainer's "where to look next" and the rustism index are the
idiom-level companion; this document is the conceptual-level companion. If you
internalize only two things, make them **I1** and **I2**.

---

## 6. Where the theory is only partially held (honest ledger)

A theory that only describes what it does well is marketing. Here is where this
theory is stretched or deferred — and Naur would say these are exactly the
places a modification that *ignores* the theory would hurt most:

- **Automation curves are unshipped.** The log coalesces control-rate streams
  into gestures *in principle*, but a smooth fader ride is a staircase of
  `SetParam`s today. This is the biggest hole in "the log is the product value."
- **The graph is forward-order-only → no feedback.** A delay that feeds itself —
  the single most common effect in *dub*, which the product cites as
  inspiration — is **not expressible**. Also only one audio `Out` per node, and
  control inputs are single-driver. These are the direct costs of a dead-simple
  linear render.
- **Mount identity is enforced by convention, not type.** `node_of`/`disposers`
  are keyed by the plugin's `id()` while patches and unmounts key by mount
  name, and only a `debug_assert` says the two agree (found in the 2026-09-29
  external review, deferred with a reason —
  [the disposition](../research/architecture/2026-09-29-space-bunny-review.md)).
  A rename or alias at that seam would be invisible until release. *(Closed
  since the last revision: the "two command vocabularies" split — media
  commands such as `pool`/`play`/`splice`/`bounce`/`export` now log arrangement
  events in the same engine log, so determinism is no longer split-brain.)*
- **Control→render handoff is seeded, not finished.** Control-side mutations
  currently apply on the render call stack; the real handoff is `flush_scheduled`
  and the parked-op contract — present but young.
- **Silent drops at the edges.** Fixed-capacity buffers mean floods drop triggers
  and rings drop monitor samples (counted, but dropped); voice management is
  deferred (`ToneGen` has `MAX_BLIPS = 16`).
- **Stereo sources/clips are the next sub-step.** The master is stereo and the
  mixer pans mono channels into L/R, but stereo *takes/clips* aren't here yet.
  The per-port channel-count seam is where it lands.

None of these is *wrong* for a pre-alpha. But each is a place where the theory
and the code diverge, and each is exactly the sort of place where a new
programmer — Goedecke's "new team" — would unknowingly add a patch that fights
the theory if they didn't know it.

---

## 7. The meta-theory: a project that tries to keep its own theory alive

This is the most interesting part, in the Naur frame, and the reason this doc is
worth writing here at all.

Naur's bleak conclusion is that a program *dies* when the team holding its theory
dissolves, because the theory is know-how, not text. **This repo is, in effect,
an experiment in refusing that** — in externalizing as much of the theory as can
be externalized so it survives a team that turns over (and, for a multi-model
project like this, it turns over *constantly*). The mechanisms you can see in
`AGENTS.md` and `.agents/notes/`:

- **Decision records carry reasons and alternatives.** Every note has
  `## Alternatives considered` — the rejected options and why — so the *why*
  survives, not just the what. A note supersedes, never rewrites, a decision.
- **Notes state shipped reality in present tense**, and are updated in the same
  change that alters the thing they describe — so the note tracks the theory,
  not an aspirational version of it.
- **Explainer docs carry a "Last verified against commit …" banner**, and the
  convention is *trust the code and move the line forward* when they drift. This
  is an explicit acknowledgment that docs go stale against a moving theory.
- **Every non-trivial change adds or updates a note** — the theory is expected
  to change, and to be *recorded as it changes*.
- **The model-co-work routing** (which model drives, which reviews, which gates
  merges) is itself a theory about how to build and check a theory across LLM
  authors — an AI-era answer to "how does a team maintain one coherent theory?"

In Naur's terms, this is the *life of a program* being actively extended. And it
connects directly to the two blog posts: **Hartikainen** would cheer (theory
building is not just code; the notes/chats/reviews are part of it). **Goedecke**
would caution that at scale you can't keep it perfect, and that "keep the theory
simple" is itself a value you sometimes trade. Both are true; this codebase
sits at the small end where the theory genuinely *can* be held whole — and the
infrastructure exists to keep it that way as it grows.

---

## 8. Reading order, if you want to go deeper

This doc is the *what/why*. To see it in practice, in Naur's spirit (the theory
is in the program, not just these words):

1. `crates/engine/src/lib.rs` — the four pieces + the two invariants, in 58 lines.
2. `crates/engine/src/render.rs` — the engine loop; re-read `render_block` with
   §3.4 in mind.
3. `crates/media/src/timeline.rs` — the arrangement value and its ACID ops.
4. `crates/media/src/clip_editor.rs` — the op↔log codec and the "logged value"
   carve-out.
5. `crates/host/src/lib.rs` — `parse_script` + `process` + `wire_arranger`, the
   whole product in miniature.
6. `crates/media/src/ring.rs` — the one intentionally-unsafe core, and why.
7. The decision notes under `.agents/notes/`, starting with the minimal-core and
   UI-as-plugin ones.

And the tests, always the tests: they encode the invariants as *specifications*
("bounce twice, compare bytes"; "render, count allocations") in a way prose
never can.
