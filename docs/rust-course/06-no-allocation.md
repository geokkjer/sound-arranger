# Lesson 6 — The no-allocation render path

**Material:** `EventBuf` in [`crates/engine/src/graph.rs`](../../crates/engine/src/graph.rs) (lines 53–123),
and the counting-allocator test in `crates/engine/tests/spike_a.rs`.

One rule dominates this codebase's style: **the steady-state render loop never
allocates and never blocks.** On a real audio device, render runs in a
time-critical callback; one heap allocation that triggers an OS call at the
wrong moment is an audible click. This lesson covers the Rust features that
make the rule *structural* rather than aspirational.

## 6.1 Const generics: capacity as part of the type

```rust
pub struct EventBuf<T: Copy + Default, const CAP: usize> {
    buf: [T; CAP],
    count: usize,
}
```

`const CAP: usize` is a **const generic** — a compile-time value parameter.
`EventBuf<NoteEvent, 32>` and `EventBuf<NoteEvent, 128>` are different types
with different sizes. The buffer is `[T; CAP]` — a plain array, stored inline,
no heap, no pointer. Compare:

| | `Vec<T>` | `[T; N]` |
|---|---|---|
| storage | heap, growable | inline, fixed |
| length | runtime value | part of the type |
| allocation | on push past capacity | none, ever |

`EventBuf` deliberately chooses the second column and reimplements the tiny API
it needs (`push`, `clear`, `as_slice`). The interesting decision is in `push`:

```rust
pub fn push(&mut self, value: T) -> bool {
    if self.count == CAP {
        return false;   // drop, don't grow
    }
    self.buf[self.count] = value;
    self.count += 1;
    true
}
```

A full buffer *drops* the event and says so via the return value. Growing would
allocate on the hot path; dropping an occasional event flood was judged the
lesser evil, and it's documented as policy, not accident.

## 6.2 The bounds: `T: Copy + Default`

The `<T: Copy + Default>` clause restricts what T may be:

- **`Copy`** — events are small plain-data values; copying beats borrowing for
  something that flows through buffers.
- **`Default`** — lets `new()` fill the array without requiring callers to pass
  an initial element:

```rust
buf: std::array::from_fn(|_| T::default()),
```

(`from_fn` builds `[T; CAP]` by calling the closure once per slot — the
standard trick for constructing large inline arrays.)

Note also the conditional bound on one method only:

```rust
pub fn insert_sorted(&mut self, value: T) -> bool
where
    T: PartialOrd,
{ ... }
```

The struct requires `Copy + Default`; *this method additionally* requires
orderability. That's a `where` clause — per-method bounds, so `push` stays
available for types that aren't comparable.

## 6.3 Slices everywhere: `&[T]` and `&mut [T]`

Look again at the `AudioNode::render` signature from Lesson 3:

```rust
fn render(&mut self, io: &NodeIO, out_audio: &mut [f32], /* ... */)
```

`out_audio: &mut [f32]` is a slice: a pointer + length view over someone
else's memory. The node doesn't own its output buffer; it's *handed* a window
into preallocated graph storage and must fill it. This signature shape is the
allocation rule made structural:

- can't return a fresh `Vec` (nothing asks for one),
- can't grow the output (slices have fixed length),
- writing means indexing or iterating — no allocation possible.

You've seen slices used read-only too:

```rust
pub fn segments(&self) -> &[TempoSegment]
self.buf[..self.count].partition_point(...)
```

Slicing syntax `&buf[..n]` borrows the first n elements. Internalize the
pattern: **functions take `&[T]`/`&mut [T]`, not `&Vec<T>`, when they don't
need to resize** — it accepts arrays too, and states intent precisely.

## 6.4 Proving it: the counting allocator

From the tests (see `spike_a.rs`): the test binary installs a global allocator
that counts calls, renders blocks, and asserts the count didn't move:

```rust
struct CountingAllocator;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if MEASURING.with(|m| m.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    // ... dealloc/realloc forward likewise
}

#[global_allocator]
static GLOBAL_ALLOC: CountingAllocator = CountingAllocator;
```

`MEASURING` is a thread-local flag set only around the measured `render_into`
call, so the control-side allocations that prime a session are not counted.

`#[global_allocator]` swaps the process-wide allocator; because Rust routes all
heap traffic through it, "does my code allocate?" becomes a measurable integer,
not an opinion. This is the kind of property test that's hard to write in most
languages and almost free here.

## 6.5 `debug_assert!`: cheap guards, gone in release

Sprinkled through the render path:

```rust
debug_assert_eq!(written, out.len());
```

`debug_assert!` compiles to nothing in release builds (`--release`). Philosophy:
assert loudly while developing, never risk a panic on a real-time thread in a
shipped binary. Contrast with `.expect()` (Lesson 2), which panics in all
builds — reserve that for invariants you'd want to fail hard on always.

## 6.6 Preallocating everything else

Skim `Graph`'s fields and notice every buffer exists before rendering:
`audio_out: Vec<Vec<f32>>` sized per node at add-time, PDC delay lines
preallocated to their max size, control outputs one `f32` each. The `Vec`s are
used but *never resized* during render — `Vec` with a fixed length behaves
exactly like an array. The distinction isn't the type; it's whether anything on
the hot path can call `push`.

## Your turn

⭐ **1.** Find the counting-allocator tests (there are several now — one per
slice that needs the proof):

```sh
rg -l "global_allocator" crates/
```

Read one, then run just that test and watch it pass. Break a rule: add a
temporary `vec![0.0f32; 8]` inside some node's `render` and watch the test
fail. Feel the guard rail click. Revert.

🔧 **2.** Implement `iter()` on `EventBuf` returning
`impl Iterator<Item = &T> + '_` (one line using `as_slice()`).

🔧 **3.** Add `retain(predicate)` to `EventBuf` without allocating (hint:
`copy_within` plus a write cursor — study how `insert_sorted` shifts elements).
Test: retain even offsets keeps order.

🔧 **4.** Write a function `fn sum(buf: &[f32]) -> f32`. Call it with both a
`Vec<f32>` and a `[f32; 512]` (via `&arr[..]`) and observe that one signature
accepts both. That acceptance is why APIs here speak slices.

## Checkpoints

1. Why does `push` return `bool` instead of growing?
2. What's the difference between `[f32; 512]`, `Vec<f32>`, and `&[f32]`?
3. When do you use `debug_assert!` vs `expect`?

*(Answers: 1 — growth allocates on the hot path; overflow drops are counted
and accepted policy. 2 — inline fixed array / heap-owned growable / borrowed
view over either. 3 — dev-only invariant checks vs always-on hard contracts.)*

Next: [Lesson 7 — Threads, atomics, and a lock-free ring](07-concurrency.md)
