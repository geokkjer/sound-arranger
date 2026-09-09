//! The reference host (UI-as-plugin note): the **Host API contract** and the
//! headless host that exercises it deterministically.
//!
//! The contract has three parts:
//! - **commands** — [`HostCommand`]: engine commands carry an optional
//!   `at_frame` (applied at that frame by rendering up to it; the log records
//!   the same frame — *the log is the command list*), media commands are the
//!   spike-B command types until P1.3 merges them into the log (documented);
//! - **events** — the append-only log stream, meters, peaks, transport (the
//!   host renders these, never computes them; `meters()`/`log()` ship now);
//! - **values** — declarative snapshots the host interprets: the graph value,
//!   `providers_of` (`providers()` ships now), the pool index.
//!
//! [`run_script`] assembles the profile (the "sound clip arranger mixer
//! sampler editor") and executes a command list deterministically: the same
//! script on fresh sessions produces byte-identical bounces. The CLI
//! (`src/main.rs`) is the composition-seams "headless smoke binary". A future
//! Tauri shell implements the identical contract — swapping shells swaps only
//! the transport adapter; the text format (`parse_script`, versioned
//! [`HOST_API_VERSION`]) is the wire schema its commands validate against.
//!
//! Contract names: the command surface's plugin/port/param names are the
//! host registry ([`HOST_PLUGINS`] / [`HOST_PORTS`] / [`HOST_PARAMS`]) — a
//! closed vocabulary, validated per slot.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use engine::*;
use media::{ClipRef, FilePlayer, Mailbox, PlaybackNode, SpliceCmd, DEFAULT_RING_CAPACITY};

/// The Host API contract version. The text format's first line must be
/// `host v{N}`; mismatches are refused (kimi review finding 6).
pub const HOST_API_VERSION: u32 = 1;

/// The host registry: plugins, then ports, then parameters — validated per
/// slot by the parser (a port name is not a plugin name).
pub const HOST_PLUGINS: &[&str] = &["euclidean", "scale", "tone", "mixer"];
pub const HOST_PORTS: &[&str] = &[
    "triggers", "trigger", "note", "audio", "ch0", "ch1", "ch2", "ch3", "ch4", "ch5", "ch6", "ch7",
];
pub const HOST_PARAMS: &[&str] = &[
    "steps", "pulses", "rotation", "pulses_per_beat", "root", "note_len", "gain", "blip_len",
    "channels", "master.gain",
    "ch0.gain", "ch0.mute", "ch0.solo", "ch0.pan", "ch1.gain", "ch1.mute", "ch1.solo", "ch1.pan",
    "ch2.gain", "ch2.mute", "ch2.solo", "ch2.pan", "ch3.gain", "ch3.mute", "ch3.solo", "ch3.pan",
    "ch4.gain", "ch4.mute", "ch4.solo", "ch4.pan", "ch5.gain", "ch5.mute", "ch5.solo", "ch5.pan",
    "ch6.gain", "ch6.mute", "ch6.solo", "ch6.pan", "ch7.gain", "ch7.mute", "ch7.solo", "ch7.pan",
];

fn in_list(list: &'static [&str], s: &str, what: &str) -> Result<&'static str, String> {
    list.iter()
        .find(|n| **n == s)
        .copied()
        .ok_or_else(|| format!("unknown {what} '{s}' (registry: {})", list.join(", ")))
}

/// A command in the Host API contract. Engine commands carry an optional
/// `at_frame` — `None` applies at the current position, `Some(f)` renders up
/// to `f` first, so the log records the command's real frame.
#[derive(Debug, Clone)]
pub enum HostCommand {
    Mount {
        plugin: &'static str,
        params: Vec<(&'static str, f32)>,
        at_frame: Option<u64>,
    },
    Patch {
        from: (&'static str, &'static str),
        to: (&'static str, &'static str),
        at_frame: Option<u64>,
    },
    SetParam {
        plugin: &'static str,
        param: &'static str,
        value: f32,
        at_frame: Option<u64>,
    },
    SetTempo {
        bpm: f64,
        beats_per_bar: u32,
        at_frame: Option<u64>,
    },
    Unmount {
        plugin: &'static str,
        at_frame: Option<u64>,
    },
    /// Play a clip into mixer channel `channel` at the current frame (the
    /// profile's monitoring wiring — the player node routes into the mixer).
    Play {
        clip: ClipRef,
        channel: usize,
        at_frame: Option<u64>,
    },
    /// Splice the playing clip to `clip` at an absolute frame (crossfade).
    Splice {
        at_frame: u64,
        clip: ClipRef,
        crossfade: u32,
    },
    /// Capture device input into a take (declared for the device path; the
    /// device input path is not wired into the reference host — its refusal
    /// names the gap).
    Record {
        take_id: String,
    },
    /// An arrangement edit (the clip editor's ACID op) — a **logged command**
    /// carrying `at_frame`. The host applies it to the clip editor's value and
    /// wires the arrangement nodes into the mixer before rendering. This is the
    /// P1.3.4 replacement for `Play`/`Splice` (which remain for the recorder
    /// player path).
    Arrange {
        op: media::ArrangeOp,
        at_frame: Option<u64>,
    },
    /// Point the host at the media pool (where arrangement clip source paths
    /// resolve). Part of the script so it is self-describing.
    Pool {
        dir: PathBuf,
    },
    /// Render `frames` from the current position and write the master to a
    /// 16-bit WAV.
    Bounce {
        frames: usize,
        path: PathBuf,
    },
}

impl HostCommand {
    fn at_frame(&self) -> Option<u64> {
        match self {
            HostCommand::Mount { at_frame, .. }
            | HostCommand::Patch { at_frame, .. }
            | HostCommand::SetParam { at_frame, .. }
            | HostCommand::SetTempo { at_frame, .. }
            | HostCommand::Unmount { at_frame, .. }
            | HostCommand::Play { at_frame, .. }
            | HostCommand::Arrange { at_frame, .. } => *at_frame,
            _ => None,
        }
    }
}

/// The assembled profile: the engine, the registered factories, the media
/// wiring (player nodes into the mixer), and the master bounce path.
pub struct HostSession {
    engine: Engine,
    /// pending (player node, mixer channel) cords, wired once the mixer's
    /// node exists (flush_scheduled materializes it — no discarded audio).
    pending_cords: Vec<(NodeId, usize)>,
    player_mailbox: Option<Mailbox>,
    player_underruns: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    player_deferred: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    /// per-track arranger underrun counters (one Arc per wired track). The
    /// render path writes into the nodes' counters; the control side reads them
    /// here, so a reader slip in the arranger is surfaced instead of silently
    /// glitching the bounce. Rebuilt with every `wire_arranger`.
    arranger_underruns: Vec<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    /// the mixer's mounted channel count (validated at Play apply).
    mixer_channels: Option<usize>,
    /// media commands applied (the log covers engine commands only until
    /// P1.3; the host reports both — kimi review finding 9).
    media_commands: usize,
    /// the clip editor (lazy): owns the arrangement value + op handlers.
    editor: Option<media::ClipEditor>,
    /// resolves a pool source id (stem) to its `.wav` path, set by `set_pool`.
    pool_resolver: Option<media::PoolResolver>,
    /// track id → the arranger node mounted for it (avoids re-wiring on a rebuild).
    wired_tracks: std::collections::HashMap<String, engine::NodeId>,
    /// the arrangement value changed since last wiring (re-wire before render).
    arrange_dirty: bool,
}

impl HostSession {
    /// Create a live host session. The reference host's one-shot path is
    /// [`run_script`]; this is the persistent form a UI holds and edits
    /// incrementally via [`execute`](Self::execute).
    pub fn new() -> Self {
        let mut engine = Engine::new(48_000, 120.0, 4);
        engine.register_factory("euclidean", plugins::euclidean_factory, plugins::euclidean::EUCLIDEAN_PORTS, &[]);
        engine.register_factory("scale", plugins::scale_factory, plugins::scale::SCALE_PORTS, &[]);
        engine.register_factory("tone", plugins::tone_factory, plugins::tone::TONE_PORTS, plugins::tone::TONE_PARAMS);
        engine.register_factory("mixer", plugins::mixer_factory, plugins::mixer::MIXER_PORTS, plugins::mixer::MIXER_PARAMS);
        HostSession {
            engine,
            pending_cords: Vec::new(),
            player_mailbox: None,
            player_underruns: None,
            player_deferred: None,
            arranger_underruns: Vec::new(),
            mixer_channels: None,
            media_commands: 0,
            editor: None,
            pool_resolver: None,
            wired_tracks: std::collections::HashMap::new(),
            arrange_dirty: false,
        }
    }

    /// Set the media pool for arrangement clips. The pool (float-WAV sources by
    /// stem id) is where the clip editor's clips resolve their source paths.
    pub fn set_pool(&mut self, pool_dir: impl Into<PathBuf>) -> Result<(), String> {
        let dir: PathBuf = pool_dir.into();
        media::Pool::open(&dir)?; // validate it exists as a directory
        let resolver: media::PoolResolver = std::sync::Arc::new(move |id| {
            let p = dir.join(format!("{id}.wav"));
            p.is_file().then_some(p)
        });
        self.pool_resolver = Some(resolver);
        Ok(())
    }

    /// Ensure the clip editor exists and its op handlers are registered.
    fn ensure_editor(&mut self) -> Result<(), String> {
        if self.editor.is_none() {
            let ed = media::ClipEditor::new();
            ed.register(&mut self.engine)?;
            self.editor = Some(ed);
        }
        Ok(())
    }

    /// Read access to the engine (the summary/`summarize` reads the graph). A
    /// real host speaks `HostCommand`, not `&mut Engine` — the boundary the note
    /// draws against in-process shells (kimi nit 16).
    pub fn engine_ref(&self) -> &Engine {
        &self.engine
    }

    /// Resolve a parsed clip (len 0 = "open the file at apply") to its real
    /// length.
    fn resolve_clip(clip: &ClipRef) -> Result<ClipRef, String> {
        if clip.len == 0 {
            ClipRef::whole(&clip.path)
        } else {
            Ok(clip.clone())
        }
    }

    /// Warm a player's ring synchronously: determinism must not rest on a
    /// thread race (kimi review finding 1). Blocks until the ring is full (or
    /// the clip is done), bounded.
    fn warm_player(player: &FilePlayer, cap: usize) -> Result<(), String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while player.produced() < cap as u64 && !player.eof() {
            if std::time::Instant::now() > deadline {
                return Err("player ring did not warm up in 10 s".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Ok(())
    }

    fn apply(&mut self, cmd: &HostCommand) -> Result<(), String> {
        match cmd {
            HostCommand::Mount { plugin, params, .. } => self.engine.mount(plugin, params),
            HostCommand::Patch { from, to, .. } => self.engine.patch(*from, *to),
            HostCommand::SetParam { plugin, param, value, .. } => {
                self.engine.set_param(plugin, param, *value)
            }
            HostCommand::SetTempo { bpm, beats_per_bar, .. } => {
                self.engine.set_tempo(*bpm, *beats_per_bar)
            }
            HostCommand::Unmount { plugin, .. } => {
                let r = self.engine.unmount(plugin);
                if *plugin == "mixer" {
                    // the mixer (the bus) is gone — clear the host's mixer-dependent
                    // state so a later engine command cannot reach a stale
                    // `wire_pending`/`wire_arranger` and panic (GLM-5.3 review #6).
                    self.pending_cords.clear();
                    self.player_mailbox = None;
                    self.player_underruns = None;
                    self.player_deferred = None;
                    self.mixer_channels = None;
                }
                r
            }
            HostCommand::Play { clip, channel, .. } => {
                let channels = self.mixer_channels.ok_or(
                    "play requires the mixer to be mounted first (mount mixer channels=N)",
                )?;
                if *channel >= channels {
                    return Err(format!("play channel ch{channel} is beyond the mixer's {channels} channels"));
                }
                if self.player_mailbox.is_some() {
                    return Err("the reference host plays one clip at a time".into());
                }
                let clip = Self::resolve_clip(clip)?;
                let player = FilePlayer::start(clip, DEFAULT_RING_CAPACITY)?;
                Self::warm_player(&player, DEFAULT_RING_CAPACITY)?; // deterministic, no race
                let mailbox = media::mailbox();
                let node = PlaybackNode::new(Some(player), mailbox.clone());
                let underruns = node.underrun_counter();
                let deferred = node.deferred_counter();
                // Place the player BEFORE the mixer in topological order (the
                // graph's forward-order rule): if the mixer is already
                // materialized, insert_before(mixer); otherwise add (append) and
                // the mixer materializes later and lands after the player. A
                // player appended after a materialized mixer would make its cord
                // backward — every later render fails.
                let node = NodeKind::Opaque(Box::new(node));
                let ports = vec![Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio, channels: 1 }];
                let id = match self.engine.graph.out_node {
                    Some(mixer) => self.engine.graph.insert_before(mixer, node, ports)
                        .map_err(|e| format!("play node: {e}"))?,
                    None => self.engine.graph.add_node(node, ports),
                };
                self.player_mailbox = Some(mailbox);
                self.player_underruns = Some(underruns);
                self.player_deferred = Some(deferred);
                self.pending_cords.push((id, *channel));
                self.media_commands += 1;
                Ok(())
            }
            HostCommand::Splice { at_frame, clip, crossfade } => {
                let Some(mailbox) = &self.player_mailbox else {
                    return Err("splice requires a playing clip".into());
                };
                let clip = Self::resolve_clip(clip)?;
                let incoming = FilePlayer::start(clip, DEFAULT_RING_CAPACITY)?;
                Self::warm_player(&incoming, DEFAULT_RING_CAPACITY)?;
                mailbox.lock().map_err(|_| "player mailbox poisoned")?.push_back(SpliceCmd {
                    at_frame: *at_frame,
                    incoming,
                    crossfade: *crossfade,
                });
                self.media_commands += 1;
                Ok(())
            }
            HostCommand::Record { .. } => {
                Err("recording requires a device — the device input path exists in media::devices (open_input) but is not wired into the host; the reference host renders offline via Bounce".into())
            }
            HostCommand::Arrange { op, .. } => {
                self.ensure_editor()?;
                if self.pool_resolver.is_none() {
                    return Err("arrange requires set_pool first (clips need pool-source paths)".into());
                }
                let editor = self.editor.as_mut().ok_or("clip editor not initialized")?;
                editor.apply(&mut self.engine, op)?;
                self.arrange_dirty = true;
                self.media_commands += 1;
                Ok(())
            }
            HostCommand::Pool { dir } => {
                self.set_pool(dir.clone())?;
                self.media_commands += 1;
                Ok(())
            }
            HostCommand::Bounce { frames, path } => {
                let out = self.render(*frames)?; // wires (pending + arranger) then renders
                let channels = self.engine.graph.out_channels().max(1) as u16;
                let mut w = media::WavWriter::create(path, self.engine.clock.sample_rate, channels)?;
                w.write(&out)?;
                w.finalize()?;
                self.media_commands += 1;
                Ok(())
            }
        }
    }

    /// Wire the pending player→mixer cords after materializing the scheduled
    /// mounts (the mixer claims the bus) — `flush_scheduled`, no discarded
    /// audio (kimi review finding 5). Infallible after Play's validation.
    fn wire_pending(&mut self) -> Result<(), String> {
        if self.pending_cords.is_empty() {
            return Ok(());
        }
        self.engine.flush_scheduled();
        let mixer = self.engine
            .graph
            .out_node
            .ok_or("play requires the mixer to be mounted (mount mixer first)")?;
        let pending = std::mem::take(&mut self.pending_cords);
        for (player, channel) in pending {
            self.engine
                .graph
                .connect(player, "audio", mixer, &format!("ch{channel}"))
                .map_err(|e| format!("player→mixer cord: {e}"))?;
        }
        Ok(())
    }

    /// Reconcile the arranger nodes against the current arrangement value.
    ///
    /// The reference host is **reconcile-on-dirty**, not add-only: an `Arrange`
    /// op mutates the value eagerly, and the next render re-derives the graph
    /// from that value. This is what makes edits reach audio instead of leaving
    /// a stale node playing. Specifically it fixes the `RemoveTrack` ghost —
    /// previously a wired track was never retired, so removing it left its
    /// `ArrangerNode` mounted and audibly playing while the UI showed no track.
    ///
    /// **Order matters** (the graph's topological rule): the arranger source
    /// nodes must precede the mixer. The mixer is the last node (it claims
    /// `out_node`), so each rebuilt arranger node is `insert_before(mixer)`-ed,
    /// keeping `arranger → mixer` a forward cord. `flush_scheduled` materializes
    /// the mixer (and any pending mount) before the inserts, so the pivot exists.
    ///
    /// Channels map to **track index** (`ch{ti}`), the same stable mapping as a
    /// fresh arrangement, so a removed track also re-numbers its successors.
    /// Control side only — readers are warmed here; the render path never touches
    /// this.
    fn wire_arranger(&mut self) -> Result<(), String> {
        if !self.arrange_dirty {
            return Ok(());
        }
        let Some(resolver) = self.pool_resolver.clone() else { return Ok(()) };
        let Some(editor) = &self.editor else { return Ok(()) };
        let timeline = editor.snapshot()?;
        let channels = self.mixer_channels.unwrap_or(4);
        let from_frame = self.engine.clock.frame();

        // **Validate before mutating** (the engine's own OpHandler contract: an
        // `Err` means nothing changed). Two read-only steps must precede any
        // teardown: (a) the mixer must be present — materialize scheduled mounts
        // and resolve it by *name*, not by `graph.out_node` (the bus points at
        // whatever audio node was mounted last; if the mixer was unmounted and
        // some other node claimed the bus, `out_node` would be misleading), and
        // (b) every new reader must build fine (missing source, rate mismatch,
        // too many tracks) — this is the fallible part. Only when both succeed
        // do we tear down the old wiring, so a failed reconcile never leaves a
        // partial arrangement behind and the dirty flag survives for a clean
        // retry.
        self.engine.flush_scheduled();
        let mixer = self
            .engine
            .node_of("mixer")
            .ok_or("arrange requires the mixer to be mounted (mount mixer channels=N)")?;

        // A new wiring replaces the old nodes (and their counters); drop the
        // previous counters now so underruns() reflects the current wiring.
        self.arranger_underruns.clear();
        let mut built: Vec<(String, usize, media::ArrangerNode)> = Vec::new();
        for (ti, track) in timeline.tracks.iter().enumerate() {
            if ti >= channels {
                return Err(format!("track '{}' has no mixer channel ch{ti} (channels={channels})", track.id));
            }
            let node = media::ArrangerNode::new(track.clone(), &resolver, media::DEFAULT_RING_CAPACITY, self.engine.clock.sample_rate, from_frame)?;
            // Keep a handle to the node's underrun counter so the host can
            // surface a reader slip even after the node is moved into the graph.
            self.arranger_underruns.push(node.underruns_arc());
            built.push((track.id.clone(), ti, node));
        }

        // All validated — now commit (this phase cannot fail: `insert_before`
        // has a live pivot and `connect` is forward-ordered with `ch0..ch7`
        // declared). Retire the previous wiring, then place each new node.
        let old_nodes: Vec<engine::NodeId> = self.wired_tracks.drain().map(|(_, id)| id).collect();
        for id in old_nodes {
            self.engine.graph.remove_node(id);
        }

        for (tid, ti, node) in built {
            let id = self.engine.graph.insert_before(
                mixer,
                engine::NodeKind::Opaque(Box::new(node)),
                vec![engine::Port { name: "audio", direction: engine::Direction::Out, kind: engine::SignalKind::Audio , channels: 1 }],
            )?;
            self.engine
                .graph
                .connect(id, "audio", mixer, &format!("ch{ti}"))
                .map_err(|e| format!("arranger→mixer cord: {e}"))?;
            self.wired_tracks.insert(tid, id);
        }
        self.arrange_dirty = false;
        Ok(())
    }

    /// The reference host renders the whole bounce into one f32 buffer; bound the
    /// *bytes* so a malformed `bounce 999999999999` cannot OOM (≈1.5 h @48 kHz).
    const MAX_BOUNCE_BYTES: usize = 1 << 30;

    /// Render `frames` from the current position (wiring pending cords/arranger
    /// first). A wiring failure (e.g. the mixer was unmounted after a `play`) is
    /// a clean `Err`, never a panic in a host.
    pub fn render(&mut self, frames: usize) -> Result<Vec<f32>, String> {
        // The master may be stereo (L/R), so a frame costs up to 2 * 4 bytes.
        let budget = frames
            .saturating_mul(std::mem::size_of::<f32>())
            .saturating_mul(2);
        if budget > Self::MAX_BOUNCE_BYTES {
            return Err(format!("bounce of {frames} frames exceeds the ~{:.0} MiB budget", Self::MAX_BOUNCE_BYTES / (1 << 20)));
        }
        self.wire_pending()?;
        self.wire_arranger()?;
        Ok(self.engine.render(frames))
    }

    pub fn log(&self) -> &SessionLog {
        &self.engine.log
    }

    pub fn event_count(&self) -> usize {
        self.engine.log.len()
    }

    pub fn media_command_count(&self) -> usize {
        self.media_commands
    }

    /// The clip editor's arrangement value (read-only snapshot). `Ok(default)`
    /// when no arrangement has been built (the legitimate empty case); a
    /// snapshot error (a poisoned timeline) is **propagated** — a shell must
    /// see "could not build the arrangement", never a silent empty value. The
    /// value is a pure reconstruction of the logged `Arrange` commands
    /// (byte-identically replayable).
    pub fn arrangement(&self) -> Result<media::Timeline, String> {
        match &self.editor {
            None => Ok(media::Timeline::default()),
            Some(e) => e.snapshot(),
        }
    }

    /// Total underruns across the player and every wired arranger track. A non-
    /// zero value means a reader slipped somewhere on the render path and the
    /// bounce is not exact — the summary surfaces it instead of hiding it.
    pub fn underruns(&self) -> u64 {
        let player = self
            .player_underruns
            .as_ref()
            .map(|u| u.load(Ordering::Relaxed))
            .unwrap_or(0);
        let arrangers: u64 = self
            .arranger_underruns
            .iter()
            .map(|u| u.load(Ordering::Relaxed))
            .sum();
        player + arrangers
    }

    /// Splices whose frame had passed when applied — 0 means the splice was
    /// sample-accurate.
    pub fn deferred(&self) -> u64 {
        self.player_deferred
            .as_ref()
            .map(|u| u.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// The mixer's meters (an "event" the host renders — a shell reads the
    /// same accessor; nothing is computed host-side).
    pub fn meters(&self) -> Option<std::sync::Arc<MeterBank>> {
        self.engine.ctx.get::<std::sync::Arc<MeterBank>>("mixer.meters").cloned()
    }

    /// The patch bay's provider registry (a "value" the host interprets —
    /// the dropdown's data source).
    pub fn providers(&self, kind: engine::SignalKind) -> Vec<&'static str> {
        self.engine.provider_names_of(kind)
    }
}

impl std::fmt::Debug for HostSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostSession")
            .field("event_count", &self.event_count())
            .field("media_commands", &self.media_command_count())
            .field("underruns", &self.underruns())
            .field("deferred", &self.deferred())
            .finish()
    }
}

/// Execute a command script deterministically. Engine commands with an
/// `at_frame` render up to that frame first, so the log records real frames;
/// the final `Bounce` writes the master. The same script on fresh sessions
/// bounces byte-identically.
pub fn run_script(script: &[HostCommand]) -> Result<HostSession, String> {
    let mut session = HostSession::new();
    for cmd in script {
        session.process(cmd)?;
    }
    Ok(session)
}

impl HostSession {
    /// Apply a single command onto this persistent session — the incremental
    /// form of [`run_script`], for a live UI that edits one op at a time. A
    /// refused op returns `Err` and changes nothing; the session stays usable.
    pub fn execute(&mut self, cmd: &HostCommand) -> Result<(), String> {
        self.process(cmd)
    }

    /// Process one command exactly as the host contract does: validate the
    /// mixer mount, render up to the command's absolute frame, then `apply`.
    /// Shared by [`run_script`] and [`execute`] so the live path can never
    /// diverge from the one-shot path.
    fn process(&mut self, cmd: &HostCommand) -> Result<(), String> {
        let mut pending_mixer_channels = None;
        if let HostCommand::Mount { plugin: "mixer", params, .. } = cmd {
            // The mixer's channel count is the contract's own state. A fraction
            // (e.g. channels=4.5) must be refused, not silently truncated (the
            // review's "float cast" nit), and it must be a sane positive count.
            let channels = params
                .iter()
                .find(|(name, _)| *name == "channels")
                .map(|(_, v)| {
                    if !v.is_finite() || *v <= 0.0 || v.fract() != 0.0 {
                        return Err(format!("mixer channels must be a positive whole number, got {v}"));
                    }
                    Ok(*v as usize)
                })
                .transpose()?
                .unwrap_or(4);
            if channels > MIXER_CHANNELS_MAX {
                return Err(format!("mixer channels {channels} exceeds the max {MIXER_CHANNELS_MAX}"));
            }
            // Defer committing the channel count until apply succeeds: a refused
            // re-mount must not overwrite the live mixer's channel count (a
            // refused op changes nothing).
            pending_mixer_channels = Some(channels);
        }
        if let Some(frame) = cmd.at_frame() {
            let now = self.engine.clock.frame();
            if frame > now {
                // The pre-render wiring can fail (a later command after the
                // mixer was unmounted); a clean Err, never a panic in a host.
                self.render((frame - now) as usize)?;
            }
        }
        let r = self.apply(cmd);
        if r.is_ok() {
            if let Some(ch) = pending_mixer_channels {
                self.mixer_channels = Some(ch);
            }
        }
        r
    }
}

// ---------------------------------------------------------------- text form

/// The text format (the wire schema, versioned): the first line must be
/// `host v{N}`; then one command per line:
///
/// ```text
/// host v1
/// mount tone gain=0.25 blip_len=1200 @0
/// mount mixer channels=4 @0
/// patch euclidean.triggers scale.trigger @0
/// set_param mixer ch0.gain 0.5 @2000
/// set_tempo 240 4 @0
/// unmount tone @3000
/// play /abs/clip.wav ch0 @0
/// splice 4000 /abs/clip2.wav 512
/// pool /data/takes                           # the media pool dir
/// arrange add_track t0 @0                    # the clip editor (P1.3.4)
/// arrange add_clip t0 c0 s1 0 4000 0 0 0 1.0 @0
/// bounce 6000 /abs/out.wav
/// ```
pub fn parse_script(text: &str) -> Result<Vec<HostCommand>, String> {
    let mut lines = text.lines();
    let mut commands = Vec::new();

    // version line (the first non-empty, non-comment line), tracking the
    // 1-based line number so error messages stay correct even with leading
    // blank/comment lines before the header.
    let mut header_line = 0usize;
    let version_line = loop {
        let raw = lines.next().ok_or("empty script (want 'host v1' first)")?;
        header_line += 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if !line.is_empty() {
            break line;
        }
    };
    let expected = format!("host v{HOST_API_VERSION}");
    if version_line != expected {
        return Err(format!("bad version line '{version_line}' (want '{expected}')"));
    }

    for (lineno, raw) in lines.enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let at = header_line + lineno + 1; // header line + loop offset (lineno is 0-based)
        let mut words: Vec<&str> = line.split_whitespace().collect();
        // optional trailing @frame token
        let at_frame = match words.last() {
            Some(tok) if tok.starts_with('@') => {
                let f = tok[1..]
                    .parse::<u64>()
                    .map_err(|_| format!("line {at}: bad frame '{tok}'"))?;
                words.pop();
                Some(f)
            }
            _ => None,
        };
        let kind = words.first().copied().unwrap_or("");
        match kind {
            "mount" => {
                let plugin = in_list(HOST_PLUGINS, word(&words, 1, at)?, "plugin")?;
                let mut params = Vec::new();
                for w in &words[2..] {
                    let Some((k, v)) = w.split_once('=') else {
                        return Err(format!("line {at}: bad param '{w}' (want k=v)"));
                    };
                    let value = v.parse::<f32>().map_err(|_| format!("line {at}: bad value '{v}'"))?;
                    params.push((in_list(HOST_PARAMS, k, "param")?, value));
                }
                commands.push(HostCommand::Mount { plugin, params, at_frame });
            }
            "patch" => {
                let from = port_ref(&words, 1, at)?;
                let to = port_ref(&words, 2, at)?;
                commands.push(HostCommand::Patch { from, to, at_frame });
            }
            "set_param" => {
                let plugin = in_list(HOST_PLUGINS, word(&words, 1, at)?, "plugin")?;
                let param = in_list(HOST_PARAMS, word(&words, 2, at)?, "param")?;
                let value = word(&words, 3, at)?.parse::<f32>().map_err(|_| format!("line {at}: bad value"))?;
                commands.push(HostCommand::SetParam { plugin, param, value, at_frame });
            }
            "set_tempo" => {
                let bpm = word(&words, 1, at)?.parse().map_err(|_| format!("line {at}: bad bpm"))?;
                let beats = word(&words, 2, at)?.parse().map_err(|_| format!("line {at}: bad beats"))?;
                commands.push(HostCommand::SetTempo { bpm, beats_per_bar: beats, at_frame });
            }
            "unmount" => {
                let plugin = in_list(HOST_PLUGINS, word(&words, 1, at)?, "plugin")?;
                commands.push(HostCommand::Unmount { plugin, at_frame });
            }
            "play" => {
                let clip = ClipRef { path: PathBuf::from(word(&words, 1, at)?), start: 0, len: 0 };
                let channel = channel_of(word(&words, 2, at)?, at)?;
                commands.push(HostCommand::Play { clip, channel, at_frame });
            }
            "splice" => {
                let at_frame = word(&words, 1, at)?.parse().map_err(|_| format!("line {at}: bad frame"))?;
                let clip = ClipRef { path: PathBuf::from(word(&words, 2, at)?), start: 0, len: 0 };
                let crossfade = word(&words, 3, at)?.parse().map_err(|_| format!("line {at}: bad crossfade"))?;
                commands.push(HostCommand::Splice { at_frame, clip, crossfade });
            }
            "record" => {
                commands.push(HostCommand::Record { take_id: word(&words, 1, at)?.to_string() });
            }
            "bounce" => {
                let frames = word(&words, 1, at)?.parse().map_err(|_| format!("line {at}: bad frames"))?;
                let path = PathBuf::from(word(&words, 2, at)?);
                commands.push(HostCommand::Bounce { frames, path });
            }
            "pool" => {
                let dir = PathBuf::from(word(&words, 1, at)?);
                commands.push(HostCommand::Pool { dir });
            }
            "arrange" => {
                let op = parse_arrange(&words[1..], at)?;
                commands.push(HostCommand::Arrange { op, at_frame });
            }
            other => return Err(format!("line {at}: unknown command '{other}'")),
        }
    }
    Ok(commands)
}

/// Parse an `arrange` command's operands (everything after the `arrange`
/// keyword) into an [`ArrangeOp`]. Grammar per op:
///
/// ```text
/// arrange add_track t0 @0
/// arrange add_clip t0 c0 s1 0 4000 0 0 0 1.0 [loop_len] @0
/// arrange razor_split t0 c0 cL cR 3000 @0
/// arrange trim t0 c0 start 500 @0
/// arrange move_clip t0 c0 9000 @0
/// arrange move_clip_to_track t0 c0 t1 50 @0
/// arrange duplicate t0 c0 c1 @0
/// arrange delete t0 c0 @0
/// arrange set_clip_gain t0 c0 0.75 @0
/// arrange set_clip_fade t0 c0 64 128 @0
/// arrange loop_region t0 c0 3 @0
/// arrange chop t0 c0 4 pre @0    # split c0 into 4 contiguous pieces (ids pre.0..pre.3)
/// ```
fn parse_arrange(words: &[&str], at: usize) -> Result<media::ArrangeOp, String> {
    let op = words.first().copied().ok_or_else(|| format!("line {at}: arrange needs an op"))?;
    // helpers: operand i is `words[i]` — `words[0]` is the op name, so the first
    // real operand (e.g. add_track's track) is `words[1]` = operand 1. Error
    // messages number operands the way a script author counts them (`i`, not i+1).
    let s = |i: usize| -> Result<String, String> {
        words.get(i).copied().map(str::to_owned)
            .ok_or_else(|| format!("line {at}: arrange {op} missing operand {i}"))
    };
    let u = |i: usize| -> Result<u64, String> {
        s(i)?.parse().map_err(|_| format!("line {at}: arrange {op} operand {i} must be a frame/count"))
    };
    let i64 = |i: usize| -> Result<i64, String> {
        s(i)?.parse().map_err(|_| format!("line {at}: arrange {op} operand {i} must be an integer"))
    };
    let f = |i: usize| -> Result<f32, String> {
        let v: f32 = s(i)?.parse().map_err(|_| format!("line {at}: arrange {op} operand {i} must be a number"))?;
        if !v.is_finite() {
            return Err(format!("line {at}: arrange {op} operand {i} must be finite (a NaN/inf gain or count reaches the audio path)"));
        }
        Ok(v)
    };
    // strict arity: a wrong operand COUNT is a parse error (the wire schema must
    // not silently accept trailing junk). `add_clip`'s `[loop_len]` is the one
    // documented optional trailing operand.
    let arity = |n: usize| -> Result<(), String> {
        if words.len() != n {
            return Err(format!("line {at}: arrange {op} expects {n} words (op + operands), got {}", words.len()));
        }
        Ok(())
    };

    match op {
        "add_track" => { arity(2)?; Ok(media::ArrangeOp::AddTrack { track: s(1)? }) }
        "remove_track" => { arity(2)?; Ok(media::ArrangeOp::RemoveTrack { track: s(1)? }) }
        "add_clip" => {
            // add_clip track c0 source src_start src_len at_frame fade_in fade_out gain [loop_len]
            // operands (words[0]=op): track(1) id(2) source(3) src_start(4) src_len(5)
            //                       at_frame(6) fade_in(7) fade_out(8) gain(9) loop_len(10)
            if !(10..=11).contains(&words.len()) {
                return Err(format!("line {at}: arrange add_clip expects 10 or 11 words (op + 9 operands + optional loop_len), got {}", words.len()));
            }
            let clip = media::Clip {
                id: s(2)?,
                source: s(3)?,
                src_start: u(4)?,
                src_len: u(5)?,
                at_frame: u(6)?,
                fade_in: u(7)?,
                fade_out: u(8)?,
                gain: f(9)?,
                loop_len: words.get(10).map(|x| x.parse::<u64>()).transpose().map_err(|_| format!("line {at}: arrange add_clip loop_len must be a count"))?.filter(|v| *v != 0),
            };
            Ok(media::ArrangeOp::AddClip { track: s(1)?, clip })
        }
        "razor_split" => { arity(6)?; Ok(media::ArrangeOp::RazorSplit {
            track: s(1)?, clip: s(2)?, new_left: s(3)?, new_right: s(4)?, at_frame: u(5)?,
        }) }
        "trim" => { arity(5)?; Ok(media::ArrangeOp::Trim {
            track: s(1)?, clip: s(2)?,
            edge: match s(3)?.as_str() { "start" => media::Edge::Start, "end" => media::Edge::End, other => return Err(format!("line {at}: arrange trim edge must be start|end, got '{other}'")) },
            by_frames: i64(4)?,
        }) }
        "move_clip" => { arity(4)?; Ok(media::ArrangeOp::MoveClip { track: s(1)?, clip: s(2)?, at_frame: u(3)? }) }
        "move_clip_to_track" => { arity(5)?; Ok(media::ArrangeOp::MoveClipToTrack { from: s(1)?, clip: s(2)?, to: s(3)?, at_frame: u(4)? }) }
        "duplicate" => { arity(4)?; Ok(media::ArrangeOp::Duplicate { track: s(1)?, clip: s(2)?, new_id: s(3)? }) }
        "delete" => { arity(3)?; Ok(media::ArrangeOp::Delete { track: s(1)?, clip: s(2)? }) }
        "set_clip_gain" => { arity(4)?; Ok(media::ArrangeOp::SetClipGain { track: s(1)?, clip: s(2)?, gain: f(3)? }) }
        "set_clip_fade" => { arity(5)?; Ok(media::ArrangeOp::SetClipFade { track: s(1)?, clip: s(2)?, fade_in: u(3)?, fade_out: u(4)? }) }
        "loop_region" => { arity(4)?; Ok(media::ArrangeOp::LoopRegion {
            track: s(1)?, clip: s(2)?,
            times: u32::try_from(u(3)?).map_err(|_| format!("line {at}: arrange loop_region operand 3 must fit a u32 (times)"))?,
        }) }
        "chop" => { arity(5)?; Ok(media::ArrangeOp::ChopClip {
            track: s(1)?, clip: s(2)?,
            times: u32::try_from(u(3)?).map_err(|_| format!("line {at}: arrange chop operand 3 must fit a u32 (times)"))?,
            prefix: s(4)?,
        }) }
        other => Err(format!("line {at}: unknown arrange op '{other}'")),
    }
}

/// The `idx`-th word of a command line; a missing operand is a clean parse error
/// (never an index-out-of-bounds panic on a truncated line — the wire schema's
/// public parser must not panic).
fn word<'a>(words: &'a [&str], idx: usize, at: usize) -> Result<&'a str, String> {
    words
        .get(idx)
        .copied()
        .ok_or_else(|| format!("line {at}: missing command operand (need {})", idx + 1))
}

fn port_ref(words: &[&str], idx: usize, at: usize) -> Result<(&'static str, &'static str), String> {
    let s = words.get(idx).ok_or_else(|| format!("line {at}: expected a plugin.port reference"))?;
    let Some((plugin, port)) = s.split_once('.') else {
        return Err(format!("line {at}: bad port reference '{s}' (want plugin.port)"));
    };
    Ok((in_list(HOST_PLUGINS, plugin, "plugin")?, in_list(HOST_PORTS, port, "port")?))
}

fn channel_of(s: &str, at: usize) -> Result<usize, String> {
    let Some(idx) = s.strip_prefix("ch") else {
        return Err(format!("line {at}: bad channel '{s}' (want ch0..ch7)"));
    };
    let n: usize = idx.parse().map_err(|_| format!("line {at}: bad channel '{s}'"))?;
    if n > 7 {
        return Err(format!("line {at}: channel ch{n} out of range (ch0..ch7)"));
    }
    Ok(n)
}

/// A structured summary of a finished script run (the CLI's report; a future
/// shell renders the same facts).
pub fn summarize(session: &HostSession) -> String {
    format!(
        "engine log events: {}\nmedia commands: {}\nunderruns: {}\ndeferred splices: {}\nmaster out node: {:?}\n",
        session.event_count(),
        session.media_command_count(),
        session.underruns(),
        session.deferred(),
        session.engine_ref().graph.out_node,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only used by the debug-gated poisoning test; gate the import so release
    // clippy (-D warnings) doesn't flag them as unused.
    #[cfg(debug_assertions)]
    use std::panic::{catch_unwind, AssertUnwindSafe};

    /// No `Arrange` command has run, so there is no editor: `arrangement()` is
    /// `Ok(default)` — the legitimate "nothing built yet" case, never an error.
    #[test]
    fn arrangement_without_an_editor_is_ok_default() {
        let session = HostSession::new();
        assert_eq!(
            session.arrangement().expect("no editor must be Ok, never Err"),
            media::Timeline::default(),
            "no editor → the default (empty) timeline"
        );
    }

    /// An editor whose `snapshot()` errors must surface as `Err` — never a
    /// silent empty `Timeline` (the fail-loud rule this accessor exists for).
    ///
    /// The error is induced the way it realistically occurs: a panic while the
    /// editor holds its timeline lock. `SetClipFade` with `fade_in + fade_out`
    /// overflowing `u64` trips the debug overflow check inside the op apply,
    /// *under the lock* — the poisoned mutex is exactly what `snapshot()` maps
    /// to `Err`. The panic is contained with `catch_unwind` so the session (and
    /// its now-poisoned editor) survives to be read. Gated on
    /// `debug_assertions` because the mechanism is an overflow check (release
    /// builds have no reachable poison path through the public API).
    #[test]
    #[cfg(debug_assertions)]
    fn arrangement_propagates_an_errored_snapshot_instead_of_an_empty_value() {
        let mut session = HostSession::new();
        session.ensure_editor().expect("the editor registers its op handlers");
        let editor = session.editor.as_mut().expect("ensure_editor built the editor");

        // A valid track + clip so the overflowing SetClipFade reaches the
        // overflow add (an absent clip would refuse before it).
        let mut engine = Engine::new(48_000, 120.0, 4);
        editor.register(&mut engine).expect("register op handlers");
        editor
            .apply(&mut engine, &media::ArrangeOp::AddTrack { track: "t0".into() })
            .expect("add track");
        editor
            .apply(
                &mut engine,
                &media::ArrangeOp::AddClip {
                    track: "t0".into(),
                    clip: media::Clip {
                        id: "c0".into(),
                        source: "s1".into(),
                        src_start: 0,
                        src_len: 4000,
                        at_frame: 0,
                        fade_in: 0,
                        fade_out: 0,
                        gain: 1.0,
                        loop_len: None,
                    },
                },
            )
            .expect("add clip");

        // Poison: the overflow panics while the editor holds the timeline lock.
        let poisoned = catch_unwind(AssertUnwindSafe(|| {
            let _ = editor.apply(
                &mut engine,
                &media::ArrangeOp::SetClipFade {
                    track: "t0".into(),
                    clip: "c0".into(),
                    fade_in: u64::MAX,
                    fade_out: 1,
                },
            );
        }))
        .is_err();
        assert!(poisoned, "the overflowing SetClipFade must panic (overflow check) to poison the lock");

        // The session survived the contained panic; its snapshot now errors and
        // arrangement() must propagate that Err — never Ok(empty).
        let err = session
            .arrangement()
            .expect_err("a poisoned editor must Err, not return an empty Timeline");
        assert!(err.contains("poison"), "the error is the snapshot poison, got: {err}");
    }
}
