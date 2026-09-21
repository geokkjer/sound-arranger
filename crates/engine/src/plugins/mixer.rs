//! The soft mixer plugin (Phase 1): the profile's master bus, **adaptable to
//! the inputs** (P1.2, user requirement 2026-08-18): the channel count is a
//! mount parameter (`channels`, 1..=8) the profile sets from the input
//! device's layout — e.g. the Soundcraft Notepad-12FX's 4 USB capture
//! channels, or the Scarlett 2i2's 2.
//!
//! Per-channel gain / mute / solo; a master fader; per-channel + master
//! **meters** (peak per block, written to shared atomics — the render path
//! never blocks); and — by mounting — the mixer **owns the master bus**: its
//! `apply` claims the graph's `out_node`, ending the Spike-A.5 "last audio
//! provider wins" limitation. Mono in Phase 1 (the graph is mono; pan arrives
//! with stereo).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use super::{Disposer, ParamDef, Plugin, PluginApi};
use crate::graph::{AudioNode, Direction, EventBuf, NodeId, NodeIO, NodeKind, NoteEvent, Port, RenderBlock, SignalKind, Trigger, CAP_EVENTS};

/// Maximum mixer channels (bounded by the graph's [`MAX_AUDIO_INS`]); the
/// declared port/parameter surfaces cover this maximum.
pub const MIXER_CHANNELS_MAX: usize = 8;
/// The default channel count (the Notepad-12FX's four USB capture channels).
pub const MIXER_CHANNELS: usize = 4;

/// The mixer's declared port surface: one audio input per channel (up to
/// [`MIXER_CHANNELS_MAX`]) + the master audio output. Patches to channels
/// beyond the mounted `channels` count are accepted but ignored (documented).
pub const MIXER_PORTS: &[Port] = &[
    Port { name: "ch0", direction: Direction::In, kind: SignalKind::Audio , channels: 1 },
    Port { name: "ch1", direction: Direction::In, kind: SignalKind::Audio , channels: 1 },
    Port { name: "ch2", direction: Direction::In, kind: SignalKind::Audio , channels: 1 },
    Port { name: "ch3", direction: Direction::In, kind: SignalKind::Audio , channels: 1 },
    Port { name: "ch4", direction: Direction::In, kind: SignalKind::Audio , channels: 1 },
    Port { name: "ch5", direction: Direction::In, kind: SignalKind::Audio , channels: 1 },
    Port { name: "ch6", direction: Direction::In, kind: SignalKind::Audio , channels: 1 },
    Port { name: "ch7", direction: Direction::In, kind: SignalKind::Audio , channels: 1 },
    Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio , channels: 2 },
];

/// The mixer's declared runtime parameter surface — the logged `SetParam`
/// namespace (`Engine::set_param` refuses anything not listed here).
pub const MIXER_PARAMS: &[ParamDef] = &[
    ParamDef { name: "ch0.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch0.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch0.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch0.pan", min: -1.0, max: 1.0 },
    ParamDef { name: "ch1.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch1.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch1.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch1.pan", min: -1.0, max: 1.0 },
    ParamDef { name: "ch2.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch2.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch2.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch2.pan", min: -1.0, max: 1.0 },
    ParamDef { name: "ch3.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch3.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch3.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch3.pan", min: -1.0, max: 1.0 },
    ParamDef { name: "ch4.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch4.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch4.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch4.pan", min: -1.0, max: 1.0 },
    ParamDef { name: "ch5.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch5.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch5.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch5.pan", min: -1.0, max: 1.0 },
    ParamDef { name: "ch6.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch6.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch6.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch6.pan", min: -1.0, max: 1.0 },
    ParamDef { name: "ch7.gain", min: 0.0, max: 2.0 },
    ParamDef { name: "ch7.mute", min: 0.0, max: 1.0 },
    ParamDef { name: "ch7.solo", min: 0.0, max: 1.0 },
    ParamDef { name: "ch7.pan", min: -1.0, max: 1.0 },
    ParamDef { name: "master.gain", min: 0.0, max: 2.0 },
];

/// Per-block peak meters: one atomic per channel (f32 bits) plus the master.
/// The render path writes, the control side reads — no blocking, no
/// allocation. Only the first `channels` entries are meaningful (the mount
/// parameter). Meter points: channel meters are **post-gain, pre-mute/solo** —
/// a muted channel still shows its level; the master meter is
/// post-master-gain.
pub struct MeterBank(pub [AtomicU32; MIXER_CHANNELS_MAX + 1]);

impl MeterBank {
    /// Peak of channel `k` (0..MIXER_CHANNELS_MAX) for the last rendered block.
    pub fn channel_peak(&self, k: usize) -> f32 {
        f32::from_bits(self.0[k].load(Ordering::Relaxed))
    }

    /// Peak of the master bus for the last rendered block.
    pub fn master_peak(&self) -> f32 {
        f32::from_bits(self.0[MIXER_CHANNELS_MAX].load(Ordering::Relaxed))
    }
}

impl Default for MeterBank {
    fn default() -> Self {
        MeterBank(std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())))
    }
}

/// The mixer node: per-channel gain/mute/solo, master fader, post-fader
/// meters. Reads its channel inputs from `io.audio_ins` (per-port fan-in);
/// processes only the first `channels` inputs.
pub struct MixerNode {
    channels: usize,
    gains: [f32; MIXER_CHANNELS_MAX],
    pans: [f32; MIXER_CHANNELS_MAX],
    mutes: [bool; MIXER_CHANNELS_MAX],
    solos: [bool; MIXER_CHANNELS_MAX],
    master_gain: f32,
    meters: Arc<MeterBank>,
}

impl MixerNode {
    pub fn new() -> Self {
        Self::with_channels(MIXER_CHANNELS, Arc::new(MeterBank::default()))
    }

    /// Construct with an active channel count and a shared meter bank — the
    /// plugin uses this so the profile can read the meters through the
    /// `mixer.meters` context key.
    pub fn with_channels(channels: usize, meters: Arc<MeterBank>) -> Self {
        debug_assert!((1..=MIXER_CHANNELS_MAX).contains(&channels), "mixer channels out of range");
        MixerNode {
            channels: channels.clamp(1, MIXER_CHANNELS_MAX),
            gains: [1.0; MIXER_CHANNELS_MAX],
            pans: [0.0; MIXER_CHANNELS_MAX],
            mutes: [false; MIXER_CHANNELS_MAX],
            solos: [false; MIXER_CHANNELS_MAX],
            master_gain: 1.0,
            meters,
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn meters(&self) -> Arc<MeterBank> {
        self.meters.clone()
    }

    fn set_param(&mut self, name: &str, value: f32) {
        // "ch{i}.gain" | "ch{i}.pan" | "ch{i}.mute" | "ch{i}.solo" | "master.gain"
        if let Some(rest) = name.strip_prefix("ch") {
            let Some((idx, attr)) = rest.split_once('.') else {
                debug_assert!(false, "mixer: malformed param '{name}'");
                return;
            };
            let Ok(i) = idx.parse::<usize>() else {
                debug_assert!(false, "mixer: malformed channel '{idx}'");
                return;
            };
            if i >= MIXER_CHANNELS_MAX {
                debug_assert!(false, "mixer: channel {i} out of range");
                return;
            }
            match attr {
                "gain" => self.gains[i] = value,
                "pan" => self.pans[i] = value.clamp(-1.0, 1.0),
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
        // Stereo master: `out` is interleaved L,R. Each channel input is mono,
        // panned with an equal-power law before summation.
        debug_assert_eq!(io.audio_out_channels, 2, "mixer must be the stereo master");
        let n = io.audio_in_count.min(self.channels);
        let any_solo = self.solos[..n].iter().any(|&s| s);
        let mut peaks = [0.0f32; MIXER_CHANNELS_MAX + 1];
        for (i, sample) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let mut sl = 0.0f32;
            let mut sr = 0.0f32;
            for (ch, channel_in) in io.audio_ins[..n].iter().enumerate() {
                let v = channel_in.get(i).copied().unwrap_or(0.0);
                // Channel meter: post-gain, pre-mute/solo — a muted channel
                // still shows its level. Mute is absolute (wins over solo).
                let g = v * self.gains[ch];
                peaks[ch] = peaks[ch].max(g.abs());
                let audible = !self.mutes[ch] && (!any_solo || self.solos[ch]);
                if audible {
                    // Equal-power pan: `-1` hard left, `0` center (√2/2 each),
                    // `+1` hard right.
                    let a = (self.pans[ch] + 1.0) * std::f32::consts::FRAC_PI_4;
                    let (pl, pr) = (a.cos(), a.sin());
                    sl += g * pl;
                    sr += g * pr;
                }
            }
            let l = sl * self.master_gain;
            let r = sr * self.master_gain;
            sample[0] = l;
            sample[1] = r;
            let full = l.abs().max(r.abs());
            peaks[MIXER_CHANNELS_MAX] = peaks[MIXER_CHANNELS_MAX].max(full);
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
/// `ctx.get::<Arc<MeterBank>>("mixer.meters")`).
pub struct MixerPlugin {
    channels: usize,
}

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
            NodeKind::Opaque(Box::new(MixerNode::with_channels(self.channels, meters.clone()))),
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

/// Factory form — the `channels` mount parameter adapts the mixer to the
/// input device's layout (default [`MIXER_CHANNELS`], clamped to
/// 1..=[`MIXER_CHANNELS_MAX`]).
pub fn mixer_factory(params: &[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String> {
    let channels = match params.iter().find(|(name, _)| *name == "channels") {
        Some((_, v)) => {
            if v.fract() != 0.0 {
                return Err(format!("mixer channels must be a whole number, got {v}"));
            }
            *v as usize
        }
        None => MIXER_CHANNELS,
    };
    if !(1..=MIXER_CHANNELS_MAX).contains(&channels) {
        return Err(format!("mixer channels must be 1..={MIXER_CHANNELS_MAX}, got {channels}"));
    }
    Ok(Box::new(MixerPlugin { channels }))
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

    #[test]
    fn factory_rejects_out_of_range_channels() {
        assert!(mixer_factory(&[("channels", 0.0)]).is_err());
        assert!(mixer_factory(&[("channels", 9.0)]).is_err());
        assert!(mixer_factory(&[("channels", 4.0)]).is_ok());
    }
}
