# Lesson 1 — Structs, methods, and ownership

**Material:** [`crates/engine/src/clock.rs`](../../crates/engine/src/clock.rs) (270 lines)

The clock is the best first file in the repo: it's pure std, small, and uses
almost no "clever" Rust. It defines three things: a `TempoMap` (frames ↔ beats), a `Clock` (where are
we now), and a `Scheduler<T>` (do this at exactly frame t) — plus the
`PatternQuery` trait a generator pulls its trigger frames through.

## 1.1 Structs: data with named fields

```rust
pub struct TempoMap {
    sample_rate: u32,
    segments: Vec<TempoSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoSegment {
    pub start_frame: u64,
    pub bpm: f64,
    pub beats_per_bar: u32,
}
```

Things to notice:

- **Fields are private by default.** `TempoMap.sample_rate` has no `pub`, so
  nothing outside `clock.rs` can touch it directly. Access goes through
  methods (`sample_rate()`, `segments()`). This is Rust's default-encapsulated
  stance: you opt *in* to exposure.
- **`TempoSegment` derives traits.** `#[derive(Debug, Clone, Copy, PartialEq)]`
  generates four implementations mechanically:
  - `Debug` → printable with `{:?}`
  - `Clone` → `.clone()` makes a copy
  - `Copy` → assignment copies instead of moving (allowed only for plain-data
    types; a `String` or `Vec` field forbids it)
  - `PartialEq` → `==` compares field by field
- **Integer types are explicit.** `u32`, `u64`, `f64`. There is no silent
  numeric coercion in Rust: `frame as f64` (seen below) is an explicit cast.

## 1.2 Constructors and methods

Rust has no constructors or `this`. A "constructor" is just an associated
function named `new`:

```rust
impl TempoMap {
    pub fn new(sample_rate: u32, bpm: f64, beats_per_bar: u32) -> Self {
        TempoMap {
            sample_rate,
            segments: vec![TempoSegment { start_frame: 0, bpm, beats_per_bar }],
        }
    }
}
```

Notes:

- `Self` means "the type this `impl` block is for".
- `sample_rate,` inside the struct literal is **field-init shorthand** — same
  as `sample_rate: sample_rate`.
- `vec![...]` builds a `Vec` with initial elements.

Methods take `self` in one of three flavors, and choosing between them is the
core daily decision of Rust ownership:

| Signature | Meaning | In clock.rs |
|---|---|---|
| `fn f(&self)` | read-only borrow | `tempo_at`, `beat_at` |
| `fn f(&mut self)` | exclusive mutable borrow | `push`, `advance` |
| `fn f(self)` | consumes the value | (none here — rare) |

```rust
pub fn advance(&mut self, frames: u64) {
    self.frame += frames;
}
```

## 1.3 The ownership rules (the actual heart of Rust)

1. Every value has exactly one **owner**.
2. You can have **many shared borrows (`&T`)** *or* **one mutable borrow
   (`&mut T`)** of a value at a time — never both.
3. When the owner goes out of scope, the value is dropped.

Why does the clock care? Look at who calls what. The render loop does
`clock.advance(...)` (needs `&mut`), while plugins read `clock.frame()`
(needs only `&`). Because `advance` requires `&mut self`, the compiler
guarantees nobody can be reading the frame *while* it's being advanced.
That guarantee is what later lets the codebase claim "rendering is a pure
function of the log" without locks.

A taste of what the borrow checker rejects — try compiling this mentally:

```rust
let map = Clock::new(48_000, 120.0, 4);
let r1 = &map;
let r2 = &mut map; // ERROR: cannot borrow `map` as mutable while borrowed as immutable
println!("{}", r1.seconds());
```

## 1.4 `Vec` and iteration

`Vec<T>` is a growable array — the workhorse collection. From `beat_at`:

```rust
let mut beat = 0.0f64;
for (i, seg) in self.segments.iter().enumerate() {
    if frame <= seg.start_frame {
        break;
    }
    ...
}
```

- `self.segments.iter()` yields `&TempoSegment` items (borrowing, not moving).
- `.enumerate()` wraps each item as `(index, item)`.
- `0.0f64` is a typed literal: "zero point zero, as f64". Suffixes (`u64`,
  `f32`) attach types to literals inline.

Also note the type conversions — always explicit, often via `as`:

```rust
beat += frames as f64 * seg.bpm / 60.0 / self.sample_rate as f64;
```

(`as` between number types is a plain cast; between other kinds of types you'll
see `From`/`Into`/`TryFrom` in later lessons.)

## 1.5 Generics preview: `Scheduler<T>`

You'll meet generics properly in Lesson 3, but the file ends with one and it's
the simplest possible form:

```rust
pub struct Scheduler<T> {
    entries: Vec<(u64, T)>,
}
```

`Scheduler<T>` is "a scheduler of *something*". The engine instantiates it as
`Scheduler<SchedEvent>`; the unit tests instantiate it as `Scheduler<&str>`.
One implementation, many payload types — that's all a generic parameter is.

## Your turn

⭐ **1. Run the tests and read them against the code.**

```sh
cargo test -p engine clock
```

Open `scheduler_delivers_exactly_at_frame` at the bottom of `clock.rs` and
trace each assertion through `schedule` / `peek_frame` / `drain_until` by hand
before running it.

🔧 **2. Add a method to `Clock`:**

```rust
/// Bars elapsed since frame 0, derived from the tempo map.
pub fn bar(&self) -> f64
```

(Hint: `beat()` divided by the current meter: `self.tempo_map.meter_at(self.frame) as f64`.)
Write one test for it in the existing `mod tests`.

🔧 **3. Deliberately break the borrow checker.** In a scratch test, take two
`&mut` borrows of the same `Clock` and watch the error. Read the error message
carefully — rustc's errors are genuinely good teaching material. Then fix it by
reordering the borrows.

🔧 **4. Make `TempoSegment` not `Copy`** (delete `Copy` from the derive). What
errors appear where? This shows you exactly which code relied on copy
semantics. Put it back afterwards.

## Checkpoints

1. Why is `segment_at` allowed to return `&TempoSegment` instead of cloning?
2. What's the difference between `iter()` and `into_iter()` on a `Vec`?
3. When must a method take `&mut self`?

*(Answers: 1 — it borrows `self`, which outlives the call; no copy needed.
2 — `iter()` yields references and leaves the Vec intact; `into_iter()` yields
owned values and consumes the Vec. 3 — whenever it assigns to or mutates any
field, e.g. `push`, `advance`.)*

Next: [Lesson 2 — Enums, match, Option & Result](02-enums-match-result.md)
