//! The fundsp-backed mono voice — the proof that our own soft-synth voices can
//! be built on fundsp's per-sample blocks behind our opaque [`AudioNode`].
//!
//! This is the **native-soft-synth spike**: fundsp supplies the oscillator and
//! filter DSP; we supply the voice layer fundsp does not — note scheduling at a
//! sample offset, a linear-decay envelope, and per-voice gain. The split is
//! deliberate (the native-soft-synth note): the patch stays our loggable value,
//! the fundsp graph is trusted in-process code behind the opaque tier.
//!
//! The voice is **monophonic** (a new note retriggers the single voice). This
//! proves the integration and the invariants (zero-alloc render, byte-identical
//! replay, logged `set_param`, correct `latency()` for PDC); a polyphonic voice
//! pool is the layer above, not this spike.

use super::{Disposer, DisposerCtx, ParamDef, Plugin, PluginApi};
use crate::graph::{
    AudioNode, CAP_EVENTS, Direction, EventBuf, NodeIO, NodeId, NodeKind, NoteEvent, Port,
    RenderBlock, SignalKind, Trigger,
};
use fundsp::prelude32::*;

pub const FUNDSP_PORTS: &[Port] = &[
    Port {
        name: "note",
        direction: Direction::In,
        kind: SignalKind::Note,
        channels: 1,
    },
    Port {
        name: "audio",
        direction: Direction::Out,
        kind: SignalKind::Audio,
        channels: 1,
    },
];

/// Runtime parameter surface (the logged `SetParam` namespace). These are the
/// voice-layer params our wrapper applies with zero allocation; the filter
/// cutoff is a construction (mount) param baked into the fundsp graph.
pub const FUNDSP_PARAMS: &[ParamDef] = &[
    ParamDef {
        name: "gain",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "env_len",
        min: 1.0,
        max: 1_000_000.0,
    },
];

/// The fundsp voice node. `Box<dyn AudioUnit>` is fundsp's dynamic (object-safe)
/// form, built and `allocate()`d at mount so the render path never allocates.
pub struct FundspSynth {
    unit: Box<dyn AudioUnit>,
    /// Current voice frequency in Hz; meaningful only while the envelope is live.
    freq: f32,
    /// Envelope length in samples (linear decay to silence).
    env_len: u32,
    /// Envelope samples remaining; 0 = idle (no ticking).
    remaining: u32,
    /// Per-voice output gain.
    gain: f32,
}

impl FundspSynth {
    pub fn new(cutoff: f32, q: f32, env_len: u32, gain: f32, sample_rate: u32) -> Self {
        // A 1-input (pitch in Hz) / 1-output (filtered sine) graph.
        let graph = sine() >> lowpass_hz(cutoff.max(20.0), q.clamp(0.1, 20.0));
        let mut unit: Box<dyn AudioUnit> = Box::new(graph);
        unit.allocate();
        unit.set_sample_rate(sample_rate as f64);
        FundspSynth {
            unit,
            freq: 0.0,
            env_len: std::cmp::max(env_len, 1),
            remaining: 0,
            gain,
        }
    }
}

impl AudioNode for FundspSynth {
    /// The fundsp path is `lowpass_hz`, a state-variable filter (`FixedSvf`) —
    /// minimum-phase, so it adds no whole-sample latency: PDC compensation is
    /// 0. A lookahead voice (linear-phase EQ, resampling) would report its real
    /// sample delay here — that is the contract.
    fn latency(&self) -> u32 {
        0
    }

    /// A live envelope is output the piece owns — the drain phase renders it out
    /// rather than cutting the decaying voice at the frame budget.
    fn has_tail(&self) -> bool {
        self.remaining > 0
    }

    fn render(
        &mut self,
        io: &NodeIO,
        out: &mut [f32],
        _control: &mut f32,
        _triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        _block: RenderBlock,
    ) {
        // `io.notes_in` is the merged note stream for this block, sorted by
        // offset ascending. A single pass of the block starts the voice at each
        // note's exact sample (the monophonic voice retriggers).
        let mut ni = 0usize;
        for (i, sample) in out.iter_mut().enumerate() {
            while ni < io.notes_in.len() && io.notes_in[ni].offset as usize <= i {
                if io.notes_in[ni].offset as usize == i {
                    let note = io.notes_in[ni];
                    self.freq = 440.0 * 2f32.powf(note.pitch / 12.0);
                    self.unit.reset(); // clear stale filter state
                    self.remaining = self.env_len;
                }
                ni += 1;
            }
            let mut v = 0.0f32;
            if self.remaining > 0 {
                let mut t = [0.0f32; 1];
                self.unit.tick(&[self.freq], &mut t);
                let env = self.remaining as f32 / self.env_len as f32;
                v = t[0] * env * self.gain;
                self.remaining -= 1;
            }
            *sample = v;
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        match name {
            "gain" => self.gain = value.clamp(0.0, 1.0),
            "env_len" => self.env_len = std::cmp::max(value.max(1.0) as u32, 1),
            _ => {}
        }
    }
}

/// The plugin config; `apply` materializes the opaque [`FundspSynth`] node.
pub struct FundspSynthPlugin {
    cutoff: f32,
    q: f32,
    env_len: u32,
    gain: f32,
}

impl Plugin for FundspSynthPlugin {
    fn id(&self) -> &'static str {
        "fundsp_synth"
    }

    fn inject(&self) -> &'static [&'static str] {
        &[]
    }

    fn ports(&self) -> &'static [Port] {
        FUNDSP_PORTS
    }

    fn params(&self) -> &'static [ParamDef] {
        FUNDSP_PARAMS
    }

    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
        let node = api.graph.add_node(
            NodeKind::Opaque(Box::new(FundspSynth::new(
                self.cutoff,
                self.q,
                self.env_len,
                self.gain,
                api.clock.sample_rate,
            ))),
            FUNDSP_PORTS.to_vec(),
        );
        Ok((
            node,
            Box::new(move |dis: &mut DisposerCtx| {
                dis.graph.remove_node(node);
            }),
        ))
    }
}

pub fn fundsp_synth_factory(params: &[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String> {
    let get = |key: &str, default: f32| {
        params
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
            .unwrap_or(default)
    };
    Ok(Box::new(FundspSynthPlugin {
        cutoff: get("cutoff", 4_000.0),
        q: get("q", 0.7),
        env_len: get("env_len", 20_000.0) as u32,
        gain: get("gain", 0.25),
    }))
}
