//! The euclidean rhythm plugin: a pure maximally-even pulse generator.
//!
//! In the patch-bay model this plugin mounts a *generator node* with an
//! `out("triggers", Trigger)` port — it produces no sound itself. Patch the
//! triggers into a scale and a tone generator (or MIDI out, or anything else).
//! It also provides the `rhythm` service for consumers that want the pattern.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use super::{Disposer, DisposerCtx, Plugin, PluginApi};
use crate::graph::{
    Direction, EUCLIDEAN_MAX_STEPS, EuclideanGen, NodeId, NodeKind, Port, SignalKind,
};

/// The plugin's declared port surface (the dropdown's data source).
pub const EUCLIDEAN_PORTS: &[Port] = &[Port {
    name: "triggers",
    direction: Direction::Out,
    kind: SignalKind::Trigger,
    channels: 1,
}];

/// The context key the plugin **publishes** its drop counter under — the same
/// context-service pattern the clock-out plugin uses for `clock_out.overflows`,
/// so a host (and through it a shell's snapshot) can read the live count of
/// grid steps a block could not evaluate. Provided on apply, withdrawn by the
/// disposer. See [`EuclideanGen::drops`] for what the count means.
pub const EUCLIDEAN_DROPS_KEY: &str = "euclidean.drops";

/// The refusal a `steps` over [`EUCLIDEAN_MAX_STEPS`] gets, from the factory
/// (a script line) and from [`Euclidean::apply`] (a hand-assembled instance).
/// One wording, so the door and the backstop say the same thing.
pub fn steps_refusal(steps: u32) -> String {
    format!(
        "euclidean 'steps' is {steps}, over the {EUCLIDEAN_MAX_STEPS}-step limit — the pattern is \
         one bool per step and the node holds three copies of it"
    )
}

/// Maximally-even pulse placement: a pulse at `floor(i * steps / pulses)`,
/// rotated by `rotation` steps. A valid Euclidean rhythm generator (the
/// canonical Bjorklund rotation is a rotation of this one).
///
/// **Panics** if `steps` is over [`EUCLIDEAN_MAX_STEPS`]: the pattern is one
/// `bool` per step and this is the allocation site, so the bound belongs here as
/// well as at the plugin boundary. A `steps` that large is not a rhythm, it is
/// a multi-megabyte request from a parameter.
pub fn euclid(steps: u32, pulses: u32, rotation: u32) -> Vec<bool> {
    assert!(steps <= EUCLIDEAN_MAX_STEPS, "{}", steps_refusal(steps),);
    let n = steps.max(1) as usize;
    let k = pulses.min(steps) as usize;
    let mut pattern = vec![false; n];
    if k == 0 {
        return pattern;
    }
    if k >= n {
        pattern.fill(true);
        return pattern;
    }
    for i in 0..k {
        pattern[(i * n) / k] = true;
    }
    // rotate_right(r): rotation shifts pulses *later* by r steps.
    let rot = (rotation as usize) % n;
    pattern.rotate_right(rot);
    pattern
}

/// The service the euclidean plugin *provides*: the resolved rhythm pattern.
/// A consumer plugin declares `inject: ["rhythm"]` and reads it (spatial
/// composability: providers and consumers coordinate on nothing but the key).
#[derive(Debug, Clone, PartialEq)]
pub struct Rhythm {
    pub steps: u32,
    pub pulses: u32,
    pub pattern: Vec<bool>,
}

/// Configurable euclidean rhythm plugin.
pub struct Euclidean {
    pub steps: u32,
    pub pulses: u32,
    pub rotation: u32,
    /// Pattern subdivisions per beat (4 = sixteenth notes).
    pub pulses_per_beat: u32,
}

impl Plugin for Euclidean {
    fn id(&self) -> &'static str {
        "euclidean"
    }

    fn inject(&self) -> &'static [&'static str] {
        &["clock"]
    }

    fn ports(&self) -> &'static [Port] {
        EUCLIDEAN_PORTS
    }

    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
        // `apply` runs on the render thread, so the one thing this node must
        // never do there is panic — and `Euclidean`'s fields are public, so an
        // instance can be hand-assembled without the factory's door. Refuse the
        // oversize pattern here, before the allocation, rather than reaching
        // `euclid`'s or the constructor's assert from a render call.
        if self.steps > EUCLIDEAN_MAX_STEPS {
            return Err(steps_refusal(self.steps));
        }
        // The 'clock' dependency is a core service: satisfied by the engine
        // (see `Engine::core_services`), read via the block's tempo map.
        let pattern = euclid(self.steps, self.pulses, self.rotation);
        // The drop counter is published, not kept private: the host snapshot
        // reads it back under the key (the clock-out plugin's `overflows`
        // pattern), so a truncated step walk is visible rather than silent.
        let drops = Arc::new(AtomicU64::new(0));
        let node = api.graph.add_node(
            NodeKind::Opaque(Box::new(EuclideanGen::with_drop_counter(
                self.steps,
                self.pulses_per_beat.max(1),
                pattern.clone(),
                drops.clone(),
            ))),
            EUCLIDEAN_PORTS.to_vec(),
        );
        api.ctx.provide(
            "rhythm",
            Rhythm {
                steps: self.steps,
                pulses: self.pulses,
                pattern,
            },
        );
        api.ctx.provide(EUCLIDEAN_DROPS_KEY, drops);
        Ok((
            node,
            Box::new(move |dis: &mut DisposerCtx| {
                dis.graph.remove_node(node);
                dis.ctx.remove("rhythm");
                dis.ctx.remove(EUCLIDEAN_DROPS_KEY);
            }),
        ))
    }
}

/// Factory form: build the plugin from a flat parameter list (the log's
/// `Event::Mount` payload, and the seed of a future declarative config).
///
/// **The door.** `steps` is bounded here, where the mount param is read, so an
/// oversize pattern is refused synchronously and loudly by `Engine::mount` (it
/// dry-runs the factory) instead of allocating on the render thread: mount
/// params are otherwise checked for finiteness and nothing else, and
/// `as u32` **saturates**, so `steps=4294967295` used to arrive intact. The
/// same bound is asserted by the node's constructor and by `euclid`; this is
/// the only one of the three a user can reach.
pub fn euclidean_factory(params: &[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String> {
    let get = |key: &str, default: f32| {
        params
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
            .unwrap_or(default)
    };
    let steps = get("steps", 8.0) as u32;
    if steps > EUCLIDEAN_MAX_STEPS {
        return Err(steps_refusal(steps));
    }
    Ok(Box::new(Euclidean {
        steps,
        pulses: get("pulses", 3.0) as u32,
        rotation: get("rotation", 0.0) as u32,
        pulses_per_beat: get("pulses_per_beat", 4.0) as u32,
    }))
}

#[cfg(test)]
#[path = "tests/euclidean.rs"]
mod tests;
