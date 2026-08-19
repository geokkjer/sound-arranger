//! The soft mixer plugin (Phase 1): the profile's master bus.
//!
//! Four audio input channels (`ch0..ch3` — the EP-133's four groups), each
//! with gain / mute / solo; a master fader; per-channel + master **meters**
//! (peak per block, written to shared atomics — the render path never blocks);
//! and — by mounting — the mixer **owns the master bus**: its `apply` claims
//! the graph's `out_node`, ending the Spike-A.5 "last audio provider wins"
//! limitation. Mono in Phase 1 (the graph is mono; pan arrives with stereo).
//!
//! The mixer is a plugin like any other; it just happens to claim the bus on
//! mount, and the profile keeps it mounted for the session's lifetime.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use super::{Disposer, ParamDef, Plugin, PluginApi};
use crate::graph::{AudioNode, Direction, EventBuf, NodeId, NodeIO, NodeKind, NoteEvent, Port, RenderBlock, SignalKind, Trigger, CAP_EVENTS};

/// Input channels (matches the EP-133's four groups; the graph supports up to
/// [`crate::graph::MAX_AUDIO_INS`]).
pub const MIXER_CHANNELS: usize = 4;

/// The mixer's declared port surface: one audio input per channel + the
/// master audio output.
pub const MIXER_PORTS: &[Port] = &[
    Port { name: "ch0", direction: Direction::In, kind: SignalKind::Audio },
    Port { name: "ch1", direction: Direction::In, kind: SignalKind::Audio },
    Port { name: "ch2", direction: Direction::In, kind: SignalKind::Audio },
    Port { name: "ch3", direction: Direction::In, kind: SignalKind::Audio },
    Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio },
];

/// The mixer's declared runtime parameter surface — the logged `SetParam`
/// namespace (`Engine::set_param` refuses anything not listed here; kimi
/// review finding 4).
pub const MIXER_PARAMS: &[ParamDef] = &[
    ParamDef { name: "ch0.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch0.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch0.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch1.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch1.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch1.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch2.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch2.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch2.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch3.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch3.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch3.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "master.gain", min: 0.0, max: 2.0 },
];

/// Per-block peak meters: one atomic per channel (f32 bits) plus the master.
/// The render path writes, the control side reads — no blocking, no
/// allocation. Meter points (kimi review finding 10): channel meters are
/// **post-gain, pre-mute/solo** — a muted channel still shows its level; the
/// master meter is post-master-gain.
pub struct MeterBank(pub [AtomicU32; MIXER_CHANNELS + 1]);

impl MeterBank {
    /// Peak of channel `k` (0..MIXER_CHANNELS) for the last rendered block.
    pub fn channel_peak(&self, k: usize) -> f32 {
        f32::from_bits(self.0[k].load(Ordering::Relaxed))
    }

    /// Peak of the master bus for the last rendered block.
    pub fn master_peak(&self) -> f32 {
        f32::from_bits(self.0[MIXER_CHANNELS].load(Ordering::Relaxed))
    }
}

impl Default for MeterBank {
    fn default() -> Self {
        MeterBank(std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())))
    }
}

/// The mixer node: per-channel gain/mute/solo, master fader, post-fader
/// meters. Reads its channel inputs from `io.audio_ins` (per-port fan-in).
pub struct MixerNode {
    gains: [f32; MIXER_CHANNELS],
    mutes: [bool; MIXER_CHANNELS],
    solos: [bool; MIXER_CHANNELS],
    master_gain: f32,
    meters: Arc<MeterBank>,
}

impl MixerNode {
    pub fn new() -> Self {
        Self::with_meters(Arc::new(MeterBank::default()))
    }

    /// Construct with a shared meter bank — the plugin uses this so the
    /// profile can read the meters through the `mixer.meters` context key.
    pub fn with_meters(meters: Arc<MeterBank>) -> Self {
        MixerNode {
            gains: [1.0; MIXER_CHANNELS],
            mutes: [false; MIXER_CHANNELS],
            solos: [false; MIXER_CHANNELS],
            master_gain: 1.0,
            meters,
        }
    }

    pub fn meters(&self) -> Arc<MeterBank> {
        self.meters.clone()
    }

    fn set_param(&mut self, name: &str, value: f32) {
        // "ch{i}.gain" | "ch{i}.mute" | "ch{i}.solo" | "master.gain"
        if let Some(rest) = name.strip_prefix("ch") {
            let Some((idx, attr)) = rest.split_once('.') else {
                debug_assert!(false, "mixer: malformed param '{name}'");
                return;
            };
            let Ok(i) = idx.parse::<usize>() else {
                debug_assert!(false, "mixer: malformed channel '{idx}'");
                return;
            };
            if i >= MIXER_CHANNELS {
                debug_assert!(false, "mixer: channel {i} out of range");
                return;
            }
            match attr {
                "gain" => self.gains[i] = value,
                "mute" => self.mutes[i] = value != 0.0,
                "solo" => self.solos[i] = value != 0.0,
                other => debug_assert!(false, "mixer: unknown channel attr '{other}'"),
            }
        } else if name == "master.gain" {
            self.master_gain = value;
        } else {
            debug_assert!(false, "mixer: unknown param '{name}'");
        }
    }
}

impl Default for MixerNode {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioNode for MixerNode {
    fn latency(&self) -> u32 {
        0
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
        let n = io.audio_in_count.min(MIXER_CHANNELS);
        let any_solo = self.solos[..n].iter().any(|&s| s);
        let mut peaks = [0.0f32; MIXER_CHANNELS + 1];
        for (i, sample) in out.iter_mut().enumerate() {
            let mut sum = 0.0f32;
            for (ch, channel_in) in io.audio_ins[..n].iter().enumerate() {
                let v = channel_in.get(i).copied().unwrap_or(0.0);
                // Channel meter: post-gain, pre-mute/solo — a muted channel
                // still shows its level. Mute is absolute (wins over solo).
                let g = v * self.gains[ch];
                peaks[ch] = peaks[ch].max(g.abs());
                let audible = !self.mutes[ch] && (!any_solo || self.solos[ch]);
                sum += g * if audible { 1.0 } else { 0.0 };
            }
            *sample = sum * self.master_gain;
            peaks[MIXER_CHANNELS] = peaks[MIXER_CHANNELS].max(sample.abs());
        }
        for (k, p) in peaks.iter().enumerate() {
            self.meters.0[k].store(p.to_bits(), Ordering::Relaxed);
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        self.set_param(name, value);
    }
}

/// The mixer plugin: mounting it claims the master bus and provides the
/// meters under the `mixer.meters` context key (the profile reads them via
/// `ctx.get::<Arc<MeterBank>>("mixer.meters")` — kimi review finding 1).
pub struct MixerPlugin;

impl Plugin for MixerPlugin {
    fn id(&self) -> &'static str {
        "mixer"
    }

    fn inject(&self) -> &'static [&'static str] {
        &[]
    }

    fn ports(&self) -> &'static [Port] {
        MIXER_PORTS
    }

    fn params(&self) -> &'static [ParamDef] {
        MIXER_PARAMS
    }

    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
        let meters = Arc::new(MeterBank::default());
        let node = api.graph.add_node(
            NodeKind::Opaque(Box::new(MixerNode::with_meters(meters.clone()))),
            MIXER_PORTS.to_vec(),
        );
        // The mixer owns the master bus: sources route into its channels, and
        // the master out is its output. Its meters are a context service.
        api.graph.set_out(node);
        api.ctx.provide("mixer.meters", meters);
        Ok((
            node,
            Box::new(move |dis: &mut super::DisposerCtx| {
                dis.graph.remove_node(node);
                dis.ctx.remove("mixer.meters");
            }),
        ))
    }
}

/// Factory form (the log's `Event::Mount` payload; the mixer takes no params —
/// channels are fixed in Phase 1).
pub fn mixer_factory(_params: &[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String> {
    Ok(Box::new(MixerPlugin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_namespace() {
        let mut m = MixerNode::new();
        m.set_param("ch1.gain", 0.5);
        m.set_param("ch0.mute", 1.0);
        m.set_param("ch2.solo", 1.0);
        m.set_param("master.gain", 0.25);
        assert_eq!(m.gains[1], 0.5);
        assert!(m.mutes[0]);
        assert!(m.solos[2]);
        assert_eq!(m.master_gain, 0.25);
    }
}
