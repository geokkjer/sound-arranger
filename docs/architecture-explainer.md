# sound-arranger: from architecture up

> 🕒 Last verified against commit `2be3104` (2026-09-12). If the code has moved on,
> trust the code and move this line forward.

> A plain-English (mostly) tour of the Rust code, for a developer with roughly six
> months of Rust under their belt. It assumes you are comfortable with ownership,
> `Result`, `Option`, and trait objects, but it doesn't assume you know audio
> programming — so it stops to explain *why* certain decisions look the way they do,
> and it points out the Rust idioms as they pass by. If you're newer than that,
> take [the course](rust-course/README.md) first, and operate something before
> either: [FIRST_SESSION.md](FIRST_SESSION.md).

**What this is.** A companion to the terse module docs and the decision notes in
`.agents/notes/`. It walks top-down: the shape of the whole system, then the core
engine piece by piece, then the media layer, then the host, and finally a "rustism
index" you can skim for the patterns and what they buy you.

**One sentence first.** sound-arranger is an audio platform built on a *minimal
core* — a clock, a graph interpreter, a session log, and context plumbing — where
everything else is a **plugin**, and a shipping product is an **assembled
profile**. The first profile is a clip-based arranger: record long live jams, then
cut, splice, rearrange, and mix them.

---

## 1. The big picture

### 1.1 The idea: a minimal core, everything else a plugin

Most audio software is a monolith: the timeline, the mixer, the transport, and the
audio engine all grew together, and pulling them apart later is a multi-year
project (the project's notes cite Ardour's experience with exactly that). This
project inverts the arrangement. The core is deliberately tiny and *model-free* —
it knows about **time**, **a graph**, **a log of events**, and **services**. It
does not know what a "track" is, what a "clip" is, or what a "mixer" is. Those are
all plugins.

The four core pieces, verbatim from `crates/engine/src/lib.rs`:

1. **`clock`** — the sample-accurate master clock.
2. **`graph`** — the audio graph interpreter (a "patch bay" of typed ports).
3. **`log`** — the append-only session event log.
4. **`ctx`** — context plumbing: services, dependencies, reversible registration.

Everything the product *does* is a plugin mounted onto those four. A profile —
like the clip-arranger — is just a named assembly of plugins plus a thin shell
(the host).

### 1.2 Four crates, four layers

The workspace (`Cargo.toml`) has four members, and the layering is strict:

```
crates/engine    crates/media     crates/host      crates/shell
 (the core)     (streaming/I-O)   (headless shell)   (Tauri+Vue)
     ^                ^                 ^                 ^
     |                | depends on      | depends on      | will depend on
     | depends on     | engine          | engine + media  | host (same
     +--- std only ----+                |                 |  contract)
                        std + cpal       |
```

- **`engine`** — the minimal core. **Std-only.** No `cpal`, no Tauri, no I/O of
  any kind. This is a deliberate, guarded property: the core is the one thing that
  must never be tied to a device, a windowing system, or an OS.
- **`media`** — disk streaming, the recorder, device-clock drift, and the `cpal`
  device path. It depends on `engine` and mounts its nodes into the engine's graph
  as opaque nodes (more on that in §3).
- **`host`** — a headless reference host. It defines the Host API contract
  (commands / events / values) and a CLI binary that runs a text script to
  assemble the profile, render, and bounce — with **no frontend at all** (the host
  crate stays headless). The Tauri shell (`shell`) implements the *same*
  contract; swapping shells swaps one transport adapter.
- **`shell`** — the Tauri v2 + Vue scaffold for the graphical shell. As of this
  writing it is a **bridge**: it owns a live `host::HostSession` over the engine
  and exposes a single Tauri command (`run_host_script`) that runs the versioned
  text format — the *same* contract the CLI smoke binary drives, so a script that
  bounces byte-identically on the CLI behaves the same here. The Vue component is
  still a stub (no product UI yet). It exists so the frontend work has a home and
  to prove host↔shell wiring, not because anything in the core depends on it.

The dependency direction is the whole point: `engine` and `media` must never
depend on `tauri`. The UI is a plugin, not the substrate (the
[UI-as-plugin note](../.agents/notes/implemented/architecture/2026-08-18-ui-as-plugin-host-api-and-headless-reference.md)
owns that decision).

### 1.3 Two governing invariants

Everything below is explained by two rules, stated in `lib.rs`:

1. **Every mutation is logged at call time with its absolute frame**, then applied
   by the render loop when the clock reaches that frame. *A refused mutation is
   never logged.*
2. **Rendering is a pure function of the log** — no wall clock, no randomness:
   *the same log renders byte-identical audio*, including mid-session changes.

The first rule gives you sample-accurate, replayable, non-destructive history. The
second gives you determinism — you can replay a session and get bit-for-bit the
same bounce. Keep these in your head; every file is in service of them.

---

## 2. The clock — one source of truth for time

`crates/engine/src/clock.rs`

The hardest-won lesson in audio software is that *time is the thing everyone must
agree on*. If the mixer thinks the beat is at sample 48,000 and the sequencer
thinks it's at sample 48,001, you get audible flams. The clock exists so no
plugin owns time; every plugin *reads* the clock.

### 2.1 Absolute frames, musical time derived

The clock keeps **absolute sample frames** (`frame: u64`) as the canonical time.
Musical position — beats — is **derived** through a `TempoMap`, never stored:

```rust
pub fn beat(&self) -> f64 {
    self.tempo_map.beat_at(self.frame)
}
```

Why derive instead of store? Because a tempo edit must never corrupt stored
positions. If you stored "this event is at beat 17" and then the user changes the
tempo halfway through, every downstream beat would shift. Storing absolute frames
and *deriving* beats means a tempo change is just another logged event that only
affects future interpretation — the frames don't move. This is the "log's
time-basis rule," and it's the Feldman-inspired choice of a **seconds/samples
timebase** rather than bars-and-beats (a beat grid can be layered on later).

> **Rustism — newtype for value:**
> ```rust
> pub struct NodeId(pub u64);
> ```
> A bare `u64` for a node id is a footgun — you could pass a frame where a node id
> belongs. Wrapping it in a one-field struct makes them different *types* that the
> compiler refuses to mix up, at zero runtime cost. This codebase leans on that
> trick heavily (see also `NoteEvent`, `RenderBlock`).

### 2.2 The `TempoMap` and the deriving trick

`TempoMap` is a sorted list of `TempoSegment { start_frame, bpm, beats_per_bar }`.
Two methods convert between frames and beats:

- `beat_at(frame)` walks segments, accumulating fractional beats.
- `frame_at(beat)` is its inverse.

There is a delightful detail in `frame_at` — its "unreachable" comment:

```rust
// Unreachable: the final open-ended segment covers any finite beat.
self.segments.last().expect("tempo map never empty").start_frame
```

The last segment is open-ended (runs to `u64::MAX` of frames), so any finite beat
value is covered, and the fall-through can never run. The `.expect(...)` documents
an invariant ("the map is never empty") rather than a recoverable error — if that
line ever fires, it's a programming bug, so a panic is correct. This is the
"expect means invariant" idiom: use it for "this cannot happen unless my code is
wrong," not for user error.

### 2.3 The scheduler — "do this at sample *exactly* t"

```rust
pub struct Scheduler<T> {
    entries: Vec<(u64, T)>,   // (frame, payload), kept sorted by frame
}
```

The scheduler is a sample-accurate one-shot queue. `schedule(frame, payload)`
inserts in sorted order; the render loop drains events with `frame <= now`.
It's generic over `T` (`Scheduler<T>`) — the engine instantiates it with
`SchedEvent`, but nothing about the queue cares what the payload is.

> **Rustism — `partition_point` for binary search:**
> ```rust
> let at = self.entries.partition_point(|(f, _)| *f <= frame);
> self.entries.insert(at, (frame, payload));
> ```
> `partition_point` (stable since Rust 1.52) returns the index where the predicate
> stops being true — i.e. a binary search for the insertion point *without* the
> classic off-by-one errors of hand-rolled `binary_search`. You'll see it in the
> `TempoMap` and the graph too. If you've been reaching for `binary_search` and
> patching the edge cases, `partition_point` is the cleaner tool.

There's also a `drain_until` that returns an `impl Iterator<Item = T> + '_` — a
*lazy* drain (no allocation, no copied vector) that the render loop uses via
`peek_frame()` instead of eager popping. The `+ '_` is an elided lifetime: the
returned iterator borrows `self` for as long as `self` lives.

---

## 3. The graph — a typed patch bay

`crates/engine/src/graph.rs`

If the clock answers "when?", the graph answers "how are the sounds wired?". It's
an audio graph interpreter modelled as a **patch bay**: nodes declare named, typed
ports, and patch cords connect them.

### 3.1 Four signal kinds

The core insight is that "a signal" is not one thing. The graph distinguishes four
`SignalKind`s:

- **Audio** — a buffer of `f32` samples per block.
- **Control** — one `f32` per block (a knob value).
- **Trigger** — a sample-timestamped event (an onset, "now!").
- **Note** — a sample-timestamped pitched event (semitones + velocity + duration).

A patch cord may only connect ports of the same kind; `connect` returns an error
on a mismatch. This gives you *type checking at the wiring level* — you literally
cannot patch a knob into an audio input, the way you can't assign a `String` to a
`u32`. The euclidean generator produces **triggers**, the scale plugin turns
triggers into **notes**, the tone plugin turns notes into **audio**. That chain is
the whole Spike A.5 demo, expressed as data.

Audio ports also carry a **channel count** (`Port { channels: u16 }`, mono by
default, stereo via `Port::stereo`). `connect` refuses a count mismatch, and the
interpreter sizes each node's audio buffers by its port's channel count. Today the
graph's audio *inputs* are mono channels and the *master* is stereo — this count
is the seam a genuine stereo take/clip will land on, the moment one exists.

> **Rustism — `enum` as a closed, total vocabulary:**
> `SignalKind` and `Direction` are enums with a fixed set of variants. The
> advantage over using strings `"audio"`/`"trigger"` is exhaustiveness: a `match`
> on the enum must handle every variant, and the compiler tells you when a new
> kind breaks a caller. This is "make illegal states unrepresentable" in miniature
> — an audio cord and a trigger cord are *different* values that can't be
> confused.

### 3.2 Nodes, ports, and the two tiers

A node is `{ id, kind, ports }`. A port is `{ name: &'static str, direction, kind }`
— a `&'static str` name plus a direction and a kind. (The `'static` says "this
string lives forever," which is true for string literals; it lets port names be
copied around without lifetime bookkeeping or allocation.)

There are **two node tiers**, a decision the minimal-core note spends real ink on:

- **Declarative nodes** — serializable data (a gain stage, a sine oscillator, a
  clip reference). Diffable, loggable, IPC-safe.
- **Opaque nodes** — `NodeKind::Opaque(Box<dyn AudioNode>)`. A trait object
  wrapping arbitrary code that runs on the audio thread.

The opaque tier is the extension point: `fundsp` composites and CLAP plugins will
land here later, contributed through a factory registry. The *code* is trusted
in-process; its *existence and params* are values in the graph.

> **Rustism — `Box<dyn Trait>` ("trait object"):**
> ```rust
> Opaque(Box<dyn AudioNode>)
> ```
> A `Box<dyn AudioNode>` is a heap-allocated value of *some* type that implements
> `AudioNode`, whose concrete type is erased. This is Rust's answer to the
> "interface/abstract base class holding a subclass" pattern. You call
> `node.render(...)` through the vtable without knowing the concrete type — that's
> how `Sine`, `Gain`, `EuclideanGen`, and `ToneGen` are all stored in the same
> `Vec<Node>` and rendered uniformly. The `Send` bound on `AudioNode` means "safe
> to move to another thread," which matters because (in a later phase) the graph
> rendering runs on the audio thread.

### 3.3 The `AudioNode` trait — the render contract

```rust
pub trait AudioNode: Send {
    fn latency(&self) -> u32;
    fn render(
        &mut self,
        io: &NodeIO,
        out_audio: &mut [f32],
        out_control: &mut f32,
        out_triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        out_notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        block: RenderBlock,
    );
    fn set_param(&mut self, _name: &str, _value: f32) {}
}
```

Three things to notice:

1. **`latency()`.** Every node reports how many samples of latency it introduces.
   The interpreter performs **plugin delay compensation (PDC)** — it delays the
   *fast* paths so parallel branches align at the output. This is "part of the
   value from day one" because lookahead nodes (limiters, linear-phase EQ) make it
   non-optional, and retrofitting latency into a graph schema later is painful.

2. **`set_param` has a default impl that does nothing.** This is the
   "override-me-if-you-need-it" pattern: nodes with no runtime params (like
   `Sine`, except its `freq`) can skip it, while `ToneGen` and the mixer override
   it. Default trait methods are how Rust gives you optional behaviour without a
   separate "HasParams" trait.

3. **The signature forces a discipline: no allocation.** The node is handed
   pre-existing output buffers (`&mut [f32]`, `&mut EventBuf<...>`) and must fill
   them. It can't return a fresh `Vec` even if it wanted to. This *shape* of the
   API is itself the enforcement of the "render path never allocates" invariant —
   the type signature makes the cheap path the natural one.

### 3.4 The no-allocation render path

Here is the single most important performance rule in the whole project, enforced
by tests rather than by hope:

> **The steady-state render loop never allocates and never blocks.**

Why? On a real device, the render happens in a `cpal` audio callback on a
real-time thread. A single `Vec::push` that reallocates, or a `Mutex::lock` that
waits, can miss the deadline and produce an audible click. So the graph
pre-allocates **everything** and the render path only *writes into* it.

Concretely, look at `Graph`'s fields: `audio_out: Vec<Vec<f32>>`,
`control_out: Vec<f32>`, `triggers_out: Vec<EventBuf<...>>`, `audio_ins:
Vec<Vec<Vec<f32>>>`, `delays: Vec<RingDelay>`. Every one of those is sized when a
node is added (control side), not during render. The `render` method walks nodes
gathering inputs, calls the node, applies PDC, and writes the output — purely by
indexing into preallocated buffers.

How do they *know* it's allocation-free? A **counting-allocator test**: they
install a global allocator that counts allocations, render a block, and assert the
count doesn't grow. That's a beautiful property of Rust being explicit about
allocation: you can *measure* it. (The `#[global_allocator]` mechanism lets a test
swap in an allocator that records every call — see the tests.)

> **Rustism — fixed-capacity buffers with `const` generics:**
> ```rust
> pub struct EventBuf<T: Copy + Default, const CAP: usize> {
>     buf: [T; CAP],
>     count: usize,
> }
> ```
> `const CAP: usize` is a *const generic* — the capacity is part of the *type*,
> so `EventBuf<NoteEvent, 32>` and `EventBuf<NoteEvent, 128>` are different types
> with different sizes, and the array is inline (no heap). This is how you get a
> bounded, allocation-free buffer whose size is checked at compile time. The
> `BLOCK = 512` sample block size and `CAP_EVENTS = 32` event capacity are the
> same idea applied globally.

The one place this is stated *and also honored loosely*: the fixed-size `Vec`s are
preallocated at node-add time, but `push` on `EventBuf` can still "overflow" its
fixed capacity — it returns `false` and *drops* rather than growing. Dropping is
documented as an accepted policy for event floods; growing would be an allocation
on a hot path, which is worse.

### 3.5 PDC — why the fast path gets delayed

`RingDelay` is a small ring buffer used for PDC:

```rust
pub fn process(&mut self, x: f32) -> f32 {
    if self.delay == 0 { return x; }
    let cap = self.buf.len();
    let read = (self.pos + cap - self.delay) % cap;
    let delayed = self.buf[read];
    self.buf[self.pos] = x;
    self.pos = (self.pos + 1) % cap;
    delayed
}
```

The render loop computes each node's cumulative latency, finds the *maximum*, then
delays every node by `max_cum - cum[i]` so that all paths arrive at the output at
the same moment. The delay line is preallocated to `MAX_PDC` samples; only the
read offset changes during render — so PDC itself introduces no allocation. The
test `pdc_delays_the_fast_path` confirms a 3-sample-latency node pushes the fast
sine path back by 3 samples.

### 3.6 Fan-in and the forward-order rule

Look at `connect` — it has a fascinating constraint:

```rust
if fi >= ti {
    return Err("connect: patch cords must go forward (topological order)".into());
}
```

Nodes may only be connected **forward** (in index order). This is a deliberate
simplification that makes rendering a single forward pass with no cycles and no
topological sort. The cost: you must add nodes in a sensible order (sources
first, sinks last), and feedback loops (a delay feeding itself) are not
expressible yet. The benefit: the interpreter is dead simple and the render is a
linear sweep. That's a classic Rust-honesty trade — the constraint is *enforced in
the type of the operation's result* (`Result<(), String>`) and documented, rather
than being a hidden assumption.

Fan-in is also exercised here: **many producers may connect to one input port**, and
audio is *summed per port* (which is exactly what a mixer needs — its channels are
separate input ports, and multiple sources may feed a channel). Control inputs are
*single-driver* in Phase 1 (only one cord), also enforced by an error. The
`insert_sorted` on `EventBuf` merges multiple event producers into one sorted
stream so consumers like `ToneGen` can drain by offset.

---

## 4. The log and the engine — "a composition is a log"

`crates/engine/src/log.rs` and `crates/engine/src/render.rs`

### 4.1 The event log is a closed vocabulary

```rust
pub enum Event {
    Mount   { plugin: &'static str, params: Vec<(&'static str, f32)>, at_frame: u64 },
    ScheduleUnmount { plugin: &'static str, at_frame: u64 },
    Patch   { from_plugin, from_port, to_plugin, to_port, at_frame },
    SetTempo { bpm: f64, beats_per_bar: u32, at_frame: u64 },
    SetParam { plugin: &'static str, param: &'static str, value: f32, at_frame: u64 },
}
```

Every event carries the absolute frame at which it takes effect. This is the
event-sourcing discipline: the engine mutates itself *only* by applying events, so
render is a pure fold over the log. "Model-visible means logged" — borrowed from
the deepseek-harness research the project studied — is the invariant: anything a
plugin needs to know is an event, and nothing audio-rate is ever logged (a
45-minute fader ride is coalesced into gestures, not thousands of events).

Note the deliberate *smallness* of the vocabulary. There is no `Clip`, no `Track`,
no `MixerMove`. Those are plugins' business. The core log knows about lifecycle
(mount/unmount), wiring (patch), time (tempo), and *one* generic control
(`SetParam`). That's it. This is the "minimal core" discipline held firm — and
worth contrasting with the temptation to add `SetClipVolume` to the core.

> **Rustism — `enum` with data as a typed event union:**
> Each `Event` variant carries its own payload struct inline. This is Rust's
> "sum type" — the safe, checked alternative to a tagged union or a base `Event`
> class with dynamic casts. `match` on `Event` gives you the payload with its
> fields available, and the compiler enforces you handle every variant, so adding
> a new event type is a compile error everywhere until it's handled.

### 4.2 The engine: validate now, apply later

`Engine` is where the four pieces assemble. Its public API — `mount`, `patch`,
`schedule_unmount`, `unmount`, `set_tempo`, `set_param`, `replay_from`,
`providers_of` — is explicitly called out as "the seed of the core plugin contract
(the future IPC command list)."

The crucial pattern is the **two-phase mutation**:

1. **Validate synchronously** (fail-loud, return a `Result`). Check the plugin is
   registered, its declared services are all provided, its params are in range,
   etc.
2. **Log the event with its frame, and schedule it.** The render loop applies the
   event *when the clock reaches that frame*.

A refused mutation is never logged — so a bad `set_param` neither corrupts history
nor takes effect. Look at `set_param` for the full validation discipline:

```rust
if !value.is_finite() { return Err(...); }            // NaN/∞ are banned
if value < def.min || value > def.max { return Err(...); }  // range check
```

`def` comes from the plugin's *declared* param surface (`params()`), and anything
not declared is refused. So a plugin's runtime knobs are a closed, documented,
range-checked namespace — the log can trust them, and the UI can render them from
the declaration rather than hard-coding.

> **Rustism — `&'static str` as a cheap registry key:**
> Plugin names, port names, and param names are all `&'static str`. Because they're
> statics (interned at compile time for literals), they're `Copy`, comparable, and
> hashable with no allocation and no lifetime juggling. The flip side is they must
> be *closed* — you can't register a runtime-generated name. That's exactly the
> point: the `HOST_NAMES` registry in `crates/host` is a closed vocabulary the
> text parser accepts and a future shell sends verbatim.

### 4.3 Rendering interleaves events with blocks

The clever part is `render_block`. A block is `BLOCK = 512` samples, but events
don't arrive at block boundaries — they arrive at *arbitrary sample frames*. So
the render loop **splits the block around each due event**:

```rust
let next = self.scheduler.peek_frame().filter(|f| *f < f1).unwrap_or(f1);
if next > pos {
    // render [pos, next) as a contiguous chunk, then apply the event, repeat
}
```

It renders a sub-chunk, applies whatever events land exactly at `next`, renders
the next sub-chunk, and so on. The effect is *sample-accurate* lifecycle: a
`SetParam` at frame 17,000 takes effect between frames 17,000 and 17,001, not at
the 512-frame boundary. The `debug_assert_eq!(written, out.len())` at the end
verifies the splitting arithmetic always covers the whole block.

One event kind deliberately does **not** apply here: arrangement ops run
control-side handlers (threads, file I/O), which must never execute on the
render path. If a host renders without flushing them off the queue first, the
render loop **parks** the op instead of dropping it — the next
`flush_scheduled()` applies parked ops FIFO before the due queue — so nothing
logged can ever be silently lost and live/replay cannot diverge (a bug of
exactly that shape existed until the control→render handoff note fixed it;
[`implemented/architecture/2026-08-27-control-render-handoff-parked-ops.md`](../.agents/notes/implemented/architecture/2026-08-27-control-render-handoff-parked-ops.md)
owns the contract). The practical rule while the host is single-owner: *call
`flush_scheduled()` before rendering*.

**The drain/EOF phase (bounce tails).** `render_into` stops at the frame budget,
so `Engine::drain` renders extra `RenderMode::Drain` blocks after the timeline
until no mounted node reports `has_tail()` (a decaying voice, a delay/reverb
tail), then flushes the PDC transit still in flight. The mode is explicit: in a
drain block a self-driven source (`Sine`, `EuclideanGen`, the ring-streaming
media nodes) mutes itself, while processing nodes keep rendering so tails travel
the chain. `DrainPolicy::{HardCut, Tails}` names the choice; the offline `Bounce`
uses `Tails` and fails loud on a `capped` drain rather than writing a silently
shortened file (the drain/EOF note owns the contract).

> **Rustism — `debug_assert!` for "cheap in dev, gone in release":**
> The codebase is littered with `debug_assert!` — invariants that are checked in
> debug/test builds and compiled out in release. The philosophy: *assert loudly in
> development, but never risk a panic on the audio thread in a shipped build.*
> Where a release build must still be safe, the check degrades to a no-op or a
> documented skip rather than a panic. Contrast `debug_assert!` (invariant,
> removed in release) with `.expect()` (invariant, always panics if violated) —
> the former is for "trust me, just verify during dev," the latter for "this is a
> hard contract."

### 4.4 Determinism and replay

`replay_from` runs a previously-recorded log onto a *fresh* engine:

```rust
pub fn replay_from(&mut self, log: &SessionLog) -> Result<(), String> {
    for event in log.events() { /* schedule each at its recorded frame */ }
    Ok(())
}
```

Nothing is applied eagerly — every event is scheduled at its recorded frame and
applied by the render loop. So replay reproduces the *exact* timeline. Combined
with "render is a pure function of the log" (no wall clock, no randomness), this
yields the golden property: **the same log renders byte-identical audio**, which
the tests prove by bouncing twice and comparing bytes.

There's a subtle honesty in `apply_patch` and `apply_event`: if an endpoint isn't
mounted at apply time (a "log-order error" that validation should have caught),
the code `debug_assert!(false, ...)` and *returns* rather than panicking — the
intent stays in the log, replay reproduces the same refused state, and no
audio-thread crash happens. "Never panic on the audio thread, even on a bug" is a
hard rule.

---

## 5. Plugins — reversible effects in miniature

`crates/engine/src/plugins/mod.rs`

### 5.1 The `Plugin` trait

```rust
pub trait Plugin {
    fn id(&self) -> &'static str;
    fn inject(&self) -> &'static [&'static str];        // dependencies
    fn ports(&self) -> &'static [Port];                  // patch-bay surface
    fn params(&self) -> &'static [ParamDef] { &[] }      // runtime knobs
    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String>;
}
```

A plugin *declares* three things about itself:

- **`inject()`** — the services it *requires* (a "coeffect specification"). The
  euclidean plugin declares `["clock"]`; the mixer declares `[]`.
- **`ports()`** — its patch-bay surface, the data source for a "which output can I
  connect?" dropdown.
- **`params()`** — its runtime parameter namespace with ranges.

And `apply()` returns a `(NodeId, Disposer)` — the node it mounted **plus a
function that unmounts it**. That disposer is the whole "reversible effects"
discipline compressed into one type:

```rust
pub type Disposer = Box<dyn FnOnce(&mut DisposerCtx<'_>)>;
```

> **Rustism — the disposer is a closure that captures its own cleanup:**
> `Box<dyn FnOnce(...)>` is a *closure object*. `FnOnce` means "called at most
> once" — exactly right for teardown. The closure **captures** whatever it needs
> to undo (the `NodeId`, in the euclidean case) by value, so it stays alive after
> `apply` returns. This is Rust's answer to the "inverse" of a registration: you
> don't write teardown as a symmetric separate method, you *construct the inverse
> at the moment of mounting* and hand it back. The paper the project studied
> (Cordis) calls this "an effect carries its inverse"; Rust closures make it a
> value.

Look at the euclidean plugin's `apply` — it adds a node, provides the `rhythm`
service, and returns a disposer that removes both:

```rust
api.ctx.provide("rhythm", Rhythm { ... });
Ok((
    node,
    Box::new(move |dis: &mut DisposerCtx| {
        dis.graph.remove_node(node);   // `node` is captured by the move closure
        dis.ctx.remove("rhythm");
    }),
))
```

The `move` keyword transfers ownership of `node` *into* the closure, so the
closure can outlive `apply`. Without `move`, the closure would borrow `node` from
the stack and it'd dangle.

### 5.2 Fail-loud mounting, one instance per name

`Engine::validate_mount` enforces three things before anything is logged:

1. The plugin is a registered factory.
2. All of its `inject()`ed services are provided (core or `ctx`).
3. No plugin of that name is already mounted (one instance per name in this
   spike).

> **Rustism — the "temporary" destructuring trick:**
> ```rust
> let (mut a, vb) = self.nodes[fi].port_index(from_port)...;
> ```
> Actually the more instructive one is in `apply_mount`:
> ```rust
> let Engine { ctx, scheduler, graph, clock, .. } = self;
> ```
> This borrows `self` *fields individually* and simultaneously — which Rust
> otherwise forbids (you can't take two `&mut` fields of the same struct through
> one `&mut self`). By destructuring the whole struct into separate field
> bindings, the borrow checker sees disjoint borrows and allows it. It's the
> canonical way to hand multiple pieces of `self` to a helper without `RefCell`
> or clone — you'll recognize it every time you see `let Engine { .. } = self;`.

### 5.3 The mixer — the profile's master bus

`crates/engine/src/plugins/mixer.rs` is the flagship plugin, and worth a close
look because it exercises almost every idea in the project.

- **Adaptable to the device** (P1.2): the `channels` count is a *mount parameter*
  clamped to `1..=8`, sized from the input device's layout (the Notepad-12FX's 4
  USB channels, the Scarlett 2i2's 2).
- **Owns the master bus**: mounting the mixer calls `api.graph.set_out(node)`,
  which ends Spike A.5's "last audio provider wins" limitation on who owns the
  output.
- **Per-channel pan into a stereo master**: each of the `1..=8` *mono* input
  channels gets gain / mute / solo / `pan` (`ch{k}.pan`, −1..1), summed into a
  stereo master bus (`channels: 2`). A pan is exactly the equal-power
  `cos`/`sin` law from the splice, applied in miniature. (Two-channel *sources*
  are the next sub-step — the graph's audio ports are already channel-aware.)
- **Provides a service**: it shoves an `Arc<MeterBank>` under the `mixer.meters`
  context key, which the host later reads for the UI — without the UI needing to
  know how the mixer works.

The meters are the star of the Rust-al idioms:

```rust
pub struct MeterBank(pub [AtomicU32; MIXER_CHANNELS_MAX + 1]);

impl MeterBank {
    pub fn channel_peak(&self, k: usize) -> f32 {
        f32::from_bits(self.0[k].load(Ordering::Relaxed))
    }
}
```

> **Rustism — `AtomicU32` + `f32::from_bits` as a lock-free meter:**
> The audio thread writes peaks; a UI thread reads them. Naively you'd lock a
> mutex, but you must never block the audio thread. The trick: store the `f32`'s
> *raw bits* in an `AtomicU32`, so the write is a single atomic store (`p.to_bits()`
> → `store(..., Ordering::Relaxed)`) and the read is a single atomic load
> (`from_bits`). No lock, no blocking, and each read/write is one atomic operation.
> `Ordering::Relaxed` is the loosest memory ordering — correct here because a meter
> value is "best effort" (no other data depends on its ordering), and the loosest
> ordering is the cheapest. This "atomics as zero-lock meters/counters" pattern
> recurs all over `media/` (`underruns`, `overruns`, `frames`, `dropped`).

The mixer's own `set_param` parses dotted names like `"ch1.gain"` by string
splitting — a deliberate Phase-1 convenience, at the cost of string-parsing on the
control side (fine; it's not the render path). And the meter placement is a real
audio decision, documented: channel meters are *post-gain, pre-mute/solo* (a
muted channel still shows its level), while the master meter is post-master-gain.

---

## 6. The media layer — streaming without blocking

`crates/media/src/`

The engine is a pure, deterministic, in-memory thing. The media crate is what
touches the real world — disks, devices, threads — and it's where the "never
block the audio thread" rule meets reality.

### 6.1 The lock-free SPSC ring

`crates/media/src/ring.rs` is the workhorse. `Spsc<T>` is a **single-producer,
single-consumer** ring buffer, std-only, allocation-free after construction.

The design is a study in both audio engineering and Rust's unsafe discipline:

```rust
#[repr(align(64))]
pub struct Spsc<T> {
    slots: Box<[UnsafeCell<Option<T>>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    _pad: [u8; 48],          // pad so `count` sits on its own cache line
    count: AtomicUsize,
}

unsafe impl<T: Send> Sync for Spsc<T> {}
```

Let's unpack the ideas:

- **`UnsafeCell<Option<T>>`** is how you store mutable data behind a shared
  (`&`) reference. Rust's normal rule says you can't mutate through `&self`;
  `UnsafeCell` is the sanctioned escape hatch, and the code takes responsibility
  for the safety.
- **`unsafe impl<T: Send> Sync`** claims the type is safe to share *across*
  threads, justified by the SPSC contract (one writer, one reader, never the same
  slot) enforced by the `count` atomic's acquire/release edges. The `// SAFETY:`
  comment is not optional — it's the convention and the social contract for
  `unsafe` in Rust: every `unsafe` must cite the invariant that makes it sound.
- **`#[repr(align(64))]` and `_pad: [u8; 48]`** are **false-sharing** defenses:
  aligning to a 64-byte cache line and padding so the two contended atomics
  (`head`/`tail` on one side, `count` on the other) don't sit on the same cache
  line and invalidate each other. A real performance nit, caught by a code review
  (the notes credit a "kimi review" finding).

The memory-ordering story is deliberately precise and documented in the header
comment: producer writes slot, then `count.fetch_add(1, Release)`; consumer
`count.load(Acquire)`, then reads slot. The `Release`/`Acquire` pair is the
happens-before edge that makes the slot write visible to the consumer. `Relaxed`
is used for the head/tail indices *because* the count already carries the ordering
— using stronger orderings there would be redundant (and slower).

> **Rustism — when `unsafe` is the *right* answer, not the wrong one:**
> A lock-free SPSC ring genuinely needs `unsafe` (no safe-only construction can
> express "one thread writes here while another reads there, but never the same
> slot"). The codebase's discipline is exemplary: the `unsafe` is *narrow* (a few
> lines), *commented* (each block cites its `SAFETY` reasoning), and *encapsulated*
> (the safe `try_push`/`try_pop` API means nothing else in the codebase touches
> the unsafety). This is the opposite of "just sprinkle `unsafe` around"; it's
> "build a small, exhaustively-reasoned unsafe core and wrap it in a safe API."
> (A note: in production you'd likely reach for the battle-tested `rtrb` crate
> rather than hand-roll this — the RESEARCH doc lists `rtrb` for exactly this —
> but hand-rolling it here is a *learning* exercise with real tests.)

### 6.2 The player and the recorder: threads off the audio path

Both `FilePlayer` (`stream.rs`) and `WavRecorder` (`record.rs`) follow the same
architecture, and it's the key to the whole crate:

- **Control side** (when a command is issued): open the file, spawn a thread,
  warm the ring.
- **Render path** (the `AudioNode::render`): *only* `try_pop`/`try_push` the ring.
  No I/O, no `sleep`, no lock.

`FilePlayer::start` spawns a reader thread that reads the WAV into the ring. The
`PlaybackNode` (an `AudioNode`) pops the ring into its `out("audio")` port. The
reader thread blocks on `sleep(50µs)` when the ring is full — blocking is *fine
on the reader thread*, it's only forbidden on the audio thread. The separation of
"this thread may block, this thread must not" is made explicit and structural.

Two subtle, documented edge cases:

- **Underrun vs. legitimate silence:** `pop_sample` distinguishes an empty ring
  at the *end* of a clip (that's silence — the clip is done) from an empty ring
  *mid-clip* (that's an underrun — the disk didn't keep up, count it). It checks
  `eof()` and `popped >= expected` to decide.
- **Panic discipline:** a splice arriving *mid-fade*, or a splice whose frame
  already passed, is counted (`deferred`) and applied at the next block start —
  never silently dropped, never a `panic` on the render path. The `Fade` struct
  handles "fade from silence" (`cur: None`) so a splice with no current source
  can't panic either.

### 6.3 Equal-power crossfade — the splice

The splice is the product's core gesture (cut during playback, glitch-free). The
crossfade is **equal-power**: the two gains are `cos(π/2 · t)` and `sin(π/2 · t)`,
which keeps the *perceived* loudness constant (their squares sum to 1: `cos² +
sin² = 1`). A linear fade (`1-t`, `t`) would dip in the middle — an audible
"hole." At `t = 0` the output is pure current, at `t = 1` pure incoming, no gain
step at either end.

### 6.4 Device-clock drift

`drift.rs` solves a problem you might not expect to exist: the *input* device
runs at 48,001 Hz and the *session* runs at 48,000 Hz. Over 20 minutes that's a
drift of thousands of samples — the recorded timeline would slide out of sync and
the buffer would grow without bound. `DriftCompensator` is a fractional-accumulator
linear-interpolation resampler: `push_input` accepts the (drifting) device block,
`pull_output` produces exactly `out.len()` session frames. The ratio is a
parameter (`in_rate / out_rate`) — a placeholder for real measurement in Phase 1.

The test is even better than the code: `drift_preserves_pitch` records ten seconds
of 440 Hz through the compensator and counts zero-crossings to prove the pitch
didn't shift (which is what a naive "drop a sample when drifting" would do).

### 6.5 WAV and crash recovery

`wav.rs` is a *minimal* RIFF/WAVE reader and writer, std-only, with one especially
nice property:

> **A take is crash-recoverable from the first byte.**

The writer writes a complete header with **placeholder** sizes *before* any audio,
then `finalize()` seeks back and patches the real sizes. Two consequences fall
out:

- **Forgetting `finalize`** (normal `drop`) still yields a readable file, because
  `Drop` best-effort patches the header.
- **A crash mid-write** (no `drop`, no `finalize`) is recoverable: `recover()`
  rescans the data chunk, patches both size fields, and *truncates* any torn
  tail so the recovered file is formally well-formed.

This is the "the pool keeps 32-bit float, export is 16-bit" decision made
concrete: `WavWriter::create` (16-bit, clamping and quantizing) vs
`create_float` (32-bit float, lossless), and the reader transparently reads both
and downmixes stereo to channel 0 (clips are mono today).

> **Rustism — `Drop` for best-effort cleanup:**
> ```rust
> impl Drop for WavRecorder {
>     fn drop(&mut self) { let _ = self.stop(); }
> }
> ```
> Both `WavRecorder`, `Capture`, and `WavWriter` use `Drop` to do best-effort
> finalization — and note `let _ =` to swallow the `Result`, because `drop`
> cannot return errors and must not panic. The distinction the code draws is
> crisp: `Drop` handles the *normal* forgotten-call case; `recover()` handles the
> *abnormal* no-`Drop`-ran case (a crash). This is RAII (Resource Acquisition Is
> Initialization) doing double duty for durability.

### 6.6 Peaks — the waveform pyramid

`peaks.rs` builds the peak data the UI will draw. The structure is Audacity's:
min/max pairs over **256-sample base bins**, accumulated incrementally as samples
arrive ("live" — the recorder feeds it on the write side), with **upper levels**
(min/max over 2× bins per level) computed at finalize, and a persisted `.peaks`
sidecar. `PEAK_LEVELS = 10` gives you from 256 samples per bin up to ~2.7 seconds
per bin at 48 kHz — so the UI can draw a whole 25-minute file without the Rust
side ever shipping audio samples (peaks are downsampled, audio stays in Rust).

### 6.7 Capture — the multi-channel recorder

`capture.rs` is the P1.2 recorder path targeting USB hardware mixers. A device (or
test fixture) feeds **interleaved frames** into a shared ring; a **demux thread**
splits each frame into per-channel SPSC rings (for monitoring via `CaptureNode`)
and writes each channel to its own float-WAV pool source + peaks sidecar. The pool
write is authoritative (a failed writer is reported and stopped); the monitoring
rings are best-effort (full → drop + count). One real bug the comments (and tests)
call out: a *partial frame* at the tail must be kept for the next batch — dropping
it would shift every channel by a sample.

The code uses a then-recent Rust feature worth noting, the **`let`-chains** idiom:

```rust
if let Some(pc) = &mut writers[k]
    && let Err(e) = pc.push(&chan)
{ ... }
```

The `&& let` (stabilized in Rust 1.88) lets you `if let` two conditions in one
guard without nesting. If your rustc is older, this won't parse — the project
targets a recent toolchain, as does `edition = "2024"` in `Cargo.toml`.

---

## 7. The host and the Host API contract

`crates/host/src/lib.rs` and `main.rs`

### 7.1 Why a headless host first

The UI-as-plugin note's central claim: **the UI must be interchangeable — our
Tauri app is a *reference implementation* of the host, not the host.** And a
contract no second host has exercised is a wish. So before any Tauri shell, there
is a *headless* host that exercises the entire contract deterministically, CI-
tested. The `host` binary reads a text script and runs it — record, splice,
mix, bounce — with no frontend whatsoever.

The contract has three parts (carefully distinguished, and this distinction is
worth internalizing):

- **Commands** — one per logged event, 1:1. `Mount`, `Patch`, `SetParam`,
  `SetTempo`, `Unmount`, plus the media/profile commands `Play`, `Splice`,
  `Bounce`. *The log is the command list.*
- **Events** — core → host: the log stream, meters, peaks, transport. **The host
  renders events; it never computes them.**
- **Values** — declarative snapshots the host interprets: the graph value,
  `providers_of` (the patch UI's dropdown), the pool index. **The host never
  mutates shared state directly.**

The crisp rule that keeps the whole thing honest: *the UI thread never touches the
render path, and the profile's logic lives in the shared contract layer, not in
Vue components.* The headless host running the same script is the guard — if the
logic leaked into Vue, the headless host couldn't run it.

### 7.2 `HostSession` and the script

`HostSession::new()` registers the four plugin factories (euclidean, scale, tone,
mixer) by name — the same names a shell will send. Then `apply(&HostCommand)`
dispatches: engine commands call straight through (`engine.mount`, `engine.patch`,
`engine.set_param`), and the media commands (`Play`, `Splice`, `Bounce`) do the
profile-level wiring.

Alongside the one-shot `run_script`, `HostSession` is also a **persistent, live
session**: `execute(&HostCommand)` applies one command at a time (the form a UI
needs for incremental editing) and shares the exact `process` path with
`run_script`, so the live path can never diverge from the one-shot path. It also
exposes a **serializable arrangement snapshot** (`arrangement() -> Timeline`) —
the pure reconstruction of the logged `Arrange` ops — plus `meters()`, `providers()`,
`underruns()` / `deferred()`. That is the "events the host renders, values the host
interprets" half of the contract, made concrete for a shell.

The one genuinely cagey part is **play wiring**, because of the forward-order rule
(§3.6). A `Play` command adds a `PlaybackNode` to the graph *immediately* (players
must come before the mixer in node index order), but the mixer's node doesn't
exist yet — its mount applies on the *first render*. So the cord from player →
mixer channel is **deferred** into `pending_cords` and wired on the first render:

```rust
pub fn render(&mut self, frames: usize) -> Result<Vec<f32>, String> {
    // bound the buffer so a malformed `bounce` cannot OOM
    if frames.saturating_mul(std::mem::size_of::<f32>()) > Self::MAX_BOUNCE_BYTES {
        return Err("bounce exceeds the byte budget".into());
    }
    self.wire_pending()?;   // deferred player→mixer cords, once the mixer node exists
    self.wire_arranger()?;  // reconcile the arranger nodes against the arrangement value
    Ok(self.engine.render(frames))
}
```

This is a nice example of the two-phase discipline from §4.2 extended to the
profile: *schedule the intent now, resolve the wiring when both ends exist.*

> **Rustism — `&'static str` names again, but as a *closed registry*:**
> ```rust
> pub const HOST_NAMES: &[&str] = &[ "euclidean", "scale", ..., "ch7.gain" ];
> fn host_name(s: &str) -> Result<&'static str, String> {
>     HOST_NAMES.iter().find(|n| **n == s).copied()
>         .ok_or_else(|| format!("unknown host name '{s}' ..."))
> }
> ```
> The text parser maps arbitrary input strings to *known* `&'static str`s by
> looking them up in a fixed registry, failing loudly on anything unknown. This
> is how the closed `&'static str` vocabulary (§4.2) is reconciled with
> user-supplied text: intern through a checked lookup, never construct freely.
> `ok_or_else` avoids eagerly formatting the error string when there's no error
> (a small, idiomatic allocation-avoidance).

### 7.3 The text format and modularity

`parse_script` turns a line like `patch euclidean.triggers scale.trigger` into a
`HostCommand::Patch`. It's a tiny recursive-descent parser over `split_whitespace`
tokens, with `.split_once('=')` for `k=v` params and `host_name()` for
interning. Keeping the parser a pure function `&str -> Result<Vec<HostCommand>>`
(no I/O, no side effects) makes it trivially testable and lets both the CLI and the
Tauri bridge command (`run_host_script`) share it.

Then `main.rs` is a ~50-line headless smoke binary: read stdin-or-file → parse →
run → report the bounce path and a `summarize()` of log events, underruns, and
the master out node. That's the whole "no frontend" acceptance criterion,
satisfied in production code (not a test).

---

## 8. The rustism index (skim this)

A cheat sheet of the idioms this codebase leans on, each already discussed above,
pulled together for quick reference.

| Idiom | Where | What it buys you |
|---|---|---|
| **Newtype struct** (`NodeId(pub u64)`) | `graph.rs` | Distinct types for distinct ids; compiler forbids mixing them. |
| **`&'static str` as key** | ports, plugins, params | `Copy`, comparable, hashable, zero-cost; forces a *closed* vocabulary. |
| **`enum` as typed union** | `Event`, `SignalKind` | Exhaustive `match`; data inline with each variant; new variant = compile error until handled. |
| **`Box<dyn Trait>`** | `AudioNode`, `Plugin`, `Disposer` | Type-erased polymorphism; heterogeneous collections; the "interface + impl" pattern. |
| **Const generics** (`EventBuf<T, CAP>`) | `graph.rs` | Fixed-capacity, inline, allocation-free buffers; size checked at compile time. |
| **Trait default methods** (`set_param {}`) | `AudioNode` | Optional behaviour without a second trait. |
| **`partition_point`** | `clock.rs`, `graph.rs` | Binary-search insertion/query without off-by-one bugs. |
| **`debug_assert!`** | everywhere | Check invariants in dev, compile them out in release; never panic on the audio thread. |
| **`.expect()` for invariants** | `clock.rs` | "This can only fail if my code is wrong" — a documented hard contract. |
| **Destructure `self` for disjoint borrows** (`let Engine { .. } = self`) | `render.rs` | Borrow multiple fields of `self` mutably at once. |
| **`move` closures as disposers** | `plugins/*` | The inverse of a registration is a *value* that captures its cleanup. |
| **`unsafe` with `SAFETY:` comments** | `ring.rs` | Narrow, reasoned, encapsulated unsafety wrapped in a safe API. |
| **`Arc<AtomicU64>` / `f32::from_bits` + `AtomicU32`** | `media/*`, `mixer.rs` | Lock-free, non-blocking counters/meters across threads. |
| **`Ordering::Relaxed` vs `Acquire/Release`** | `ring.rs` | Use the *loosest* ordering that's still correct; document the happens-before edge. |
| **`Drop` for best-effort cleanup** | `wav.rs`, `record.rs` | RAII finalization; `recover()` covers the no-drop (crash) case. |
| **`ok_or_else` (lazy error)** | `host.rs` | Don't format an error string unless there actually is one. |
| **Two-phase validate-then-apply** | `render.rs` | Fail loud and synchronously; apply sample-accurately later; never log a refusal. |
| **Let-chains** (`if let ... && let ...`) | `capture.rs`, `stream.rs` | Guard multiple `let` bindings without nested `if`s (recent rustc). |

---

## 9. Pros and cons of the key decisions

An architecture explainer that only praises its subject is marketing. Here is the
honest ledger, decision by decision — the project's own notes record alternatives
considered, and the code is candid about its edges.

### Decision: minimal model-free core + everything-as-plugin

**Pros**

- New capabilities (euclidean rhythm, a clip editor, offline processes) are
  plugins that *compose* rather than core changes that *fork*. The core stays
  small and testable.
- The core is genuinely reusable: the same clock/log/graph serves an arranger and
  (later) a generative improviser or a headless box, because it encodes no product
  assumptions.
- Enforced by the seam: `engine`/`media` compile with no `tauri` dependency, so
  swapping the shell (headless CLI today, Tauri later, possibly egui or WASM
  after) is provably possible, not just claimed.

**Cons**

- Up-front ceremony: you pay for the plugin discipline (factories, ports, params,
  inject, disposers) before it clearly pays off. The notes call this "ceremony
  without payoff" as a *risk to watch*, and the counter is to cap the machinery at
  exactly two hosts.
- The core must resist accretion. Every new idea tempts you to add "just one more"
  event type or port kind. The notes name "core creep" as a standing risk.
- Some seams are *declared* but not yet exercised (MIDI/OSC traits in `plugins`)
  — they're promises until a real provider lands.

### Decision: log everything, render is a pure function of the log

**Pros**

- **Determinism** — same log, byte-identical bounce — is a *tested* property, not a
  hope. That's gold for debugging, regression testing, and (later) undo/fork/
  resume.
- Sample-accurate lifecycle: every change takes effect at an exact frame, even mid-
  block.
- Refusals are never logged, so bad input can't corrupt history.

**Cons**

- Only *discrete* facts are logged now. **Automation curves** (a fader ride) are a
  *not-yet-implemented* future event type; the log's stated design is to coalesce
  control-rate streams into gestures, but that's unshipped. Until then, a smooth
  fade is a staircase of `SetParam` events or nothing.
- The flat `Event::Mount` payload is `Vec<(&'static str, f32)>` — it *cannot*
  carry a file handle, so media mounts (a clip's `FilePlayer`, a recorder) bypass
  the core log and use a parallel command seam. The notes record this as a
  "core-shape finding" to be resolved in P1.3 (merging media commands into the
  log). It's an honest, temporary seam, but it means determinism is *engine*
  determinism plus *media* determinism kept in sync by discipline, not one system.

### Decision: the graph is a typed patch bay with a forward-order rule

**Pros**

- Type-checked wiring — you can't connect a knob to an audio input, at the level
  of a `connect()` error.
- Forward-order-only makes rendering a single linear pass: no cycles, no
  topological sort, dead simple, and the no-allocation property is easy to hold.
- PDC is a first-class part of the graph *value* from day one, which the notes
  argue is far cheaper than retrofitting.

**Cons**

- **No feedback.** A delay feeding itself — the single most common effect in dub
  music, which the product explicitly cites as inspiration — is *not expressible*
  yet. The forward-order rule is the direct cause.
- **One audio `Out` per node** (Phase 1). A real node that produces two outputs
  (a crossover, a send/return) can't be represented until this is relaxed.
- Control inputs are single-driver; fan-in for control (a "sum the LFOs" case)
  awaits a later phase.

### Decision: no allocation, no blocking on the render path

**Pros**

- The one rule that makes real-time audio actually work; enforced by a
  counting-allocator test and by an API shape that hands nodes preallocated
  buffers.
- Because it's structural (not tribal knowledge), new nodes are *forced* into the
  cheap path — you write to the buffer you're given.

**Cons**

- Pervasive fixed-capacity buffers mean **silent drops** at the edges: event
  floods drop triggers, full rings drop monitor samples (counted, not silent, but
  still dropped), and voice *management* is explicitly deferred — `ToneGen` has a
  `MAX_BLIPS = 16` hard cap and drops notes beyond it. The notes call voice
  stealing "undesigned" (RESEARCH §14 risk 8). None of this is *wrong* for a
  spike, but it's debt that becomes audible the moment polyphony exceeds 16.
- `VecDeque::with_capacity(8)` in the player's splice buffer is a documented
  "would allocate on the render path past 8" edge — a real, if unlikely, footgun.

### Decision: the headless host before, and as a gate on, the Tauri shell

**Pros**

- Proves the contract is real — a contract exercised by exactly one host is a
  wish; this is exercised by a CI-tested headless binary running real scripts.
- Keeps the profile *logic* out of Vue by construction: the headless host has to
  run it, so it can't hide in a component.
- `engine`/`media` stay `tauri`-free, which is the entire "interchangeable UI"
  claim.

**Cons**

- Slower to a visible product: you get a CLI before you get a window. For a
  hobby project that might *feel* like a detour, even if it's the right one.
- The media commands (`Play`/`Splice`/`Bounce`) are not yet folded into the core
  log — there are two command vocabularies until they merge (the arrangement ops
  already go through the logged dispatch envelope, §4.1).

### Decision: hand-rolled std-only WAV and SPSC ring

**Pros**

- Zero dependencies in the core; full control over the crash-recovery semantics
  (a take recoverable from the first byte), which an off-the-shelf writer might
  not give you.
- The ring and WAV code are *educational* — small, exhaustively tested, and
  honestly commented.

**Cons**

- The RESEARCH doc *itself* lists `rtrb` (realtime-safe SPSC ring) and `hound`
  (WAV I/O) as the crates you'd adopt rather than hand-roll, and the WAV code's
  own docs say "`hound` stays the Phase-1 upgrade if format edge cases
  (WAVEFORMATEXTENSIBLE, 24-bit, …) bite." So the current implementations are
  explicitly *placeholders with known limits* — 16-bit PCM and 32-bit float,
  mono/stereo only — not production codecs.
- Hand-rolled `unsafe` (the ring) is a maintenance liability versus a vetted
  crate, no matter how well-commented.

---

## 10. Where this can go wrong (honest risks)

Beyond the per-decision cons above, a few systemic risks worth naming:

1. **Voice management is undesigned.** The moment euclidean drives real
   polyphony, `MAX_BLIPS = 16` bites. It's deferred, consciously, but it's the
   schedule's loudest ticking clock.
2. **Two command vocabularies** until P1.3 merges media commands into the core
   log. Every day they stay split is more client code that learns both.
3. **Ceremony without payoff** — the whole plugin machinery must earn its keep
   via at least the second host; if the project stalls before Tauri, the elegant
   seams look like overhead.
4. **Determinism is split-brain** — engine replay is byte-identical, but media
   determinism rests on a parallel command seam and matched discipline, not one
   log. A subtle divergence there would be *very* hard to debug.
5. **Stated-present-tense discipline.** This codebase lives by the rule that
   shipped notes describe *reality*, and docs written against an older commit go
   quietly wrong — this document itself drifted within days (it said "three
   crates" the week the fourth landed). The standing mitigation is the
   `Last verified against commit …` banner at the top of each explainer doc plus
   the notes-tree verifier; if you find drift, trust the code and move the line.

---

## 11. The theory of the program — and the Naur debate, applied

This architecture is a *theory of a program* in Peter Naur's 1985 sense
([*Programming as Theory Building*](https://pages.cs.wisc.edu/~remzi/Naur.pdf)):
the code, docs, and specs are by-products; the primary product is the mental model
that makes them cohere, and that model is know-how (Ryle's sense), not rules. For
this codebase the theory is one sentence — **a composition is a log; time, wiring,
history, and capability are all plugins on a minimal core; render is a pure
function of the log** — and the two governing invariants of §1.3 are its
load-bearing walls. A full write-up lives in
[`docs/theory-of-the-program.md`](theory-of-the-program.md).

How the current debate bears on it:

- **"The theory cannot be recovered"** is a *philosophical* claim, not a practical
  one. You *can* build a working model of a codebase you didn't write (Goedecke is
  right — and it's the only option in a large, high-turnover system). But you
  cannot recover the *original* theory — the constraints, the rejected options,
  the "why shaped this way and not that" — purely from the text. Those live in the
  decisions you had to make, and they're exactly what this repo's notes and
  banners try to externalize.
- **"Theory is one value you can trade off"** underestimates it. Speed, deps,
  accessibility, "keep it simple" aren't *opposed* to the theory — they're *part*
  of it. Here, the no-alloc rule, the forward-order rule (which sacrifices feedback
  delay — the dub staple), the dual command vocabulary, and the deferred voice
  management are *the theory as it stands*, not failures to maintain it. A
  modification that ignores them is a patch; one that extends them is grounded in
  it.
- **The project's own discipline refuses Naur's bleakest conclusion.** He says a
  program dies when the team holding its theory dissolves. This repo externalizes
  as much theory as can be: `## Alternatives considered` on every decision, notes
  that state *shipped* reality in present tense, the `Last verified against a
  commit` banner, and a model-co-work routing table. It is an experiment in
  keeping one coherent theory alive across authors who turn over constantly.

The practical lesson, at this scale: the codebase genuinely fits in one head, so
Goedecke's "partial understanding is the best you can do" doesn't bite *yet* — and
the infrastructure is already in place to keep it that way as it grows.

---

## 12. Where to look next

If you want to go deeper, in a sensible order:

1. **`crates/engine/src/render.rs`** — the engine loop is the heart; re-read
   `render_block` with §4.3 in mind.
2. **`crates/media/src/ring.rs`** — the single best piece of Rust to study
   closely: `unsafe`, memory ordering, false-sharing, and tests that stress it
   across threads.
3. **The tests** — `crates/engine/tests/spike_a.rs`, `phase1.rs`, `phase1_mixer.rs`,
   and `crates/media/tests/*`. They encode *behavior as specification* in a way
   the prose never can, especially the counting-allocator and byte-identical
   bounce tests.
4. **`crates/host/src/lib.rs`** — `run_script` + `parse_script` together are the
   whole product in miniature, and the best place to see the Host API contract
   exercised end to end.
5. **The decision notes** — start with
   [`minimal-core`](../.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md)
   and
   [`ui-as-plugin`](../.agents/notes/implemented/architecture/2026-08-18-ui-as-plugin-host-api-and-headless-reference.md),
   then the `implemented/` notes for what actually shipped and why.

And if you internalize only two things, make them these:

1. **Time, render, log, then everything else is a plugin** — the core answers
   *when*, *how it's wired*, and *what happened*, and nothing more.
2. **The render path never allocates or blocks, and rendering is a pure function
   of the log** — those two rules explain every `unsafe`, every `EventBuf`, every
   `debug_assert`, and every `Arc<AtomicU64>` in the codebase.

---

*Authored by an earlier GLM-5.3 Flash · ZCode session; drift-corrected against
commit `a04288d` with GLM-5.3 Flash · ZCode, 2026-08-27; re-verified against
`7c5a2e7` with DeepSeek-V4-Flash · DeepSeek Harness, 2026-09-05.*
