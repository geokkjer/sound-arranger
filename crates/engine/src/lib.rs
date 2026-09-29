//! The minimal core (Phase 0, Spikes A + A.5).
//!
//! Four pieces, per the minimal-core note:
//!
//! 1. [`clock`] — the sample-accurate master clock: absolute frames, an editable
//!    tempo/meter map, and a sample-accurate scheduling queue.
//! 2. [`graph`] — the audio graph interpreter: a **patch bay** of nodes with
//!    named, typed ports (audio / control / trigger / note) and patch cords;
//!    per-node latency (PDC). Spike A.5 (patch-bay note 2026-08-17).
//! 3. [`log`] — the append-only session event log: *model-visible means logged*;
//!    a composition is a log, and the same log renders the same audio.
//! 4. [`ctx`] — context plumbing: services, `inject` dependencies, and reversible
//!    registration (mount returns a disposer).
//!
//! [`render::Engine`] assembles the pieces and drives the render loop. Its
//! public mutation API (`mount`, `patch`, `schedule_unmount`, `set_tempo`,
//! `replay_from`, `providers_of`) is the seed of the core plugin contract (the
//! future IPC command list).
//!
//! Invariants (Spike A + A.5):
//! - every mutation is logged at call time with its absolute frame, then applied
//!   by the render loop when the clock reaches that frame; a refused mutation is
//!   never logged;
//! - rendering is a pure function of the log — no wall clock, no randomness,
//!   `same log ⇒ byte-identical bounce` (including mid-session changes);
//! - steady-state render never allocates (fixed-size arrays, preallocated
//!   scratch and PDC delay lines; nodes must not allocate or block) — enforced
//!   by counting-allocator tests. Control-side mutations (mounts/patches) are
//!   applied on the render call stack in Spike A.5; a real control→render
//!   handoff is Phase 1.
//!
//! External I/O seams (MIDI / OSC for SuperCollider and Tidal) are declared in
//! [`plugins`] — traits now, implementations when demanded (patch-bay note,
//! external-I/O seams).

pub mod clock;
pub mod ctx;
pub mod graph;
pub mod log;
pub mod plugins;
pub mod render;
pub mod value;

pub use clock::{Clock, Scheduler, TempoMap};
pub use ctx::Context;
pub use graph::{
    AudioInputs, AudioNode, BLOCK, CAP_EVENTS, Direction, EUCLIDEAN_MAX_STEPS, EUCLIDEAN_STEP_CAP,
    EuclideanGen, EventBuf, Gain, Graph, MERGE_CAP, Node, NodeIO, NodeId, NodeKind, NoteEvent,
    Port, RenderBlock, RenderMode, ScaleGen, SignalKind, Sine, ToneGen, Trigger,
};
pub use log::{Event, SessionLog};
pub use plugins::{
    ClockOutNode, ClockOutPlugin, Disposer, DisposerCtx, EUCLIDEAN_DROPS_KEY, Euclidean, EventSink,
    EventSource, ExternalEvent, MIXER_CHANNELS, MIXER_CHANNELS_SANITY, MIXER_PARAMS, MIXER_PORTS,
    MeterBank, MidiSink, MidiSource, MixerNode, MixerPlugin, OscSink, OscSource, ParamDef, Plugin,
    PluginApi, Rhythm, Scale, TONE_PARAMS, Tone, Transport, TransportLog, euclid, mixer_factory,
};
pub use render::{
    DrainOutcome, DrainPolicy, Engine, MAX_DRAIN_FRAMES, MIN_TEMPO_BPM, OpHandler, PluginFactory,
    SchedEvent,
};
pub use value::{OpMsg, Value};
