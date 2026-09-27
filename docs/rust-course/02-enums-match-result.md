# Lesson 2 — Enums, `match`, `Option`, and `Result`

**Material:** [`crates/engine/src/log.rs`](../../crates/engine/src/log.rs) (86 lines),
plus enum usage from `graph.rs`.

The session log is 86 lines and is arguably the most important file in the
project — "a composition *is* a log". It's also the perfect vehicle for Rust's
enums, because an event log *is* a list of typed alternatives.

## 2.1 Enums with data: not your C enum

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Mount {
        plugin: &'static str,
        params: Vec<(&'static str, f32)>,
        at_frame: u64,
    },
    ScheduleUnmount { plugin: &'static str, at_frame: u64 },
    Patch { /* ... */ },
    SetTempo { bpm: f64, beats_per_bar: u32, at_frame: u64 },
    SetParam { plugin: &'static str, param: &'static str, value: f32, at_frame: u64 },
    Arrangement { op: &'static str, fields: Vec<(&'static str, Value)>, at_frame: u64 },
}
```

In C/Java an enum is a named integer. In Rust an enum is a **sum type**: each
*variant* can carry its own data. A value of type `Event` is exactly one of
these six shapes, with its payload inline. This one construct replaces:

- class hierarchies (`MountEvent extends Event`),
- tagged unions,
- "type" discriminator fields plus downcasts.

Two details worth pausing on:

- **`&'static str`** means "a string that lives for the whole program" — true of
  string literals like `"euclidean"`. It's `Copy`, comparable, hashable, zero
  allocation. The codebase uses it as its vocabulary of names (plugin names,
  port names). Lesson 8 shows how user input gets *interned* into this closed
  set.
- The struct-variant syntax `Name { field: Type }` is just sugar; variants
  could also be tuple-like `Some(u64)` or unit-like `None`.

## 2.2 `match`: the only way in

You cannot access variant data without matching (there's no downcast):

```rust
fn describe(event: &Event) -> String {
    match event {
        Event::SetTempo { bpm, at_frame } =>
            format!("tempo {bpm} at frame {at_frame}"),
        Event::SetParam { plugin, param, value, .. } =>
            format!("{plugin}.{param} = {value}"),
        Event::Arrangement { op, fields, .. } =>
            format!("arrangement op {op}, {} fields", fields.len()),
        other => format!("{other:?}"), // catch-all via Debug
    }
}
```

Rules of `match`:

- **Patterns destructure.** `{ bpm, at_frame }` binds the fields as new
  variables (they borrow, since we matched on `&Event`).
- **`..`** ignores remaining fields.
- **Exhaustiveness is enforced by the compiler.** Add an `Event::Foo` variant
  and every match that doesn't handle it becomes a compile error. This is why
  the codebase can say "adding a new event type is a compile error everywhere
  until it's handled" — that's a feature you get for free by choosing an enum.
- `_` or a bound variable (`other`) covers the rest.

## 2.3 `Option<T>`: null, but checkable

`Option<T>` is just an enum in the standard library:

```rust
enum Option<T> { Some(T), None }
```

From `Scheduler`:

```rust
pub fn peek_frame(&self) -> Option<u64> {
    self.entries.first().map(|(f, _)| *f)
}
```

There is no `null`. "Might not be there" is encoded in the return type, and the
compiler refuses to let you use the value without handling `None`. Compare with
the Java version of this function returning `Long` and throwing somewhere
far away.

Common idioms you'll see constantly in this repo:

| Idiom | Meaning |
|---|---|
| `opt.unwrap()` | assume `Some`; panic if `None` (tests/prototypes only) |
| `opt.expect("msg")` | same but with a message; used for *invariants* |
| `opt.map(f)` / `and_then(f)` | transform if present |
| `opt.unwrap_or(default)` / `unwrap_or_else(f)` | default when `None` |
| `if let Some(x) = opt { ... }` | run a branch only when present |

## 2.4 `Result<T, E>`: errors as values

Also just an enum:

```rust
enum Result<T, E> { Ok(T), Err(E) }
```

The engine's whole public API returns `Result<(), String>`:

```rust
// render.rs
pub fn set_param(&mut self, plugin: &'static str, param: &'static str, value: f32) -> Result<(), String>
```

The discipline here is worth copying into your own code: **validate now, fail
loud, never log a refusal.**

```rust
if !value.is_finite() { return Err(...); }              // NaN/∞ banned
if value < min || value > max { return Err(...); } // range check // range check
```

Error propagation uses the `?` operator: "if this is `Err`, return it from my
function now; otherwise unwrap the `Ok`":

```rust
let idx = self.lookup(plugin)?;   // early-return the error if absent
do_thing(idx)?;
Ok(())
```

`?` is syntactic sugar for a `match`. It only works in functions returning
`Result` (or `Option`). You will read hundreds of `?` in real Rust; internalize
it now.

## 2.5 Closed vocabularies: `SignalKind`

`graph.rs` has another instructive enum:

```rust
pub enum SignalKind { Audio, Control, Trigger, Note }
```

Four kinds of signal, and `Graph::connect` returns an error when a cord joins
mismatched kinds. Because these are enum variants rather than strings, the
compiler enforces the vocabulary everywhere a `SignalKind` flows — there is no
`"audio "` typo possible. "Make illegal states unrepresentable" starts here.

## Your turn

⭐ **1.** Write a function `fn n_params(e: &Event) -> usize` in a scratch test
that returns the number of params for `Mount`, `0` for everything else. Let the
compiler force you to handle all six variants.

🔧 **2.** In `log.rs`, add a variant `Event::Marker { label: &'static str, at_frame: u64 }`.
Run `cargo test -p engine` and catalogue every place the compiler complains.
That error list *is* the payoff of sum types.

🔧 **3.** Implement `SessionLog::frames(&self) -> Vec<u64>` returning each
event's `at_frame`. Every variant carries `at_frame`, so a single match arm per
variant works — notice how repetitive that is, and consider what a shared
trait method would buy (preview of Lesson 3).

🔧 **4.** Change `peek_frame`'s body to use `map_or` instead of `.map()` and
convince yourself both versions are equivalent.

## Checkpoints

1. Why does the codebase prefer enums over string constants for signal kinds?
2. What does `?` do, and where is it allowed?
3. When is `.expect()` acceptable?

*(Answers: 1 — exhaustiveness and no invalid spellings; strings are open
vocabularies, enums are closed and compiler-checked. 2 — unwraps a `Result`/
`Option`, returning early on `Err`/`None`; allowed inside functions that return
`Result`/`Option` themselves. 3 — when the invariant violated would mean a
programming bug, not bad input; e.g. `clock.rs`'s "tempo map never empty".)*

Next: [Lesson 3 — Traits and generics](03-traits-and-generics.md)
