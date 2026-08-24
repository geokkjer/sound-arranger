//! The plugin discipline (composition-seams + patch-bay notes): plugins declare
//! service dependencies (`inject`) and a port surface (`ports`), register their
//! contributions through [`PluginApi`], and return a disposer that undoes them
//! — reversible effects, in miniature. The patch bay turns trigger/note/audio
//! streams into first-class, typed, patchable outputs.

pub mod euclidean;
pub mod mixer;
pub mod scale;
pub mod tone;

use std::ops::Range;

use crate::clock::{Clock, Scheduler};
use crate::ctx::Context;
use crate::graph::{NodeId, Port};
use crate::render::SchedEvent;

pub use euclidean::{euclid, euclidean_factory, Euclidean, Rhythm};
pub use mixer::{mixer_factory, MixerNode, MixerPlugin, MIXER_CHANNELS, MIXER_CHANNELS_MAX, MIXER_PARAMS, MIXER_PORTS, MeterBank};
pub use scale::{scale_factory, Scale};
pub use tone::{tone_factory, Tone, TONE_PARAMS};

/// What a disposer may touch to undo a plugin's contributions.
pub struct DisposerCtx<'a> {
    pub ctx: &'a mut Context,
    pub scheduler: &'a mut Scheduler<SchedEvent>,
    pub graph: &'a mut crate::graph::Graph,
}

/// The inverse of a plugin's effects: run on unmount to remove every
/// contribution the plugin registered (reversible registration).
pub type Disposer = Box<dyn FnOnce(&mut DisposerCtx<'_>)>;

/// What a plugin may touch when it applies (mounts). The clock is read-only:
/// no plugin owns time.
pub struct PluginApi<'a> {
    pub ctx: &'a mut Context,
    pub scheduler: &'a mut Scheduler<SchedEvent>,
    pub graph: &'a mut crate::graph::Graph,
    pub clock: &'a Clock,
}

/// A mountable plugin: declares its service dependencies (`inject`) and its
/// port surface (`ports`), then on apply registers its node, returning its id
/// plus a disposer that undoes everything.
pub trait Plugin {
    fn id(&self) -> &'static str;
    /// Declared service dependencies (the coeffect specification).
    fn inject(&self) -> &'static [&'static str];
    /// Declared patch-bay surface (the dropdown's data source).
    fn ports(&self) -> &'static [Port];
    /// Declared runtime parameter surface (the logged `SetParam` namespace).
    /// `Engine::set_param` refuses names not declared here — fail-loud, never
    /// logged (Phase 1: the mixer's gain/mute/solo/fader).
    fn params(&self) -> &'static [ParamDef] {
        &[]
    }
    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String>;
}

/// A declared runtime parameter: name plus a sane range (validation + doc).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamDef {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
}

/// An external event entering the patch bay (MIDI, OSC, another host).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExternalEvent {
    NoteOn { offset: u32, pitch: f32, velocity: f32 },
    NoteOff { offset: u32, pitch: f32 },
    Trigger { offset: u32 },
    Control { value: f32 },
}

/// A source of external events for a block (seam declaration — patch-bay note
/// §4; implementations arrive with Phase 2 integrations).
pub trait EventSource: Send {
    fn id(&self) -> &'static str;
    fn for_each_event(&self, block: Range<u64>, emit: &mut dyn FnMut(ExternalEvent));
}

/// A sink our events are sent to (e.g. scsynth via OSC `/s_new`).
pub trait EventSink: Send {
    fn id(&self) -> &'static str;
    fn send(&mut self, events: &[ExternalEvent], frame: u64);
}

/// MIDI input seam (notes/CC in; MIDI-learn later — RESEARCH §8).
pub trait MidiSource: EventSource {}
/// MIDI output seam.
pub trait MidiSink: EventSink {}
/// OSC input seam (Tidal → our SuperDirt-compatible endpoint).
pub trait OscSource: EventSource {}
/// OSC output seam (our notes → scsynth).
pub trait OscSink: EventSink {}
