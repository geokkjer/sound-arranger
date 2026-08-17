//! The tone plugin: the sound generator. `in("note")` → `out("audio")` — each
//! note starts a fixed-decay blip at the note's pitch, sample-accurately. This
//! is where patched notes become audible; it owns the master audio output in
//! Spike A.5 (the Phase 1 mixer generalizes the bus).

use super::{Disposer, DisposerCtx, Plugin, PluginApi};
use crate::graph::{Direction, NodeId, NodeKind, Port, SignalKind, ToneGen};

pub const TONE_PORTS: &[Port] = &[
    Port { name: "note", direction: Direction::In, kind: SignalKind::Note },
    Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio },
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

    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
        let node = api.graph.add_node(
            NodeKind::Opaque(Box::new(ToneGen::new(self.gain, self.blip_len))),
            TONE_PORTS.to_vec(),
        );
        // The tone node owns the audio output in Spike A.5.
        api.graph.set_out(node);
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
