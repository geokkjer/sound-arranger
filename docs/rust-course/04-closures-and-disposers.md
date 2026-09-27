# Lesson 4 — Closures and the disposer pattern

**Material:** [`crates/engine/src/plugins/euclidean.rs`](../../crates/engine/src/plugins/euclidean.rs)
(170 lines, and its `apply` is the whole lesson).

Closures — functions that capture variables from their surroundings — are the
feature this codebase leans on most for its architecture. The key idea of the
plugin system, "an effect carries its inverse", is literally a closure value.

## 4.1 Closure syntax

```rust
|x| x + 1              // shortest form; type inferred
|key: &str, default: f32| ...   // explicit types
```

You can already see one at work in `euclidean_factory`:

```rust
let get = |key: &str, default: f32| {
    params
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| *value)
        .unwrap_or(default)
};
```

`get` captures `params` (the function argument) by reference and uses it in the
body. Calling it looks like any function call: `get("steps", 8.0)`. This
"lookup with default" chain — `iter().find().map().unwrap_or()` — is a very
idiomatic iterator pipeline; spend a minute reading it left to right:
iterate → find matching pair → extract value → or fall back.

## 4.2 The three closure traits

Every closure implements one or more of:

- **`Fn`** — callable many times; only *reads* what it captures.
- **`FnMut`** — callable many times; may *mutate* captures.
- **`FnOnce`** — callable *once*; may *consume* (move out of) captures.

The compiler infers the weakest sufficient trait. Where do you see them? Two
places in files you've read:

```rust
// clock.rs — the PatternQuery seam; a callback we invoke repeatedly per block:
fn for_each_trigger(&self, block: Range<u64>, tempo: &TempoMap,
                    emit: &mut dyn FnMut(u64));
//                                        ^^^^^ FnMut: called N times,
//                                              may mutate its captures

// plugins/mod.rs — teardown, by definition single-use:
pub type Disposer = Box<dyn FnOnce(&mut DisposerCtx<'_>)>;
//                             ^^^^^^ called exactly once
```

Choosing the trait is part of API design: ask "how many times will callers run
this, and does it need to change captured state?"

## 4.3 `move`: taking ownership into the closure

Now the star. `Euclidean::apply` mounts a node, provides a service, and returns
a disposer that undoes both:

```rust
Ok((
    node,
    Box::new(move |dis: &mut DisposerCtx| {
        dis.graph.remove_node(node);
        dis.ctx.remove("rhythm");
    }),
))
```

Walk through it slowly:

- The closure body references `node`, which is a local variable inside `apply`.
  When `apply` returns, that stack slot dies.
- **`move`** forces the closure to take ownership of `node` (copying the id —
  it's a plain `u64` newtype). The closure now *owns* everything it needs to
  undo.
- `Box::new(...)` puts the closure on the heap behind `Box<dyn FnOnce(...)>`,
  erasing its concrete type — the engine stores all disposers uniformly.
- Later, unmount calls it once: node removed, service removed.

Without `move`, the compiler would try to borrow `node` from the dying stack
frame and reject the code. When you see `move` in Rust, think: "this closure
must outlive the current function."

This is the architectural payoff: **teardown is constructed at mount time as a
value**, not written later as a symmetric method. Registration order, captured
ids, provided keys — all frozen into the disposer. It's dependency-injection
reversibility without any framework.

## 4.4 Returning closures: `impl Trait`

A lighter-weight cousin appears in `clock.rs`:

```rust
pub fn drain_until(&mut self, frame: u64) -> impl Iterator<Item = T> + '_ {
    let at = self.entries.partition_point(|(f, _)| *f <= frame);
    self.entries.drain(..at).map(|(_, p)| p)
}
```

`impl Iterator<Item = T> + '_` means "returns *some* iterator type; you don't
get to know which, but you know its item type, and it borrows self for as long
as self lives (`+ '_`)". No boxing needed. You'll use this constantly for
returning iterator chains.

## 4.5 Iterators are lazy pipelines

The test at the bottom of euclidean.rs is also worth reading as idioms:

```rust
let shifted = base.iter()      // &bool items
    .cycle()                   // loop forever
    .skip(6)                   // drop first 6
    .take(8)                   // keep next 8
    .copied()                  // bool: &bool -> bool
    .collect::<Vec<_>>();      // materialize
```

Nothing runs until `collect`; each adapter wraps the previous. And note the
turbofish `collect::<Vec<_>>` — when the target type can't be inferred, you
spell it inline like this. Alternative spelling: `let shifted: Vec<bool> = ...collect();`.

## Your turn

⭐ **1.** Run `cargo test -p engine euclidean` and read `euclid_is_maximally_even`
until you understand why `windows(2)` + the wraparound push checks evenness.

🔧 **2.** Write a closure-capture experiment in a scratch test:

```rust
let mut count = 0;
let mut bump = || count += 1;   // infers FnMut
bump();
bump();
assert_eq!(count, 2);
```

Then try calling a `FnOnce` twice and read the error.

🔧 **3.** Add a second contribution to the euclidean plugin's `apply`: provide a
service under `"euclidean.config"` containing steps/pulses/rotation, and extend
the disposer to remove it. Run the plugin tests — the existing teardown tests
will verify your disposer actually undoes both.

🔧 **4.** Rewrite `drain_until`'s return type as
`Box<dyn Iterator<Item = T> + '_>` and confirm the tests still pass. Then put
`impl` back and articulate (out loud, even) why `impl` is preferable here.

## Checkpoints

1. Why must the disposer be `FnOnce` rather than `FnMut`?
2. What breaks if you delete `move` from the apply closure?
3. `emit: &mut dyn FnMut(u64)` — decode every token.

*(Answers: 1 — teardown consumes the captured cleanup state; running twice
would be unsound, so the type forbids it. 2 — borrow-check error: the closure
would borrow locals that die before the disposer is called. 3 — a mutable
reference to a dynamically-dispatched callable taking `u64`, callable many
times.)*

Next: [Lesson 5 — Lifetimes and shared borrows](05-lifetimes-and-borrowing.md)
