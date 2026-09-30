# Lesson 7 — Threads, atomics, and a lock-free ring

**Material:** [`crates/media/src/ring.rs`](../../crates/media/src/ring.rs) (212 lines —
the single best file in the repo to study closely).

Everything before this lesson was single-threaded. Now: the recorder's disk
thread produces samples, the audio callback consumes them, and they must never
block each other. `Spsc<T>` is a hand-rolled lock-free ring buffer that solves
it — and it's a guided tour of Rust's concurrency machinery.

## 7.1 The shape of the problem

```rust
#[repr(C, align(64))]
pub struct Spsc<T> {
    head: AtomicUsize,
    _pad0: [u8; 56],
    tail: AtomicUsize,
    _pad1: [u8; 56],
    count: AtomicUsize,
    _pad2: [u8; 56],
    slots: Box<[UnsafeCell<Option<T>>]>,
}
```

One producer pushes (`head`), one consumer pops (`tail`), `count` tracks how
many slots are occupied. No mutex anywhere. Before the clever parts, note the
simple ones:

- **`AtomicUsize`** — an integer with indivisible read-modify-write operations.
  `load`, `store`, `fetch_add`, `fetch_sub`.
- **`#[repr(C, align(64))]`** — control memory layout. `align(64)` places the
  struct on a 64-byte boundary (one cache line). Why? **False sharing**: if two
  threads write variables that happen to share a cache line, each write
  invalidates the other core's copy and performance craters. The `_pad` arrays
  force each atomic onto its own line.

The repo even *tests* the layout (and pins it at compile time with
`std::mem::offset_of!` in a `const _: () = { assert!(...) }` block — worth
reading as a party trick: compile-time assertions about memory layout).

## 7.2 `Send` and `Sync`: Rust's thread-safety types

Two marker traits govern what crosses threads:

- **`Send`** — safe to *move* to another thread.
- **`Sync`** — safe to *share* (`&T`) across threads.

They're auto-derived from a type's fields — which is exactly why this struct
must override them manually:

```rust
// SAFETY: the SPSC contract — one producer, one consumer, and the count
// protocol guaranteeing producer and consumer never touch the same slot —
// makes sharing the UnsafeCell slots sound.
unsafe impl<T: Send> Sync for Spsc<T> {}
```

Read this carefully; it's the most instructive five lines in the codebase:

- `UnsafeCell<Option<T>>` is the primitive that permits mutation through
  shared references — it *removes* your type's automatic `Sync`, because the
  compiler can't verify interior mutation is race-free.
- `unsafe impl Sync` is you telling the compiler "I take responsibility: here
  is the argument why sharing is sound."
- The **`// SAFETY:` comment is mandatory convention**, not decoration. Every
  `unsafe` block must cite its invariant. This one cites the SPSC contract.
- `<T: Send>` keeps a condition: you may share the ring across threads only if
  the payloads themselves are movable across threads.

The discipline to imitate: unsafety should be *narrow* (a few lines),
*commented* (cite the invariant), and *encapsulated* (the public API —
`try_push`/`try_pop` — is completely safe; no caller ever touches the unsafe
parts).

## 7.3 Memory ordering in one paragraph

Atomics come with an `Ordering` that says how much synchronization the
operation carries. This file uses two levels:

```rust
// producer:
*self.slots[idx].get() = Some(value);          // 1. write the slot
self.head.store(head.wrapping_add(1), Ordering::Relaxed);   // 2. bump index
self.count.fetch_add(1, Ordering::Release);    // 3. announce

// consumer:
if self.count.load(Ordering::Acquire) == 0 { return None; } // 4. check
let value = unsafe { (*self.slots[idx].get()).take() };     // 5. read the slot
```

The pair that matters: **`Release` (step 3) / `Acquire` (step 4)**. Together
they create a *happens-before* edge: everything the producer did before its
Release (including step 1) becomes visible to the consumer after its Acquire,
so step 5 sees a fully-written slot. Meanwhile steps 2 uses `Relaxed` — the
loosest ordering, no synchronization — because correctness doesn't depend on
when the index moves, only on `count`. Rule of thumb the file follows: use the
*loosest* ordering that's still correct, and document which edge carries the
guarantee.

## 7.4 The API: non-blocking by contract

```rust
pub fn try_push(&self, value: T) -> bool  // false = full, caller decides
pub fn try_pop(&self) -> Option<T>        // None = empty
```

Both take `&self` (!) yet mutate — legal only because of the atomics and the
`UnsafeCell`. Both return instantly; blocking policy belongs to callers. Look
at how the callers differ — `media/src/stream.rs` holds both sides of a clip
read:

- the reader thread (may block): sleeps 50 µs when `try_push` returns false
  (`stream.rs:163`);
- the render path (must not block): `pop_sample` pops once and counts an
  underrun if the ring is empty before the clip is done (`stream.rs:276`).

The recorder is the mirror image: `record.rs` pushes each captured frame and
counts an *overrun* when the ring is full (`record.rs:144`).

Same data structure, opposite policies — the separation is structural.

## 7.5 Threads and `Arc`

The stress test shows plain std threading:

```rust
let ring = std::sync::Arc::new(Spsc::new(1024));
let (pr, pc) = (ring.clone(), ring.clone());
let producer = std::thread::spawn(move || {
    for i in 0..N {
        while !pr.try_push(i) {
            std::thread::yield_now();
        }
    }
});
producer.join().unwrap();
```

- **`Arc<T>`** — atomically reference-counted shared ownership. `.clone()`
  bumps a counter; the last drop frees.
- Closures passed to `thread::spawn` must be `'static` + `Send` — hence
  `move` capturing the `Arc`s by value (Lesson 4 pays off).
- `join()` blocks until the thread ends and returns its result; `.unwrap()`
  because a panicking child yields `Err`.

Note what the compiler did for free: try changing the test to push from *two*
threads and it won't compile without reaching for something beyond `&Spsc` —
the SPSC contract is partly encoded in the type system.

## Your turn

⭐ **1.** Run `cargo test -p media ring` — including the cross-thread stress
test. Read `ordered_transfer` until the wraparound arithmetic
(`idx = head & (len - 1)` — why does capacity being a power of two matter?)
makes sense.

🔧 **2.** Write a small program (scratch test) using two channels of an
`mpsc::channel`, then rewrite it with `Arc<Mutex<Vec<u64>>>`, then imagine the
third version being this ring. You're calibrating your intuition for when each
tool applies: message passing / coarse locking / lock-free.

🔧 **3.** Delete one `_pad` field and the matching alignment assertion fails to
compile — run `cargo test -p media ring` to see the compile-time layout check
fire. Restore.

🔧 **4.** In the stress test, change `Ordering::Release` to `Relaxed` on the
producer's `count.fetch_add`. It will *usually still pass* — say out loud why
that makes it worse, not better. (Memory-ordering bugs are statistical.)
Restore.

## Checkpoints

1. Why must `unsafe impl Sync` be written by hand here?
2. What guarantee does Acquire/Release give that Relaxed doesn't?
3. Difference between `Arc` and `Rc`?

*(Answers: 1 — `UnsafeCell` opts out of auto-traits; only a manual impl with a
human-supplied safety argument can restore it. 2 — visibility/happens-before
of prior writes; Relaxed gives atomicity only. 3 — `Arc` is thread-safe
(atomic refcounts, requires `T: Send + Sync` to share); `Rc` is
single-threaded only and won't compile across threads.)*

Next: [Lesson 8 — Modules, errors, and the whole program](08-modules-errors-host.md)
