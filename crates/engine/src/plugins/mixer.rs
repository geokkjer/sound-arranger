//! The soft mixer plugin (Phase 1): the profile's master bus, **adaptable to
//! the inputs** (P1.2, user requirement 2026-08-18): the channel count is a
//! mount parameter (`channels`, 1..=[`MIXER_CHANNELS_SANITY`]) the profile sets from
//! the input device's layout — e.g. the Soundcraft Notepad-12FX's 4 USB capture
//! channels, or the Scarlett 2i2's 2.
//!
//! Per-channel gain / mute / solo; a master fader; per-channel + master
//! **meters** (peak per block, written to shared atomics — the render path
//! never blocks); and — by mounting — the mixer **owns the master bus**: its
//! `apply` claims the graph's `out_node`, ending the Spike-A.5 "last audio
//! provider wins" limitation. Mono in Phase 1 (the graph is mono; pan arrives
//! with stereo).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{Disposer, ParamDef, Plugin, PluginApi};
use crate::graph::{
    AudioNode, CAP_EVENTS, Direction, EventBuf, NodeIO, NodeId, NodeKind, NoteEvent, Port,
    RenderBlock, SignalKind, Trigger,
};

/// A **sanity bound** on a mounted channel count: a typo guard, not a design ceiling.
/// Nothing is sized by it — the node's state, its meter bank and its peak scratch are
/// allocated at mount from the count the profile chose — and it sits far above any real
/// rig. It exists so `channels=1000000` cannot allocate gigabytes.
pub const MIXER_CHANNELS_SANITY: usize = 64;
/// The default channel count (the Notepad-12FX's four USB capture channels).
pub const MIXER_CHANNELS: usize = 4;

/// The parameter kinds every mixer channel carries, in catalog order:
/// `(suffix, min, max)`.
const MIXER_PARAM_KINDS: &[(&str, f32, f32)] = &[
    ("gain", 0.0, 2.0),
    ("mute", 0.0, 1.0),
    ("solo", 0.0, 1.0),
    ("pan", -1.0, 1.0),
];

/// Intern a generated channel name, at most once per name per **process**.
///
/// The mounted surface is built from the mount params, so its names cannot be literals —
/// but a `Port`/`ParamDef` carries `&'static str`, so they have to be. Interning keeps
/// the cost bounded by the widest layout this process ever mounts, rather than by the
/// number of mounts (a replay rebuild re-mounts, and per-mount leaking would grow
/// without bound).
fn intern(name: &str) -> &'static str {
    static NAMES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<&'static str>>> =
        std::sync::OnceLock::new();
    let mut names = NAMES
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
        .lock()
        .expect("the channel-name interner is not poisoned");
    if let Some(existing) = names.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// The ports a mixer mounted with `channels` channels offers: one mono input per
/// channel, then the stereo master output.
fn channel_ports(channels: usize) -> Vec<Port> {
    let mut ports: Vec<Port> = (0..channels)
        .map(|i| Port::audio(intern(&format!("ch{i}")), Direction::In))
        .collect();
    ports.push(Port {
        name: "audio",
        direction: Direction::Out,
        kind: SignalKind::Audio,
        channels: 2,
    });
    ports
}

/// The parameters a mixer mounted with `channels` channels offers: four per channel
/// (gain, mute, solo, pan), then the master fader — the catalog's order.
fn channel_params(channels: usize) -> Vec<ParamDef> {
    let mut params = Vec::with_capacity(channels * MIXER_PARAM_KINDS.len() + 1);
    for i in 0..channels {
        for (kind, min, max) in MIXER_PARAM_KINDS {
            params.push(ParamDef {
                name: intern(&format!("ch{i}.{kind}")),
                min: *min,
                max: *max,
            });
        }
    }
    params.push(ParamDef {
        name: "master.gain",
        min: 0.0,
        max: 2.0,
    });
    params
}

/// The mixer's declared port surface: one audio input per channel (up to
/// [`MIXER_CHANNELS_MAX`]) + the master audio output. Patches to channels
/// beyond the mounted `channels` count are accepted but ignored (documented).
pub const MIXER_PORTS: &[Port] = &[
    Port {
        name: "ch0",
        direction: Direction::In,
        kind: SignalKind::Audio,
        channels: 1,
    },
    Port {
        name: "ch1",
        direction: Direction::In,
        kind: SignalKind::Audio,
        channels: 1,
    },
    Port {
        name: "ch2",
        direction: Direction::In,
        kind: SignalKind::Audio,
        channels: 1,
    },
    Port {
        name: "ch3",
        direction: Direction::In,
        kind: SignalKind::Audio,
        channels: 1,
    },
    Port {
        name: "ch4",
        direction: Direction::In,
        kind: SignalKind::Audio,
        channels: 1,
    },
    Port {
        name: "ch5",
        direction: Direction::In,
        kind: SignalKind::Audio,
        channels: 1,
    },
    Port {
        name: "ch6",
        direction: Direction::In,
        kind: SignalKind::Audio,
        channels: 1,
    },
    Port {
        name: "ch7",
        direction: Direction::In,
        kind: SignalKind::Audio,
        channels: 1,
    },
    Port {
        name: "audio",
        direction: Direction::Out,
        kind: SignalKind::Audio,
        channels: 2,
    },
];

/// The mixer's declared runtime parameter surface — the logged `SetParam`
/// namespace (`Engine::set_param` refuses anything not listed here).
pub const MIXER_PARAMS: &[ParamDef] = &[
    ParamDef {
        name: "ch0.gain",
        min: 0.0,
        max: 2.0,
    },
    ParamDef {
        name: "ch0.mute",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch0.solo",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch0.pan",
        min: -1.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch1.gain",
        min: 0.0,
        max: 2.0,
    },
    ParamDef {
        name: "ch1.mute",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch1.solo",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch1.pan",
        min: -1.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch2.gain",
        min: 0.0,
        max: 2.0,
    },
    ParamDef {
        name: "ch2.mute",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch2.solo",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch2.pan",
        min: -1.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch3.gain",
        min: 0.0,
        max: 2.0,
    },
    ParamDef {
        name: "ch3.mute",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch3.solo",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch3.pan",
        min: -1.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch4.gain",
        min: 0.0,
        max: 2.0,
    },
    ParamDef {
        name: "ch4.mute",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch4.solo",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch4.pan",
        min: -1.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch5.gain",
        min: 0.0,
        max: 2.0,
    },
    ParamDef {
        name: "ch5.mute",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch5.solo",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch5.pan",
        min: -1.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch6.gain",
        min: 0.0,
        max: 2.0,
    },
    ParamDef {
        name: "ch6.mute",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch6.solo",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch6.pan",
        min: -1.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch7.gain",
        min: 0.0,
        max: 2.0,
    },
    ParamDef {
        name: "ch7.mute",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch7.solo",
        min: 0.0,
        max: 1.0,
    },
    ParamDef {
        name: "ch7.pan",
        min: -1.0,
        max: 1.0,
    },
    ParamDef {
        name: "master.gain",
        min: 0.0,
        max: 2.0,
    },
];

/// Per-block peak meters: one atomic per channel (f32 bits) plus the master.
/// The render path writes, the control side reads — no blocking, no
/// allocation. Only the first `channels` entries are meaningful (the mount
/// parameter). Meter points: channel meters are **post-gain, pre-mute/solo** —
/// a muted channel still shows its level; the master meter is
/// post-master-gain.
pub struct MeterBank(pub Vec<AtomicU32>);

impl MeterBank {
    /// A bank with one atomic per channel plus the master — sized from the **mount**, so
    /// the mixer's width is not a compile-time property of the meters either.
    pub fn new(channels: usize) -> Self {
        MeterBank(
            std::iter::repeat_with(|| AtomicU32::new(0.0f32.to_bits()))
                .take(channels + 1)
                .collect(),
        )
    }

    /// Peak of channel `k` for the last rendered block.
    pub fn channel_peak(&self, k: usize) -> f32 {
        f32::from_bits(self.0[k].load(Ordering::Relaxed))
    }

    /// Peak of the master bus for the last rendered block.
    pub fn master_peak(&self) -> f32 {
        let slot = self.0.last().expect("a bank always has a master slot");
        f32::from_bits(slot.load(Ordering::Relaxed))
    }
}

/// The mixer node: per-channel gain/mute/solo, master fader, post-fader
/// meters. Reads its channel inputs from `io.audio_ins` (per-port fan-in);
/// processes only the first `channels` inputs.
pub struct MixerNode {
    channels: usize,
    gains: Vec<f32>,
    pans: Vec<f32>,
    mutes: Vec<bool>,
    solos: Vec<bool>,
    master_gain: f32,
    meters: Arc<MeterBank>,
    /// Per-block peak scratch — one slot per channel plus the master, allocated at
    /// construction and cleared each block, so a mount-time channel count costs the
    /// render path nothing.
    peaks: Vec<f32>,
}

impl MixerNode {
    pub fn new() -> Self {
        Self::with_channels(MIXER_CHANNELS, Arc::new(MeterBank::new(MIXER_CHANNELS)))
    }

    /// Construct with an active channel count and a shared meter bank — the
    /// plugin uses this so the profile can read the meters through the
    /// `mixer.meters` context key. The bank must agree with `channels`.
    pub fn with_channels(channels: usize, meters: Arc<MeterBank>) -> Self {
        let channels = channels.clamp(1, MIXER_CHANNELS_SANITY);
        MixerNode {
            channels,
            gains: vec![1.0; channels],
            pans: vec![0.0; channels],
            mutes: vec![false; channels],
            solos: vec![false; channels],
            master_gain: 1.0,
            meters,
            peaks: vec![0.0; channels + 1],
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
            if i >= self.channels {
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
        // Cleared and reused each block: a field, so the render path still allocates
        // nothing even though the width is a mount parameter.
        for peak in self.peaks.iter_mut() {
            *peak = 0.0;
        }
        for (i, sample) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let mut sl = 0.0f32;
            let mut sr = 0.0f32;
            for (ch, peak) in self.peaks[..n].iter_mut().enumerate() {
                let channel_in = io.audio_ins.get(ch);
                let v = channel_in.get(i).copied().unwrap_or(0.0);
                // Channel meter: post-gain, pre-mute/solo — a muted channel
                // still shows its level. Mute is absolute (wins over solo).
                let g = v * self.gains[ch];
                *peak = peak.max(g.abs());
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
            self.peaks[self.channels] = self.peaks[self.channels].max(full);
        }
        for (k, p) in self.peaks.iter().enumerate() {
            match self.meters.0.get(k) {
                Some(slot) => slot.store(p.to_bits(), Ordering::Relaxed),
                None => debug_assert!(false, "mixer: the meter bank is narrower than the node"),
            }
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

    /// The surface this instance **mounted**: derived from the mount's `channels`, so
    /// the mixer's width is no longer a property of the code. A patch to a channel the
    /// instance did not mount is refused rather than accepted and ignored.
    fn mounted_ports(&self) -> Vec<Port> {
        channel_ports(self.channels)
    }

    fn mounted_params(&self) -> Vec<ParamDef> {
        channel_params(self.channels)
    }

    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
        let meters = Arc::new(MeterBank::new(self.channels));
        // The node declares the surface this instance **mounted**, not the catalog's
        // full `ch0..ch7`: a patch to a channel beyond the mounted count is then
        // refused by the graph itself, so the old "accepted but ignored" path is
        // unreachable rather than merely discouraged.
        let node = api.graph.add_node(
            NodeKind::Opaque(Box::new(MixerNode::with_channels(
                self.channels,
                meters.clone(),
            ))),
            self.mounted_ports(),
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
/// input device's layout (default [`MIXER_CHANNELS`]). The only bound is a **sanity**
/// one ([`MIXER_CHANNELS_SANITY`]): the surface, the node's state and its meters are
/// all allocated from the count the profile chose, so an arbitrary width costs an
/// allocation rather than a code change.
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
    if !(1..=MIXER_CHANNELS_SANITY).contains(&channels) {
        return Err(format!(
            "mixer channels must be 1..={MIXER_CHANNELS_SANITY} (a sanity bound, not a design \
             limit), got {channels}"
        ));
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

    /// The only bound left is a **sanity** one, and it is not a design ceiling: nine
    /// channels is a valid mount (it used to be refused), and so is anything up to the
    /// bound. What the bound protects is gross typos, not width.
    #[test]
    fn factory_accepts_any_sane_width_and_refuses_the_absurd() {
        assert!(mixer_factory(&[("channels", 0.0)]).is_err());
        assert!(mixer_factory(&[("channels", 4.0)]).is_ok());
        for wide in [9.0, 24.0, MIXER_CHANNELS_SANITY as f32] {
            assert!(
                mixer_factory(&[("channels", wide)]).is_ok(),
                "{wide} channels is a valid mount now"
            );
        }
        assert!(mixer_factory(&[("channels", (MIXER_CHANNELS_SANITY + 1) as f32)]).is_err());
        // A fractional count is still refused: channels are lanes, not a knob.
        assert!(mixer_factory(&[("channels", 2.5)]).is_err());
    }

    /// The mounted surface follows the mount, not a constant: a 12-channel mixer offers
    /// twelve channel ports and twelve channels' worth of parameters.
    #[test]
    fn the_mounted_surface_follows_the_mount() {
        let plugin = mixer_factory(&[("channels", 12.0)]).expect("a wide mixer mounts");
        let ports = plugin.mounted_ports();
        let params = plugin.mounted_params();
        assert_eq!(
            ports.iter().filter(|p| p.name.starts_with("ch")).count(),
            12,
            "one channel input per mounted channel"
        );
        assert!(
            ports.iter().any(|p| p.name == "audio"),
            "the master out is there"
        );
        assert!(
            params.iter().any(|p| p.name == "ch11.gain"),
            "the twelfth channel has params"
        );
        assert!(
            !params.iter().any(|p| p.name == "ch12.gain"),
            "and no thirteenth"
        );
        // The catalog stays nominal, so the patch bay has something to offer before a
        // mount exists — but it is not what the instance answers with.
        assert_ne!(ports.len(), MIXER_PORTS.len());
    }
}
