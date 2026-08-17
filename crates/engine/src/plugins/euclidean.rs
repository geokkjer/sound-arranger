//! The euclidean rhythm plugin: a pure maximally-even pulse generator.
//!
//! `euclid` is a pure function (Bresenham-style even pulse placement). The
//! plugin mounts a [`Generator`] that the render loop pulls from block by block,
//! plus an opaque [`BlipSynth`] voice node that turns each trigger into a
//! sample-accurate blip. Unmounting removes both — no residual sound or state.

use super::{Disposer, DisposerCtx, Generator, GeneratorMount, Plugin, PluginApi};
use crate::clock::TempoMap;
use crate::graph::{BlipSynth, NodeId, NodeKind};
use std::ops::Range;

/// Maximally-even pulse placement: a pulse at `floor(i * steps / pulses)`,
/// rotated by `rotation` steps. A valid Euclidean rhythm generator (the
/// canonical Bjorklund rotation is a rotation of this one).
pub fn euclid(steps: u32, pulses: u32, rotation: u32) -> Vec<bool> {
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

/// Configurable euclidean rhythm plugin.
pub struct Euclidean {
    pub steps: u32,
    pub pulses: u32,
    pub rotation: u32,
    /// Pattern subdivisions per beat (4 = sixteenth notes).
    pub pulses_per_beat: u32,
    pub pitch: f32,
    pub gain: f32,
    /// Blip length in samples.
    pub blip_len: u32,
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

impl Plugin for Euclidean {
    fn id(&self) -> &'static str {
        "euclidean"
    }

    fn inject(&self) -> &'static [&'static str] {
        &["clock"]
    }

    fn apply(&mut self, api: &mut PluginApi) -> Result<Disposer, String> {
        // The 'clock' dependency is a core service: satisfied by the engine
        // (see `Engine::core_services`), read-only via `api.clock`. Not in ctx.
        let pattern = euclid(self.steps, self.pulses, self.rotation);
        let node: NodeId = api
            .graph
            .add_node(NodeKind::Opaque(Box::new(BlipSynth::new(
                self.pitch,
                self.gain,
                self.blip_len,
            ))));
        api.graph.set_out(node);
        api.ctx.provide(
            "rhythm",
            Rhythm {
                steps: self.steps,
                pulses: self.pulses,
                pattern: pattern.clone(),
            },
        );
        api.generators.push(GeneratorMount {
            id: self.id(),
            node,
            generator: Box::new(EuclideanGen {
                steps: self.steps,
                pulses_per_beat: self.pulses_per_beat.max(1),
                pattern,
            }),
        });
        Ok(Box::new(move |dis: &mut DisposerCtx| {
            dis.graph.remove_node(node);
            dis.generators.retain(|mount| mount.id != "euclidean");
            dis.ctx.remove("rhythm");
        }))
    }
}

struct EuclideanGen {
    steps: u32,
    pulses_per_beat: u32,
    pattern: Vec<bool>,
}

impl Generator for EuclideanGen {
    fn id(&self) -> &'static str {
        "euclidean"
    }

    fn for_each_trigger(&self, block: Range<u64>, tempo: &TempoMap, emit: &mut dyn FnMut(u64)) {
        let step_beats = 1.0 / self.pulses_per_beat as f64;
        // Beat range covered by the block (one step of margin at the top).
        let b0 = tempo.beat_at(block.start);
        let b1 = tempo.beat_at(block.end.saturating_sub(1)) + step_beats;
        let s0 = (b0 / step_beats).floor() as i64;
        let s1 = (b1 / step_beats).ceil() as i64;
        for step in s0..s1 {
            if step < 0 {
                continue;
            }
            let step = step as u64;
            if !self.pattern[(step % self.steps as u64) as usize] {
                continue;
            }
            let frame = tempo.frame_at(step as f64 * step_beats);
            if frame >= block.start && frame < block.end {
                emit(frame);
            }
        }
    }
}

/// Factory form: build the plugin from a flat parameter list (the log's
/// `Event::Mount` payload, and the seed of a future declarative config).
pub fn euclidean_factory(params: &[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String> {
    let get = |key: &str, default: f32| {
        params
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
            .unwrap_or(default)
    };
    Ok(Box::new(Euclidean {
        steps: get("steps", 8.0) as u32,
        pulses: get("pulses", 3.0) as u32,
        rotation: get("rotation", 0.0) as u32,
        pulses_per_beat: get("pulses_per_beat", 4.0) as u32,
        pitch: get("pitch", 440.0),
        gain: get("gain", 0.25),
        blip_len: get("blip_len", 1200.0) as u32,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euclid_is_maximally_even() {
        for (steps, pulses) in [(8, 3), (8, 5), (16, 5), (12, 4), (7, 2), (16, 7)] {
            let pattern = euclid(steps, pulses, 0);
            assert_eq!(pattern.len(), steps as usize);
            assert_eq!(pattern.iter().filter(|p| **p).count(), pulses as usize);
            // gaps between pulses (including wraparound) differ by at most 1
            let positions: Vec<usize> = pattern
                .iter()
                .enumerate()
                .filter(|(_, p)| **p)
                .map(|(i, _)| i)
                .collect();
            let mut gaps: Vec<usize> = positions
                .windows(2)
                .map(|w| w[1] - w[0])
                .collect();
            gaps.push(positions[0] + steps as usize - positions[positions.len() - 1]);
            let min = *gaps.iter().min().unwrap();
            let max = *gaps.iter().max().unwrap();
            assert!(max - min <= 1, "E({pulses},{steps}) gaps {gaps:?} not maximally even");
        }
    }

    #[test]
    fn euclid_rotation_shifts() {
        let base = euclid(8, 3, 0);
        let rot = euclid(8, 3, 2);
        // rotate_right(2): element k of the base lands at (k - 2) mod 8.
        let shifted = base
            .iter()
            .cycle()
            .skip(6)
            .take(8)
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(rot, shifted);
    }

    #[test]
    fn euclid_edge_cases() {
        // steps clamped to 1; zero pulses → all rests; pulses ≥ steps → all hits.
        assert_eq!(euclid(0, 3, 0), vec![false]);
        assert_eq!(euclid(8, 0, 0), vec![false; 8]);
        assert_eq!(euclid(8, 99, 0), vec![true; 8]);
        assert_eq!(euclid(4, 4, 0), vec![true; 4]);
    }
}
