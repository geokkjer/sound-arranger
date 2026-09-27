# Lesson 9 — Rust is multimodal: paradigms and design principles

**Material:** everything you've read so far, viewed from above. This lesson is
the theory debrief: what *paradigms* are in play, where each one lives in this
codebase, and how classic design principles (SOLID, KISS, DRY, YAGNI) show up
in Rust — which often expresses them differently than the Java/C# literature
assumes.

You've now written Rust for eight lessons without anyone naming the elephant:
Rust doesn't belong to one programming paradigm. It's **multimodal** — it
borrows functional, object-oriented, and procedural techniques, and (this is
the important part) expects you to *choose per problem*, sometimes within a
single function. Watch:

```rust
// clock.rs — three paradigms, ten lines apart

pub fn push(&mut self, start_frame: u64, bpm: f64, beats_per_bar: u32) {
    // PROCEDURAL: sequential mutation of state, step by step
    debug_assert!(...);
    self.segments.push(TempoSegment { start_frame, bpm, beats_per_bar });
}

pub fn tempo_at(&self, frame: u64) -> f64 {
    // FUNCTIONAL: compute an answer from inputs; touch nothing
    self.segment_at(frame).bpm
}

impl<T> Scheduler<T> {
    pub fn schedule(&mut self, frame: u64, payload: T) {
        // OBJECT-ISH: method on encapsulated state with an invariant
        // ("entries stays sorted") that callers can't break
        let at = self.entries.partition_point(|(f, _)| *f <= frame);
        self.entries.insert(at, (frame, payload));
    }
}
```

None of these is "better." They're answers to different questions: *how does
state change*, *what is derivable*, and *who may break my invariants*.

---

## 1. The functional mode: computation without (visible) mutation

Functional programming: prefer **pure functions** (same inputs → same output,
no side effects), immutable data, expressions over statements, and composition
of small transformations.

### Where it lives here

**Pure functions as anchors of trust.** `euclid(steps, pulses, rotation)` in
`plugins/euclidean.rs` takes numbers, returns a pattern, touches nothing. That
purity is why its test suite is exhaustive and trivial to write — no mocks, no
setup, just input→output pairs. Same for `TempoMap::beat_at` / `frame_at`: the
*derived* musical position (Lesson 1's "derive, don't store" decision) is a
functional-programming instinct — treat stored state as the single source of
truth and everything else as a function of it.

**Iterator pipelines instead of loops.** From the euclidean test:

```rust
let positions: Vec<usize> = pattern.iter()
    .enumerate()
    .filter(|(_, p)| **p)
    .map(|(i, _)| i)
    .collect();
```

This is a declarative description of *what* you want, not *how* to loop. Each
adapter (`filter`, `map`) is itself a pure function; the pipeline composes
them. Crucially, in Rust these compile down to the same machine code as a
hand-written loop — zero-cost abstraction — so you never pay readability for
performance or vice versa.

**Event sourcing as a fold.** Step back and look at the architecture's central
claim: *"rendering is a pure function of the log"* (`render.rs`'s module doc).
That's functional thinking elevated to system architecture: the session log is
immutable history (append-only!), current model state is a **fold** — a
reduction — over that history, and replay (`replay_from`) re-runs the fold to
reproduce byte-identical audio. Determinism isn't luck; it's what you get when
you refuse hidden inputs (no wall clock, no randomness).

**Immutability by default.** `let x = ...` is immutable unless you write
`let mut`. The language nudges you functional-first and makes mutation a
deliberate, greppable choice (`mut` appears exactly where state changes).

### Why Rust likes this mode

Correctness. Pure functions are testable in isolation, safe to call in any
order, and trivially cacheable/replayable. When a bug report comes in, "which
pure function gives the wrong output?" is a much better question than "who
mutated this shared field?"

---

## 2. The object-oriented mode: polymorphism and encapsulation, minus inheritance

Rust is not a class-based OO language — there is no implementation inheritance,
no subclassing, ever. But the *good parts* of OO are all present:

### Encapsulation

`TempoMap`'s fields are private; access goes through methods that maintain
invariants (`push` enforces ascending order via `debug_assert`). This is
classic information hiding — the module boundary (`pub`, Lesson 8) is the
encapsulation unit, sharper than a class because it's compiler-enforced.

### Polymorphism

Two kinds, and knowing which to pick is a core Rust skill (Lesson 3):

```rust
Opaque(Box<dyn AudioNode>)     // runtime polymorphism: vtable, mix types
fn drain_until<T>(&mut self)   // compile-time polymorphism: monomorphized
```

### Composition over inheritance — enforced, not advised

OO wisdom says "favor composition"; Rust removes the alternative. Need shared
behavior? You don't extend a base class — you either compose (a struct holding
another) or define a trait. See the seam traits in `plugins/mod.rs`:

```rust
pub trait MidiSource: EventSource {}
```

That looks like inheritance but is a **supertrait bound**: "anything that is a
`MidiSource` must also provide `EventSource`'s methods." It's interface
refinement without any implementation coupling — no base-class fragility, no
diamond problem, because there's nothing to inherit.

### Where the OO mode lives here

The whole plugin architecture: `Plugin` and `AudioNode` are interfaces;
factories build implementations; the engine talks to `Box<dyn Plugin>` without
knowing a sine from a mixer. This is textbook strategy/factory patterns —
expressed in ~60 lines because the language carries the ceremony.

### Why Rust restrains this mode

Implementation inheritance couples subclasses to base-class internals (the
fragile-base-class problem) and makes behavior hard to reason about locally —
fatal in a language whose selling point is "you can understand what this line
does without reading the rest of the program." Traits give the substitutability
benefit; the borrow checker keeps the rest honest.

---

## 3. The procedural mode: imperative state machines where they earn it

Procedural programming: straight-line imperative code mutating state through
ordered steps. Unfashionable in theory, indispensable in practice — and this
codebase uses it precisely where hardware reality demands it.

### Where it lives here

**The render loop.** `graph.rs`'s render walks nodes in order, gathering
inputs into preallocated buffers, calling each node, applying PDC delay. It's
an imperative pipeline over mutable memory — because the audio callback *is*
an imperative contract: "by the deadline, these samples will be written."
Functional elegance is unavailable here (Lesson 6: no allocation, no
abstraction with hidden costs), so the code embraces the procedural style
openly.

**State machines with explicit transitions.** `apply_mount` / `apply_patch` /
the scheduler draining events mid-block — sequential steps where *order of
operations is the semantics*. The two-phase discipline ("validate now, apply
at frame") is procedural choreography.

**Small loops with accumulators.** `beat_at`'s `for` loop accumulating
fractional beats across segments. A fold would express the same thing, but
with early-exit (`break`) and index lookahead, the plain loop is clearer.
Rust's stance: iterators when transforming data, loops when the algorithm's
essence is stepping.

### Why this mode survives in Rust

Because Rust never pretends your program isn't running on real hardware with
real deadlines. The procedural mode is honest about time and memory — and this
project's two governing rules (sample accuracy; no allocation on the render
path) are both, at bottom, procedural concerns.

---

## 4. SOLID, translated into Rust

SOLID was formulated for class-based OO. It survives the translation well, but
each principle lands differently.

### S — Single Responsibility

*"One module, one reason to change."*

The four-piece core is the purest example you'll find outside a textbook:
`clock.rs` changes when *timekeeping* rules change; `log.rs` when *event
vocabulary* changes; `graph.rs` when *wiring* rules change; `ctx.rs` when
service plumbing changes. The project even guards the separation structurally —
`engine` compiles std-only, so device concerns *cannot* leak into it. When you
find yourself wanting `use std::fs` inside the engine, that's the SRP alarm.

### O — Open/Closed

*"Open for extension, closed for modification."*

Here's the most interesting tension in the codebase, because Rust lets you
choose **which axis is open**:

- **Traits are open**: add a new `AudioNode` implementation tomorrow and zero
  existing lines change. That's O/C — the plugin system exists to make new
  capabilities cheap (everything-is-a-plugin).
- **Enums are closed**: adding an `Event` variant forces edits at every
  `match`. That looks like an O/C violation but is a deliberate trade — the
  core's event vocabulary wants to be *small and closed* (minimal-core note's
  "core creep" risk). The compiler-enforced edit list is a feature: it makes
  the cost of growing the core visible.

So: open where variation is expected (node behaviors), closed where stability
is the product (core events). Java-style thinking would make both interfaces;
Rust makes you pick, per axis, and the pick is documented in the type.

### L — Liskov Substitution

*"Subtypes must be usable through the base type without surprises."*

No subclassing, so LSP becomes **trait contracts**: every `impl AudioNode`
must honor what callers assume about *any* `AudioNode` — `render` fills the
given buffers, never allocates, never blocks, reports honest `latency()` (PDC
depends on it!). Nothing in the signature says "don't block"; the contract
lives in docs, tests (counting allocator, spike_a.rs), and review discipline.
Rust's lesson: the type system can't carry all of LSP; some of it is culture,
and the tests are the culture made executable.

### I — Interface Segregation

*"Many small interfaces beat one fat one."*

`AudioNode` is four methods, two with default bodies. `PatternQuery` is one method.
`EventSource`/`EventSink` split input from output rather than making a
bidirectional mega-trait. Consumers depend only on what they use: the render
loop needs `render`; param UIs need only `mounted_params()`. Every default
method (`set_param {}`, `has_tail() -> false`, `params() -> &[]`,
`mounted_ports()`, `mounted_params()`) is ISP applied — optional capability
kept off implementors who don't care.

### D — Dependency Inversion

*"Depend on abstractions, not concretions."*

Three distinct mechanisms, worth telling apart:

1. **Trait objects**: the host drives `Box<dyn Plugin>`, media contributes
   `Box<dyn AudioNode>` nodes the engine renders without naming their types.
   Classic DI.
2. **Service injection**: `inject()` declares required services by *name*;
   `ctx.provide("rhythm", ...)` satisfies them; validation refuses unmet
   dependencies before anything logs. A service locator with fail-loud checks —
   less typed than constructor injection, but it works across mount-time
   boundaries where constructors can't reach.
3. **Direction of dependency**: media depends on engine (never reverse);
   host depends on both; the UI (an iced or ratatui shell) implements the host's
   contract rather than the core importing UI. Dependency arrows point toward
   the stable core — DIP at architecture scale.

---

## 5. The unacronymed classics

**KISS.** The forward-order-only graph rule (§3.6 of the explainer) is KISS as
an architectural decision: no cycles, no topological sort, a linear sweep —
trading away feedback loops (a known, documented limitation) for an interpreter
simple enough to hold in your head. Simple beats complete when the incomplete
version's limits are explicit.

**DRY.** Applied with judgment: `TempoSegment` fields repeat across
`TempoMap::new` and `push` (acceptable — near-duplicates), while the
validate/log/schedule sequence repeats deliberately in every engine mutation
(the *pattern* is factored conceptually; forcing it through a generic wrapper
would obscure the per-event differences). DRY is about knowledge having one
home — e.g., the mixer's channel surface is generated once from the mount's
`channels` by `channel_ports(n)`/`channel_params(n)`, and the same generated
surface is what the engine validates patches and parameters against — not
about textual similarity.

**YAGNI.** Everywhere, and load-bearing:

```rust
// patch-bay note, external-I/O seams: "traits now, implementations when demanded"
pub trait MidiSource: EventSource {}   // seam declared, no impl yet
```

Declared seams with no production implementation (only a test double exercises
`OscSource`), `MAX_BLIPS` voice-stealing deferred,
automation curves postponed — the notes track these as conscious debts, not
oversights. The counterweight is also recorded: "ceremony without payoff" is a
named risk, watched by capping the machinery at two hosts. YAGNI done well
writes down what it's deferring.

---

## 6. The synthesis: choosing a mode

A reading heuristic for this codebase — and a writing heuristic for your own:

| Question | Mode | Tell |
|---|---|---|
| Is this a *derivation* from stored truth? | functional | pure fn, iterators, no `mut` |
| Does *extension* happen here without touching this file? | OO/traits | `dyn Trait`, factory registries |
| Is *time/order/memory* the essence of the logic? | procedural | `&mut self`, loops, buffers |

And the deep reason Rust can afford all three: **ownership is the substrate**.
Borrowing makes immutability the default (functional mode affordable),
lifetimes make encapsulation verifiable (OO boundaries without runtime
overhead), and explicit `&mut` makes procedural mutation visible and auditable
(procedural mode safe). Other languages pick a paradigm and pay for it
everywhere; Rust makes the paradigm a per-function decision with the compiler
checking the receipt.

## Your turn

⭐ **1.** Paradigm census. Pick three files you haven't studied closely — say
`media/src/drift.rs`, `media/src/wav.rs`, `engine/src/value.rs`. For each
public function, tag it F (pure/derivation), O (trait dispatch /
encapsulation boundary), or P (procedural state change). Notice how few are
hard to classify, and what the hard cases have in common.

🔧 **2.** Find one O/C violation candidate: search for `match` on `SignalKind`
or `Direction` in graph.rs and count arms. If a fifth signal kind were added,
how many sites change? Now compare: adding a new node type changes zero. Write
two sentences on whether `SignalKind` should become open (it shouldn't — say
why in terms of the closed-vocabulary rule).

🔧 **3.** LSP audit: read every `impl AudioNode` and check the implicit
contract (fill buffers, no allocation/blocking, honest latency). Find any
implementation that would break PDC if its `latency()` lied, and trace what
downstream code trusts it.

🔧 **4.** Design exercise: sketch (types only, no bodies) how *feedback* — a
delay node feeding itself, forbidden by the forward-order rule — could enter
the graph without violating SRP or turning `SignalKind` open. Decide which
paradigm your solution leans on, and whether the core grows.

## Checkpoints

1. Why is the `Event` enum's closedness a feature despite looking like an
   open/closed violation?
2. Where does LSP live if there's no inheritance?
3. Name the three dependency-inversion mechanisms this repo uses.
4. What makes the render loop procedural *on purpose*?

*(Answers: 1 — the compiler turns each growth into an explicit, visible edit
list, pricing core creep; the vocabulary is meant to stay small. 2 — in trait
contracts upheld by every impl and enforced by tests/culture, not the type
system. 3 — trait objects, name-keyed service injection with validation, and
architecture-level dependency direction. 4 — its semantics are ordering and
deadlines over mutable, preallocated memory; abstraction there has audible
costs.)*

---

**Course complete.** You've gone from `struct Clock` to the philosophy of the
whole system. Re-read [the architecture explainer](../architecture-explainer.md)
one more time — it should now read like a review of code you already know.
