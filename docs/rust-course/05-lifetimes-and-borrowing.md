# Lesson 5 — Lifetimes and shared borrows

**Material:** [`crates/engine/src/render.rs`](../../crates/engine/src/render.rs)
(especially `apply_mount`), plus `clock.rs`'s `drain_until`.

Lifetimes are the part of Rust with the scariest reputation and the smallest
actual daily footprint: in most code they're *inferred*, and you only write
annotations when the compiler can't figure out a connection between references.
This lesson shows the two or three places where this codebase actually has to
write them — which is exactly the set you need to understand as a reader.

## 5.1 What a lifetime *is*

A lifetime is the region of code during which a reference is valid. The borrow
checker's core rule, restated in lifetime terms:

> A reference must never outlive the data it points at.

Most of the time this is checked silently:

```rust
let map = TempoMap::new(48_000, 120.0, 4);
let segs = map.segments();   // &'map [TempoSegment] — inferred
use(segs);
// map dropped here; segs already unused. Fine.
```

It goes wrong when you try to return a reference derived from an argument
without telling the compiler how the inputs and outputs relate:

```rust
fn longest(a: &str, b: &str) -> &str { ... }   // ERROR: which input does the output borrow from?
```

That's when you annotate:

```rust
fn longest<'a>(a: &'a str, b: &'a str) -> &'a str { ... }
```

Read `'a` not as "setting" anything but as *naming* a constraint: "the returned
reference is valid exactly as long as both inputs are." Three lifetime rules
cover most annotation cases (the "elision rules"); when they don't apply, the
compiler tells you precisely what name to add.

## 5.2 In the wild #1: `drain_until` and `+ '_`

```rust
pub fn drain_until(&mut self, frame: u64) -> impl Iterator<Item = T> + '_ {
    let at = self.entries.partition_point(|(f, _)| *f <= frame);
    self.entries.drain(..at).map(|(_, p)| p)
}
```

The returned iterator borrows `self.entries`. `+ '_` says so: "this value holds
a borrow of self; while it exists, nobody else may touch self." The compiler
then enforces it — try to mutate the scheduler while holding the drain
iterator and you'll get an error. This is one of those annotations that reads
like bureaucracy but is actually the feature working.

## 5.3 In the wild #2: structs that hold borrows — `PluginApi<'a>`

```rust
pub struct PluginApi<'a> {
    pub ctx: &'a mut Context,
    pub scheduler: &'a mut Scheduler<SchedEvent>,
    pub graph: &'a mut Graph,
    pub clock: &'a Clock,
}
```

A struct can hold references, and then it needs a lifetime parameter saying
"all these borrows live for `'a`". Why does this exist? Because of a rule
you'll hit constantly:

**You cannot take two `&mut` borrows through one path.** Given
`&mut self.engine`, calling something that wants `&mut ctx` and `&mut graph`
and `&mut scheduler` simultaneously is refused — the compiler sees them all as
going through the same `&mut Engine`. But borrowing *different fields* is fine;
that's called **disjoint borrows**.

## 5.4 The destructure-`self` trick

Here's the idiom that unlocks it, from `apply_mount`:

```rust
let (node, disposer) = {
    let Engine { ctx, scheduler, graph, clock, .. } = self;
    let mut api = PluginApi { ctx, scheduler, graph, clock };
    plugin.apply(&mut api)?
};
```

`let Engine { .. } = self` destructures `&mut Engine` into separate bindings,
one per field. Each binding is its own borrow, and the borrow checker sees
them as disjoint — so bundling all four into a `PluginApi` is legal. Without
the destructuring, constructing `PluginApi` directly from `self.ctx`,
`self.graph`, ... would be rejected.

When reading unfamiliar Rust, `let StructName { field, .. } = thing;` almost
always means "I need to split this struct apart for the borrow checker."
Recognize it on sight; it appears whenever someone hands multiple pieces of
state to a helper.

(The inner `{ }` block scopes the borrows: `api` and the field borrows die at
its end, releasing them before the next lines use `self.scheduled` etc.)

## 5.5 `let ... else`: early-return binding

From `apply_patch`:

```rust
let Some(&from_node) = self.node_of.get(from.0) else {
    debug_assert!(
        false,
        "patch endpoint '{}' not mounted at apply (log-order error)",
        from.0
    );
    return;
};
```

`let Some(x) = expr else { ... };` — if the pattern doesn't match, run the
`else` block, which *must* diverge (`return`, `continue`, panic). It's the
idiomatic replacement for nested `match`/`if let` when the `None` case is just
an early exit. Note also `Some(&from_node)`: the pattern destructures and
copies the `NodeId` out of the map entry in one step.

## 5.6 Reading lifetimes you didn't write

You'll meet annotated signatures like:

```rust
pub fn events(&self) -> &[Event]              // elided: borrows self
pub fn segments(&self) -> &[TempoSegment]
```

Elision fills these in: one `&self` input ⇒ output borrows from `self`.
So `SessionLog::events()` returning `&[Event]` can never dangle — it lives as
long as the log. When you read a signature, ask only: "what does the returned
reference borrow from?" That's the whole question lifetimes answer.

## Your turn

⭐ **1.** In `clock.rs`, delete `+ '_` from `drain_until` and read the compiler
error. It will literally explain the missing bound. Restore it.

🔧 **2.** Write a function with two reference args and a reference return; see
when elision suffices and when it doesn't:

```rust
fn first_word(s: &str) -> &str          // works without annotations
fn longer<'a>(a: &'a str, b: &'a str) -> &'a str   // needs the name
```

🔧 **3.** In `render.rs`, replace the destructuring in `apply_mount` with direct
field access (`ctx: &mut self.ctx,`) and study the error. Then restore. You
want this error to feel familiar, not frightening.

🔧 **4.** Convert one `if let Some(x) = ... { ... }` in render.rs to
`let ... else` form where the negative branch just returns, and vice versa.
Get fluent translating between the two shapes.

## Checkpoints

1. What does `PluginApi<'a>`'s `'a` constrain?
2. Why is `let Engine { ctx, graph, .. } = self;` legal while passing
   `&mut self.ctx` and `&mut self.graph` together isn't?
3. What must the `else` block of `let ... else` do?

*(Answers: 1 — all four struct borrows share validity tied to `'a`; nothing
else may mutably borrow those fields meanwhile. 2 — destructuring produces
disjoint per-field borrows; chained field access looks like one shared `&mut
self` path to the checker. 3 — diverge: return/break/panic.)*

Next: [Lesson 6 — The no-allocation render path](06-no-allocation.md)
