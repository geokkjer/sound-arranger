//! The minimal core (Phase 0, Spike A).
//!
//! Four pieces, per the minimal-core note (`.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md`):
//!
//! 1. [`clock`] — the sample-accurate master clock: absolute frames, an editable
//!    tempo/meter map, and a sample-accurate scheduling queue.
//! 2. [`graph`] — the audio graph interpreter: a *value* (nodes / edges / params)
//!    with two node tiers (declarative + opaque) and per-node latency (PDC).
//! 3. [`log`] — the append-only session event log: *model-visible means logged*;
//!    a composition is a log, and the same log renders the same audio.
//! 4. [`ctx`] — context plumbing: services, `inject` dependencies, and reversible
//!    registration (mount returns a disposer).
//!
//! [`render::Engine`] assembles the four pieces and drives the render loop. Its
//! public mutation API (`mount`, `schedule_unmount`, `set_tempo`, `replay_from`)
//! is the seed of the core plugin contract (the future IPC command list).
//!
//! Invariants (Spike A):
//! - every mutation is logged at call time with its absolute frame, then applied
//!   by the render loop when the clock reaches that frame; a refused mutation is
//!   never logged;
//! - rendering is a pure function of the log — no wall clock, no randomness,
//!   `same log ⇒ byte-identical bounce` (including mid-session changes);
//! - the render path never allocates (fixed-size arrays and preallocated
//!   scratch; nodes must not allocate or block) — enforced by a
//!   counting-allocator test.

pub mod clock;
pub mod ctx;
pub mod graph;
pub mod log;
pub mod plugins;
pub mod render;

pub use clock::{Clock, Scheduler, TempoMap};
pub use ctx::Context;
pub use graph::{AudioNode, BlipSynth, Gain, Graph, NodeId, NodeKind, RenderBlock, Sine, BLOCK};
pub use log::{Event, SessionLog};
pub use plugins::{
    euclid, Disposer, DisposerCtx, Euclidean, Generator, GeneratorMount, Plugin, PluginApi, Rhythm,
};
pub use render::{Engine, PluginFactory, SchedEvent};
