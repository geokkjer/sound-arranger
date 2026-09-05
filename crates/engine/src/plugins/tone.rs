//! The tone plugin: the sound generator. `in("note")` → `out("audio")` — each
//! note starts a fixed-decay blip at the note's pitch, sample-accurately. This
//! is where patched notes become audible. The **mixer owns the master bus in
//! Phase 1** (tone.audio is patched into a mixer channel); the tone no longer
//! claims the output (Spike-A.5's mount-order bus limitation is gone).

use super::{Disposer, DisposerCtx, ParamDef, Plugin, PluginApi};
use crate::graph::{Direction, NodeId, NodeKind, Port, SignalKind, ToneGen};

pub const TONE_PORTS: &[Port] = &[
    Port { name: "note", direction: Direction::In, kind: SignalKind::Note , channels: 1 },
    Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio , channels: 1 },
];

/// The tone's runtime parameter surface (the logged `SetParam` namespace).
pub const TONE_PARAMS: &[ParamDef] = &[
    ParamDef { name: "gain", min: 0.0, max: 1.0 },
    ParamDef { name: "blip_len", min: 1.0, max: 1_000_000.0 },
];

/// Configurable tone plugin.
pub struct Tone {
    pub gain: f32,
    pub blip_len: u32,
}

impl Plugin for Tone {
    fn id(&self) -> &'static str {
        "tone"
    }

    fn inject(&self) -> &'static [&'static str] {
        &[]
    }

    fn ports(&self) -> &'static [Port] {
        TONE_PORTS
    }

    fn params(&self) -> &'static [ParamDef] {
        TONE_PARAMS
    }

    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
        let node = api.graph.add_node(
            NodeKind::Opaque(Box::new(ToneGen::new(self.gain, self.blip_len))),
            TONE_PORTS.to_vec(),
        );
        Ok((
            node,
            Box::new(move |dis: &mut DisposerCtx| {
                dis.graph.remove_node(node);
            }),
        ))
    }
}

pub fn tone_factory(params: &[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String> {
    let get = |key: &str, default: f32| {
        params
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
            .unwrap_or(default)
    };
    Ok(Box::new(Tone {
        gain: get("gain", 0.25),
        blip_len: get("blip_len", 1200.0) as u32,
    }))
}
