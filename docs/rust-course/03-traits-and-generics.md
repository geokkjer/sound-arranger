# Lesson 3 — Traits and generics

**Material:** [`crates/engine/src/graph.rs`](../../crates/engine/src/graph.rs)
(the `AudioNode` trait), [`crates/engine/src/plugins/mod.rs`](../../crates/engine/src/plugins/mod.rs)
(the `Plugin` trait).

Rust has classes' *behavior* (interfaces) without classes' inheritance. The
two tools are **traits** (shared behavior) and **generics** (code over many
types). This codebase uses both heavily and the distinction between them
matters.

## 3.1 A trait is a contract

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

Reading it:

- `trait AudioNode` declares required methods; any type implementing it must
  provide `latency` and `render`.
- **`: Send` is a supertrait bound**: implementors must also be `Send` (safe to
  move to another thread) — needed because rendering will run on the audio
  thread.
- **`set_param` has a default body** (`{}`). Implementors may override it or
  skip it. Default methods are how Rust does "optional interface methods"
  without a second trait.
- Parameter names like `_name` — the leading underscore silences "unused"
  warnings for a parameter a default body ignores. You'll see `_` prefixes all
  over idiomatic Rust; they mean "intentionally unused".

An implementation looks like:

```rust
impl AudioNode for Sine {
    fn latency(&self) -> u32 { 0 }
    fn render(&mut self, /* ... */) { /* fill out_audio */ }
}
```

## 3.2 Trait objects: `dyn` — polymorphism at runtime

The graph stores every node in one list regardless of concrete type:

```rust
pub enum NodeKind {
    Declarative(/* ... */),
    Opaque(Box<dyn AudioNode>),
}
```

`Box<dyn AudioNode>` ("a boxed *trait object*") holds *some* heap-allocated
value whose type implements `AudioNode`. The concrete type is erased; calls go
through a vtable, exactly like an interface reference in Java/C#. That's how
`Sine`, `Gain`, `ToneGen`, the mixer, and media's playback nodes all sit in one
`Vec<Node>` and get rendered uniformly by one loop.

Cost/benefit: dynamic dispatch costs an indirect call per `render` — negligible
at block rate — in exchange for not needing to know node types at compile time.
That trade (vtable vs generic) is *the* design decision of plugin systems, and
this codebase picks vtables at the node boundary deliberately.

## 3.3 Generics: monomorphized at compile time

Contrast with `Scheduler<T>` from Lesson 1:

```rust
impl<T> Scheduler<T> {
    pub fn schedule(&mut self, frame: u64, payload: T) { /* ... */ }
}
```

Generic code is compiled *per concrete type used* (monomorphization): zero
runtime overhead, but you can't mix types in one collection. Rule of thumb:

| Need | Use |
|---|---|
| Many types, one collection / erased API | trait object `dyn Trait` |
| Same code shape over different types, performance-critical | generics |
| Both | `<T: Trait>` generic parameter |

## 3.4 The `Plugin` trait: declaring capabilities as data

```rust
pub trait Plugin {
    fn id(&self) -> &'static str;
    fn inject(&self) -> &'static [&'static str];   // required services
    fn ports(&self) -> &'static [Port];             // patch-bay surface
    fn params(&self) -> &'static [ParamDef] { &[] } // runtime knobs
    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String>;
}
```

This trait is the whole plugin architecture in six lines, and it demonstrates a
pattern worth stealing: **methods return static slices of declarative data**
(`&'static [Port]`, `&'static [ParamDef]`). A plugin doesn't just *behave*, it
*describes itself* — its ports feed UI dropdowns, its param definitions feed
validation (Lesson 2's range checks). Behavior and metadata come through the
same door.

Note again the default method: `params()` defaults to an empty slice, so
plugins without knobs (euclidean, scale) never mention params, while the mixer
declares its full gain/mute/solo surface.

## 3.5 Where to see it all wired: the factories

In `render.rs`, plugins register themselves under names:

```rust
engine.register("tone", Box::new(tone_factory));
```

A factory is itself a trait object (`PluginFactory`) producing `Box<dyn Plugin>`
— layers of erasure, each narrow and purposeful. When you later read
`host/src/lib.rs` and see `"mixer"` sent as text from a script, mapped to this
registry, and ending in a virtual `apply()` call, you've seen the entire
"everything is a plugin" claim implemented.

## Your turn

⭐ **1.** Find every `impl AudioNode for ...` in the repo:

```sh
rg "impl AudioNode for" crates/
```

For each, note which ones override `set_param` and which rely on the default.

🔧 **2.** Add your own trivial node:

```rust
struct Half; // outputs input/2

impl AudioNode for Half {
    fn latency(&self) -> u32 { 0 }
    fn render(&mut self, io: &NodeIO, out_audio: &mut [f32], ...) {
        // copy io's first audio input, halved
    }
}
```

Read how `Gain` does it first; mimic its structure. Mount it in a test via the
same path spike_a.rs uses.

🔧 **3.** Give `AudioNode::latency` a default body returning `0`. Which impls
could now be simplified? Would that be a good change? (Think: explicitness at
the point of implementation vs less boilerplate. Both defensible — decide.)

🔧 **4.** Write `fn loudest(nodes: &[&dyn AudioNode]) -> Option<u32>` returning
the max latency. Notice the `dyn` in the slice type — you cannot write
`&[impl AudioNode]`; heterogeneous lists require trait objects.

## Checkpoints

1. Difference between `Box<dyn AudioNode>` and a generic parameter?
2. What does `fn set_param(...) {}` inside a trait declaration mean?
3. Why `Send` on `AudioNode`?

*(Answers: 1 — dyn erases type at runtime via vtable, allows mixing;
generics compile per-type, no mixing, no dispatch cost. 2 — a default
implementation implementors inherit unless they override. 3 — node render will
run on the audio thread, so values must be transferable across threads.)*

Next: [Lesson 4 — Closures and the disposer pattern](04-closures-and-disposers.md)
