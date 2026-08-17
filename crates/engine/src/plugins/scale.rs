//! The scale plugin: a pitch generator. `in("trigger")` → `out("note")` — each
//! trigger advances through the configured degrees from the root. A pure
//! function of the trigger stream, so patching and replay stay deterministic.

use super::{Disposer, DisposerCtx, Plugin, PluginApi};
use crate::graph::{Direction, NodeId, NodeKind, Port, ScaleGen, SignalKind};

pub const SCALE_PORTS: &[Port] = &[
    Port { name: "trigger", direction: Direction::In, kind: SignalKind::Trigger },
    Port { name: "note", direction: Direction::Out, kind: SignalKind::Note },
];

/// Configurable scale plugin: root (semitones from A4) + degree offsets.
pub struct Scale {
    pub root: f32,
    pub degrees: Vec<i32>,
    pub note_len: u32,
}

impl Plugin for Scale {
    fn id(&self) -> &'static str {
        "scale"
    }

    fn inject(&self) -> &'static [&'static str] {
        &[]
    }

    fn ports(&self) -> &'static [Port] {
        SCALE_PORTS
    }

    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
        let node = api.graph.add_node(
            NodeKind::Opaque(Box::new(ScaleGen::new(
                self.root,
                self.degrees.clone(),
                self.note_len,
            ))),
            SCALE_PORTS.to_vec(),
        );
        Ok((
            node,
            Box::new(move |dis: &mut DisposerCtx| {
                dis.graph.remove_node(node);
            }),
        ))
    }
}

pub fn scale_factory(params: &[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String> {
    let get = |key: &str, default: f32| {
        params
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
            .unwrap_or(default)
    };
    let count = get("count", 5.0) as usize;
    let mut degrees = Vec::with_capacity(count);
    for i in 0..count {
        let value = match i {
            0 => get("degree0", 0.0),
            1 => get("degree1", 3.0),
            2 => get("degree2", 5.0),
            3 => get("degree3", 7.0),
            4 => get("degree4", 10.0),
            5 => get("degree5", 12.0),
            6 => get("degree6", 15.0),
            7 => get("degree7", 17.0),
            8 => get("degree8", 19.0),
            9 => get("degree9", 22.0),
            _ => get("degree9", 22.0),
        };
        degrees.push(value as i32);
    }
    Ok(Box::new(Scale {
        root: get("root", 0.0),
        degrees,
        note_len: get("note_len", 400.0) as u32,
    }))
}
