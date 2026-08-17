//! The plugin discipline (composition-seams note): plugins declare service
//! dependencies (`inject`), register contributions through [`PluginApi`], and
//! return a [`Disposer`] that undoes them — the reversible-effects pattern, in
//! miniature. Generators are pure pattern queries the render loop pulls from.

pub mod euclidean;

use std::ops::Range;

use crate::clock::{Clock, Scheduler, TempoMap};
use crate::ctx::Context;
use crate::graph::{Graph, NodeId};
use crate::render::SchedEvent;

/// What a disposer may touch to undo a plugin's contributions.
pub struct DisposerCtx<'a> {
    pub ctx: &'a mut Context,
    pub scheduler: &'a mut Scheduler<SchedEvent>,
    pub graph: &'a mut Graph,
    pub generators: &'a mut Vec<GeneratorMount>,
}

/// The inverse of a plugin's effects: run on unmount to remove every
/// contribution the plugin registered (reversible registration).
pub type Disposer = Box<dyn FnOnce(&mut DisposerCtx<'_>)>;

/// What a plugin may touch when it applies (mounts). The clock is read-only:
/// no plugin owns time.
pub struct PluginApi<'a> {
    pub ctx: &'a mut Context,
    pub scheduler: &'a mut Scheduler<SchedEvent>,
    pub graph: &'a mut Graph,
    pub clock: &'a Clock,
    pub generators: &'a mut Vec<GeneratorMount>,
}

/// A mountable plugin: declares its service dependencies (`inject`) and, on
/// apply, registers its contributions, returning a disposer that undoes them.
pub trait Plugin {
    fn id(&self) -> &'static str;
    /// Declared service dependencies (the coeffect specification).
    fn inject(&self) -> &'static [&'static str];
    fn apply(&mut self, api: &mut PluginApi) -> Result<Disposer, String>;
}

/// A pure pattern generator mounted into the render loop. `for_each_trigger`
/// emits the absolute frame of every trigger inside `block` — the pull-based,
/// sample-accurate scheduling query. Must be a pure function of the tempo map
/// (determinism).
pub trait Generator: Send {
    fn id(&self) -> &'static str;
    fn for_each_trigger(&self, block: Range<u64>, tempo: &TempoMap, emit: &mut dyn FnMut(u64));
}

/// A mounted generator: which graph node its triggers address.
pub struct GeneratorMount {
    pub id: &'static str,
    pub node: NodeId,
    pub generator: Box<dyn Generator>,
}

pub use euclidean::{euclid, euclidean_factory, Euclidean, Rhythm};
