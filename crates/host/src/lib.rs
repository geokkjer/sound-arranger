//! The reference host (UI-as-plugin note): the **Host API contract** and the
//! headless host that exercises it deterministically.
//!
//! The contract has three parts:
//! - **commands** — [`HostCommand`]: engine commands carry an optional
//!   `at_frame` (applied at that frame by rendering up to it; the log records
//!   the same frame — *the log is the command list*). Media commands are logged
//!   too: `pool`/`play`/`splice` ride the same `Event::Arrangement` carrier as
//!   media ops (see [`media_ops`]), so media determinism is in the one log;
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
use std::sync::{Arc, Mutex};

use engine::*;
use media::{ClipRef, FilePlayer, Interner, Mailbox, PlaybackNode, SpliceCmd, DEFAULT_RING_CAPACITY};

pub mod live;
pub mod media_ops;

use media_ops::{BounceRecord, MediaSession, PlayerIntent, SpliceIntent};

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
    /// Transport: start the clock running from the current position. The offline
    /// reference host only records this state; a shell's live runtime reads it
    /// and pumps blocks while it is set.
    TransportPlay,
    /// Transport: halt at the current position (the clock does not move while
    /// stopped; nothing renders).
    TransportStop,
    /// Transport: move the playhead to an absolute frame. v1 is **rebuild +
    /// render-to-target** (see `seek_to`): correct by construction over the
    /// deterministic log, O(target) because the core clock only advances by
    /// rendering. Intended for seek-while-stopped.
    TransportSeek { frame: u64 },
    /// Undo the most recent **arrangement** edit: drop it from the state history
    /// and rebuild to the current position, so the playhead and the graph state
    /// do not jump. A no-op when there is nothing to undo.
    Undo,
    /// Redo the most recently undone edit (re-inserted at its original history
    /// position, so the reconstruction is faithful). Cleared by a new edit.
    Redo,
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

    /// Whether this command defines *state* — replayed to rebuild a session on a
    /// seek (see `seek_to`). The media commands that shape the session
    /// (`pool`/`play`/`splice`, and the arrangement ops) are state; pure actions
    /// (`bounce`, the transport ops, `record`) are not.
    fn is_state(&self) -> bool {
        matches!(
            self,
            HostCommand::Mount { .. }
                | HostCommand::Patch { .. }
                | HostCommand::SetParam { .. }
                | HostCommand::SetTempo { .. }
                | HostCommand::Unmount { .. }
                | HostCommand::Arrange { .. }
                | HostCommand::Pool { .. }
                | HostCommand::Play { .. }
                | HostCommand::Splice { .. }
        )
    }
}

/// The transport position — the value a shell reads to draw the playhead and the
/// time readout. `frame` is the core clock's absolute position; musical time
/// (`beat`, `bpm`) is derived through the tempo map (the log's time-basis rule:
/// tempo edits never move stored positions).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub frame: u64,
    pub seconds: f64,
    pub beat: f64,
    pub bpm: f64,
    pub playing: bool,
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
    /// The pool directory (set by `set_pool`), used to list sources for the UI.
    pool_dir: Option<PathBuf>,
    /// The sources the last `set_pool` resampled to the session rate (see
    /// [`HostSession::pool_conformed`]).
    pool_conformed: Vec<media::Conform>,
    /// track id → the arranger node mounted for it (avoids re-wiring on a rebuild).
    wired_tracks: std::collections::HashMap<String, engine::NodeId>,
    /// the arrangement value changed since last wiring (re-wire before render).
    arrange_dirty: bool,
    /// Whether the transport is running. The live runtime reads this; the
    /// offline reference host only records it (`Bounce` renders regardless).
    playing: bool,
    /// What the last offline `Bounce` drained (tail frames + capped). The live
    /// transport hard-cuts, so this stays default until a bounce runs.
    last_drain: DrainOutcome,
    /// The **state** commands applied so far, in order — replayed by `seek_to`
    /// to rebuild the session at a target frame. Actions are not recorded.
    history: Vec<HostCommand>,
    /// The edits undone since the last edit, each with the history position it
    /// came from, so a redo reconstructs the same session. Cleared by a new edit.
    redo: Vec<(usize, HostCommand)>,
    /// The media intent value (pool dir, player, splices, bounce records). The
    /// media-op handlers rebuild it on replay, so a replayed log reproduces the
    /// media session — media determinism is in the one log, not a parallel seam.
    media: Arc<Mutex<MediaSession>>,
    /// Interns runtime media strings (paths) to `&'static str` for the log
    /// (spike scale — a serialized log would use a string table).
    media_intern: Interner,
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
        let media: Arc<Mutex<MediaSession>> = Arc::new(Mutex::new(MediaSession::default()));
        media_ops::register_handlers(&mut engine, media.clone())
            .expect("media op handlers register once on a fresh engine");
        HostSession {
            engine,
            media,
            media_intern: Interner::new(),
            pending_cords: Vec::new(),
            player_mailbox: None,
            player_underruns: None,
            player_deferred: None,
            arranger_underruns: Vec::new(),
            mixer_channels: None,
            media_commands: 0,
            editor: None,
            pool_resolver: None,
            pool_dir: None,
            pool_conformed: Vec::new(),
            wired_tracks: std::collections::HashMap::new(),
            arrange_dirty: false,
            playing: false,
            last_drain: DrainOutcome::default(),
            history: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Set the media pool for arrangement clips. The pool (float-WAV sources by
    /// stem id) is where the clip editor's clips resolve their source paths.
    ///
    /// **Adopting a pool conforms it.** A source whose rate differs from the
    /// session rate is resampled once, in place (the original kept beside it as
    /// `{id}.wav.pre{rate}`), because an arrangement clip is a straight read in
    /// the session's one frame domain — a 44.1 kHz file in a 48 kHz session is
    /// material to convert, not a reason to refuse the transport. The report is
    /// kept on the session ([`HostSession::pool_conformed`]) and carried on the
    /// [`HostOutcome`](crate::live::HostOutcome) so a shell can say what moved.
    pub fn set_pool(&mut self, pool_dir: impl Into<PathBuf>) -> Result<(), String> {
        let dir: PathBuf = pool_dir.into();
        let pool = media::Pool::open(&dir)?; // validate it exists as a directory
        // A file that cannot be converted is reported, not fatal: it stays at its
        // own rate and the arranger names it if a clip reads it.
        self.pool_conformed = pool.conform(self.engine.clock.sample_rate)?.converted;
        let resolver_dir = dir.clone();
        let resolver: media::PoolResolver = std::sync::Arc::new(move |id| {
            let p = resolver_dir.join(format!("{id}.wav"));
            p.is_file().then_some(p)
        });
        self.pool_resolver = Some(resolver);
        self.pool_dir = Some(dir);
        Ok(())
    }

    /// The sources the last [`set_pool`](Self::set_pool) brought to the session
    /// rate (empty when the pool already fitted). A shell shows this once, as a
    /// fact about the load — the pool is not converted again afterwards.
    pub fn pool_conformed(&self) -> &[media::Conform] {
        &self.pool_conformed
    }

    /// The pool source listing (id, frame count, sample rate, peaks), for the
    /// source-pool panel. `None` when no pool is set.
    pub fn pool_sources(&self) -> Option<Vec<media::PoolSource>> {
        let dir = self.pool_dir.as_ref()?;
        let pool = media::Pool::open(dir).ok()?;
        Some(pool.list().ok()?.sources)
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
            HostCommand::TransportPlay => {
                self.playing = true;
                Ok(())
            }
            HostCommand::TransportStop => {
                self.playing = false;
                Ok(())
            }
            HostCommand::TransportSeek { frame } => self.seek_to(*frame),
            HostCommand::Undo => self.undo().map(|_| ()),
            HostCommand::Redo => self.redo().map(|_| ()),
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
                let intent = PlayerIntent {
                    path: clip.path.to_string_lossy().into_owned(),
                    start: clip.start,
                    len: clip.len,
                    channel: *channel,
                };
                // Open + warm before logging, so a bad path never enters the log
                // (a refused command is never logged).
                let player = FilePlayer::start(clip, DEFAULT_RING_CAPACITY)?;
                Self::warm_player(&player, DEFAULT_RING_CAPACITY)?; // deterministic, no race
                // Logged as a media op; `arrange_logged` never schedules, so the
                // live path applies once and replay reconstructs the value.
                let (op, fields) = media_ops::encode_play(&mut self.media_intern, &intent);
                self.engine.arrange_logged(op, fields)?;
                // Commit (infallible after the validation above). Place the player
                // BEFORE the mixer in topological order (the graph's forward-order
                // rule): if the mixer is already materialized, insert_before(mixer);
                // otherwise add (append) and the mixer materializes later and lands
                // after the player. A player appended after a materialized mixer
                // would make its cord backward — every later render fails.
                let mailbox = media::mailbox();
                let node = PlaybackNode::new(Some(player), mailbox.clone());
                let underruns = node.underrun_counter();
                let deferred = node.deferred_counter();
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
                self.media.lock().map_err(|_| "media session poisoned")?.player = Some(intent);
                self.media_commands += 1;
                Ok(())
            }
            HostCommand::Splice { at_frame, clip, crossfade } => {
                if self.player_mailbox.is_none() {
                    return Err("splice requires a playing clip".into());
                }
                let clip = Self::resolve_clip(clip)?;
                let intent = SpliceIntent {
                    at_frame: *at_frame,
                    path: clip.path.to_string_lossy().into_owned(),
                    start: clip.start,
                    len: clip.len,
                    crossfade: *crossfade,
                };
                let incoming = FilePlayer::start(clip, DEFAULT_RING_CAPACITY)?;
                Self::warm_player(&incoming, DEFAULT_RING_CAPACITY)?;
                let (op, fields) = media_ops::encode_splice(&mut self.media_intern, &intent);
                self.engine.arrange_logged(op, fields)?;
                let mailbox = self.player_mailbox.as_ref().expect("checked above");
                mailbox.lock().map_err(|_| "player mailbox poisoned")?.push_back(SpliceCmd {
                    at_frame: *at_frame,
                    incoming,
                    crossfade: *crossfade,
                });
                self.media.lock().map_err(|_| "media session poisoned")?.splices.push(intent);
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
                // Validate + resolve, then log (a refused pool is never logged).
                self.set_pool(dir.clone())?;
                let (op, fields) = media_ops::encode_pool(&mut self.media_intern, &dir.to_string_lossy());
                self.engine.arrange_logged(op, fields)?;
                self.media.lock().map_err(|_| "media session poisoned")?.pool_dir = Some(dir.clone());
                self.media_commands += 1;
                Ok(())
            }
            HostCommand::Bounce { frames, path } => {
                // Offline bounce: render + drain buffered tails. A capped drain
                // is a *different, truncated* piece, so it fails loud rather than
                // writing a quietly shortened file.
                let (out, drain) = self.render_with_drain(*frames)?;
                if drain.capped {
                    return Err(format!(
                        "bounce drain hit the {MAX_DRAIN_FRAMES}-frame bound with output still pending — the tail is truncated"
                    ));
                }
                let channels = self.engine.graph.out_channels().max(1) as u16;
                let mut w = media::WavWriter::create(path, self.engine.clock.sample_rate, channels)?;
                w.write(&out)?;
                w.finalize()?;
                // Record the bounce in the log (frames, path, the drain policy's
                // outcome) — the drain note's "the bounce event records the policy".
                // A bounce writes a file, so it is a *record*, not replayed state.
                let record = BounceRecord {
                    frames: *frames as u64,
                    drained_frames: drain.tail_frames,
                    capped: drain.capped,
                };
                let (op, fields) = media_ops::encode_bounce(&record);
                self.engine.arrange_logged(op, fields)?;
                self.media.lock().map_err(|_| "media session poisoned")?.bounces.push(record);
                self.last_drain = drain;
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
        Self::check_bounce_budget(frames)?;
        self.wire_pending()?;
        self.wire_arranger()?;
        Ok(self.engine.render(frames))
    }

    /// Like [`render`](Self::render), but drain buffered tails after the timeline
    /// — the **offline bounce** shape. The live transport uses `render` (a hard
    /// cut): a stopped device is paused, so there is nowhere for a tail to ring.
    pub fn render_with_drain(&mut self, frames: usize) -> Result<(Vec<f32>, DrainOutcome), String> {
        // The drain can add up to MAX_DRAIN_FRAMES on top of `frames`.
        Self::check_bounce_budget(frames.saturating_add(MAX_DRAIN_FRAMES))?;
        self.wire_pending()?;
        self.wire_arranger()?;
        Ok(self
            .engine
            .render_with_drain(frames, DrainPolicy::Tails, MAX_DRAIN_FRAMES))
    }

    /// The offline bounce budget: the master may be stereo (L/R), so a frame
    /// costs up to 2 * 4 bytes; a malformed `bounce` must not OOM.
    fn check_bounce_budget(frames: usize) -> Result<(), String> {
        let budget = frames
            .saturating_mul(std::mem::size_of::<f32>())
            .saturating_mul(2);
        if budget > Self::MAX_BOUNCE_BYTES {
            return Err(format!("bounce of {frames} frames exceeds the ~{:.0} MiB budget", Self::MAX_BOUNCE_BYTES / (1 << 20)));
        }
        Ok(())
    }

    pub fn log(&self) -> &SessionLog {
        &self.engine.log
    }

    pub fn event_count(&self) -> usize {
        self.engine.log.len()
    }

    /// The session's current parameter values, **folded from the log**.
    ///
    /// The log is the command list (minimal-core note §3: *model-visible means
    /// logged*), so the current value of a parameter is simply the last thing
    /// logged for it — no second store that a shell has to keep in sync with the
    /// engine. `Mount` contributes its initial params; `SetParam` overwrites;
    /// `ScheduleUnmount` drops a plugin's params (a later mount re-adds them).
    ///
    /// Order is first-appearance, which is stable across replays, so a shell can
    /// diff two reads to see what changed.
    pub fn params(&self) -> Vec<(&'static str, &'static str, f32)> {
        let mut values: Vec<(&'static str, &'static str, f32)> = Vec::new();

        fn set(
            values: &mut Vec<(&'static str, &'static str, f32)>,
            plugin: &'static str,
            param: &'static str,
            value: f32,
        ) {
            match values
                .iter_mut()
                .find(|(p, n, _)| *p == plugin && *n == param)
            {
                Some(entry) => entry.2 = value,
                None => values.push((plugin, param, value)),
            }
        }

        for event in self.log().events() {
            match event {
                Event::Mount { plugin, params, .. } => {
                    for (param, value) in params {
                        set(&mut values, plugin, param, *value);
                    }
                }
                Event::SetParam {
                    plugin,
                    param,
                    value,
                    ..
                } => set(&mut values, plugin, param, *value),
                Event::ScheduleUnmount { plugin, .. } => {
                    values.retain(|(p, _, _)| p != plugin);
                }
                _ => {}
            }
        }

        values
    }

    pub fn media_command_count(&self) -> usize {
        self.media_commands
    }

    /// What the last offline `Bounce` drained: tail frames emitted, and whether
    /// the bound was hit. The live transport hard-cuts, so this is default until
    /// a bounce runs.
    pub fn last_drain(&self) -> DrainOutcome {
        self.last_drain
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

    /// Whether the transport is running.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// The session sample rate (the live pump paces against it).
    pub fn sample_rate(&self) -> u32 {
        self.engine.clock.sample_rate
    }

    /// The master bus channel count (1 mono until the mixer mounts, then 2). The
    /// live pump converts the rendered master to the output ring's stereo layout.
    pub fn master_channels(&self) -> usize {
        self.engine.graph.out_channels().max(1)
    }

    /// The mixer's mounted channel count, if a mixer is mounted (what the shell
    /// draws strips for).
    pub fn mixer_channels(&self) -> Option<usize> {
        self.mixer_channels
    }

    /// The transport position (frame + derived musical time + playing). A shell
    /// polls this to draw the playhead; reading it never renders.
    pub fn position(&self) -> Position {
        let frame = self.engine.clock.frame();
        Position {
            frame,
            seconds: self.engine.clock.seconds(),
            beat: self.engine.clock.beat(),
            bpm: self.engine.clock.tempo_map.tempo_at(frame),
            playing: self.playing,
        }
    }
}

impl Default for HostSession {
    fn default() -> Self {
        Self::new()
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

    /// Render forward to an absolute frame in chunks, so a long seek does not
    /// trip the single-bounce budget. A no-op when already at or past `frame`.
    pub fn render_to(&mut self, frame: u64) -> Result<(), String> {
        const CHUNK_FRAMES: u64 = 48_000; // 1 s at 48 kHz
        while self.engine.clock.frame() < frame {
            let want = (frame - self.engine.clock.frame()).min(CHUNK_FRAMES) as usize;
            self.render(want)?;
        }
        Ok(())
    }

    /// Seek to an absolute frame by **rebuilding** the session: replay the
    /// recorded state commands onto a fresh session (the same deterministic path
    /// `run_script` uses) and render forward to the target. v1 semantics —
    /// correct by construction, O(target), because the core clock only advances
    /// by rendering. The transport's playing state is preserved; a refused
    /// replay leaves the original session untouched.
    fn seek_to(&mut self, frame: u64) -> Result<(), String> {
        self.replay_to(frame)
    }

    /// Rebuild the session from its state-command history and render back to
    /// `frame` — the deterministic reconstruction `seek_to` and undo/redo share.
    /// The transport's playing state and the redo stack survive the rebuild.
    fn replay_to(&mut self, frame: u64) -> Result<(), String> {
        let history = self.history.clone();
        let redo = std::mem::take(&mut self.redo);
        let playing = self.playing;
        let mut rebuilt = HostSession::new();
        for cmd in &history {
            // State that takes effect *after* the target is not yet in force at
            // `frame`. Applying it would render the clock past the target — which
            // makes a backward seek a no-op, because `process` renders up to the
            // command's `at_frame`. Skip it: the rebuild reconstructs the session
            // as it was at `frame`.
            if cmd.at_frame().is_some_and(|at| at > frame) {
                continue;
            }
            rebuilt.process(cmd)?;
        }
        rebuilt.render_to(frame)?;
        rebuilt.playing = playing;
        rebuilt.redo = redo;
        *self = rebuilt;
        Ok(())
    }

    /// Undo the most recent **arrangement** edit: drop it from the state history
    /// and rebuild **to the current position**, so the playhead does not jump and
    /// the pool/mounts survive. `Ok(false)` when there is nothing to undo.
    ///
    /// Only `Arrange` ops are undoable — a `Mount`/`Pool`/`SetTempo` is session
    /// setup, not an edit, and undoing one would tear down the graph under the UI.
    pub fn undo(&mut self) -> Result<bool, String> {
        let Some(pos) = self
            .history
            .iter()
            .rposition(|c| matches!(c, HostCommand::Arrange { .. }))
        else {
            return Ok(false);
        };
        let undone = self.history.remove(pos);
        self.redo.push((pos, undone));
        self.replay_to(self.engine.clock.frame())?;
        Ok(true)
    }

    /// Redo the most recently undone edit, re-inserted at its original history
    /// position so the reconstruction is faithful. `Ok(false)` when nothing is
    /// undone.
    pub fn redo(&mut self) -> Result<bool, String> {
        let Some((pos, cmd)) = self.redo.pop() else {
            return Ok(false);
        };
        let at = pos.min(self.history.len());
        self.history.insert(at, cmd);
        self.replay_to(self.engine.clock.frame())?;
        Ok(true)
    }

    /// Whether there is an arrangement edit to undo — the shell's `⟲` button.
    pub fn can_undo(&self) -> bool {
        self.history.iter().any(|c| matches!(c, HostCommand::Arrange { .. }))
    }

    /// Whether there is an undone edit to redo — the shell's `⟳` button.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
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
            // Record state commands so a later seek can rebuild the session
            // deterministically (actions are not state). A new state change also
            // makes any undone branch unreachable, so the redo stack clears.
            if cmd.is_state() {
                self.history.push(cmd.clone());
                self.redo.clear();
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
/// transport play                              # live transport (state only here)
/// transport seek 96000                        # rebuild + render to the frame
/// transport stop
/// play /abs/clip.wav ch0 @0
/// splice 4000 /abs/clip2.wav 512
/// pool /data/takes                           # the media pool dir (adopting it
///                                             # resamples foreign-rate sources)
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
            "transport" => {
                let what = word(&words, 1, at)?;
                match what {
                    "play" => commands.push(HostCommand::TransportPlay),
                    "stop" => commands.push(HostCommand::TransportStop),
                    "seek" => {
                        let frame = word(&words, 2, at)?
                            .parse::<u64>()
                            .map_err(|_| format!("line {at}: transport seek needs a frame"))?;
                        commands.push(HostCommand::TransportSeek { frame });
                    }
                    other => {
                        return Err(format!(
                            "line {at}: unknown transport '{other}' (want play|stop|seek)"
                        ))
                    }
                }
            }
            "undo" => commands.push(HostCommand::Undo),
            "redo" => commands.push(HostCommand::Redo),
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

/// Parse a single `arrange …` command line — the wire form a live UI sends per
/// edit — returning the op and its optional `@frame`. It reuses the script
/// grammar ([`parse_arrange`]), so the text format stays the *one* op grammar
/// (no second schema to drift against).
///
/// ```text
/// arrange move_clip t0 c0 96000 @0
/// arrange trim t0 c0 end -24000
/// arrange razor_split t0 c0 cL cR 48000
/// trim t0 c0 start 4800          # the `arrange` keyword is optional
/// ```
pub fn parse_arrange_line(line: &str) -> Result<(media::ArrangeOp, Option<u64>), String> {
    let line = line.split('#').next().unwrap_or("").trim();
    if line.is_empty() {
        return Err("empty arrange line".into());
    }
    let mut words: Vec<&str> = line.split_whitespace().collect();
    let at_frame = match words.last() {
        Some(tok) if tok.starts_with('@') => {
            let f = tok[1..]
                .parse::<u64>()
                .map_err(|_| format!("bad frame '{tok}'"))?;
            words.pop();
            Some(f)
        }
        _ => None,
    };
    let operands = if words.first() == Some(&"arrange") { &words[1..] } else { &words[..] };
    let op = parse_arrange(operands, 1)?;
    Ok((op, at_frame))
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
    let drain = session.last_drain();
    format!(
        "engine log events: {}\nmedia commands: {}\nunderruns: {}\ndeferred splices: {}\nmaster out node: {:?}\ndrain tail frames: {}{}\n",
        session.event_count(),
        session.media_command_count(),
        session.underruns(),
        session.deferred(),
        session.engine_ref().graph.out_node,
        drain.tail_frames,
        if drain.capped { " (CAPPED)" } else { "" },
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

    /// Transport play/stop toggles the playing flag; the offline host records
    /// the state (a live runtime pumps; the reference host renders via Bounce).
    #[test]
    fn transport_play_stop_toggles_playing() {
        let mut s = HostSession::new();
        assert!(!s.is_playing(), "a fresh session is stopped");
        assert_eq!(s.position().frame, 0);
        s.execute(&HostCommand::TransportPlay).expect("play");
        assert!(s.is_playing());
        s.execute(&HostCommand::TransportStop).expect("stop");
        assert!(!s.is_playing());
        assert!(!s.position().playing);
    }

    /// Forward seek renders to the target frame (the core clock only advances by
    /// rendering) and the position reports the tempo-derived musical time.
    #[test]
    fn transport_seek_forward_renders_to_target() {
        let mut s = HostSession::new();
        s.execute(&HostCommand::TransportSeek { frame: 4_800 }).expect("seek");
        let p = s.position();
        assert_eq!(p.frame, 4_800, "seek forward renders to the target");
        assert!((p.seconds - 0.1).abs() < 1e-9, "4800 frames @48k = 0.1 s");
        assert!((p.beat - 0.2).abs() < 1e-6, "120 bpm, 0.1 s = 0.2 beats");
        assert!((p.bpm - 120.0).abs() < 1e-9, "the tempo map reports 120 bpm");
    }

    /// Backward seek rebuilds from the state-command history (the core clock
    /// cannot run backwards) and the arrangement value survives the rebuild.
    #[test]
    fn transport_seek_backward_rebuilds_and_preserves_state() {
        let pool = std::env::temp_dir().join(format!("host-seek-pool-{}", std::process::id()));
        std::fs::create_dir_all(&pool).expect("pool dir");
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mount mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() }).expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: Some(0),
        })
        .expect("add track");

        s.execute(&HostCommand::TransportSeek { frame: 9_600 }).expect("seek forward");
        assert_eq!(s.position().frame, 9_600);
        s.execute(&HostCommand::TransportSeek { frame: 1_000 }).expect("seek backward");
        assert_eq!(s.position().frame, 1_000, "backward seek rebuilds to the target");
        assert_eq!(
            s.arrangement().expect("arrangement").tracks.len(),
            1,
            "the arrangement state survives the rebuild"
        );
        let _ = std::fs::remove_dir_all(&pool);
    }

    /// A single `arrange …` line parses to its op + optional frame; the `arrange`
    /// keyword is optional (only the operands are required).
    #[test]
    fn parse_arrange_line_takes_the_op_and_an_optional_frame() {
        let (op, at) = parse_arrange_line("arrange move_clip t0 c0 9600 @48000").expect("parse");
        assert!(matches!(op, media::ArrangeOp::MoveClip { at_frame: 9_600, .. }));
        assert_eq!(at, Some(48_000));

        let (op, at) = parse_arrange_line("trim t0 c0 end -24000").expect("parse without the keyword");
        assert!(matches!(op, media::ArrangeOp::Trim { by_frames: -24_000, .. }));
        assert_eq!(at, None);

        assert!(parse_arrange_line("").is_err(), "an empty line is refused");
        assert!(parse_arrange_line("arrange nonsense t0").is_err(), "an unknown op is refused");
    }

    // ---- undo/redo fixtures ----
    /// A short mono float-WAV take, so an arrangement can actually wire a reader
    /// (a render — which `seek_to`/undo do — opens every clip's source).
    fn write_take(dir: &std::path::Path, id: &str, frames: usize) {
        let path = dir.join(format!("{id}.wav"));
        let mut w = media::WavWriter::create(&path, 48_000, 1).expect("wav writer");
        w.write(&vec![0.0f32; frames]).expect("write take");
        w.finalize().expect("finalize take");
    }

    fn add_clip(id: &str, at: u64) -> media::ArrangeOp {
        media::ArrangeOp::AddClip {
            track: "t0".into(),
            clip: media::Clip {
                id: id.into(),
                source: "s1".into(),
                src_start: 0,
                src_len: 4_800,
                at_frame: at,
                fade_in: 0,
                fade_out: 0,
                gain: 1.0,
                loop_len: None,
            },
        }
    }

    fn move_clip(id: &str, at: u64) -> media::ArrangeOp {
        media::ArrangeOp::MoveClip { track: "t0".into(), clip: id.into(), at_frame: at }
    }

    /// Undo drops the most recent arrangement edit and rebuilds **to the current
    /// position**; redo re-applies it; a new edit clears the redo branch.
    #[test]
    fn undo_and_redo_revert_and_reapply_an_edit() {
        let pool = std::env::temp_dir().join(format!("host-undo-pool-{}", std::process::id()));
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_take(&pool, "s1", 48_000);
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mount mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() }).expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("add track");
        s.execute(&HostCommand::Arrange { op: add_clip("c0", 0), at_frame: None })
            .expect("add clip");

        assert!(s.can_undo(), "edits are undoable");
        assert!(!s.can_redo(), "nothing is undone yet");

        // Play to 9600 so we can prove an undo does not rewind the playhead.
        s.execute(&HostCommand::TransportSeek { frame: 9_600 }).expect("seek");
        s.execute(&HostCommand::Arrange { op: move_clip("c0", 4_800), at_frame: None })
            .expect("move");
        assert_eq!(s.arrangement().expect("tl").tracks[0].clips[0].at_frame, 4_800);

        assert!(s.undo().expect("undo works"), "undo reports an edit was undone");
        assert_eq!(
            s.arrangement().expect("tl").tracks[0].clips[0].at_frame,
            0,
            "the move is reverted"
        );
        assert_eq!(s.position().frame, 9_600, "undo keeps the playhead where it was");
        assert!(s.can_redo(), "the undone edit can be redone");

        assert!(s.redo().expect("redo works"));
        assert_eq!(
            s.arrangement().expect("tl").tracks[0].clips[0].at_frame,
            4_800,
            "redo re-applies the move"
        );
        assert!(!s.can_redo(), "the redo branch is consumed");

        // A new edit discards the redo branch.
        s.undo().expect("undo again");
        assert!(s.can_redo());
        s.execute(&HostCommand::Arrange { op: move_clip("c0", 20_000), at_frame: None })
            .expect("a new edit");
        assert!(!s.can_redo(), "a new edit clears the redo branch");

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// Nothing to undo is a clean no-op — and session setup is not an edit.
    #[test]
    fn undo_with_nothing_to_undo_is_a_noop() {
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mount mixer");
        assert!(!s.can_undo(), "a mount is session setup, not an edit");
        assert!(!s.undo().expect("undo is a no-op"), "nothing was undone");
        assert!(!s.redo().expect("redo is a no-op"), "nothing was redone");
        assert_eq!(s.position().frame, 0);
    }

    /// The undo / redo grammar lines parse.
    #[test]
    fn undo_and_redo_lines_parse() {
        let cmds = parse_script("host v1\nundo\nredo\n").expect("parse");
        assert!(matches!(cmds[0], HostCommand::Undo));
        assert!(matches!(cmds[1], HostCommand::Redo));
    }

    /// The transport text grammar parses play / seek / stop.
    #[test]
    fn transport_lines_parse() {        let cmds = parse_script("host v1\ntransport play\ntransport seek 4800\ntransport stop\n")
            .expect("transport lines parse");
        assert!(matches!(&cmds[0], HostCommand::TransportPlay));
        assert!(matches!(&cmds[2], HostCommand::TransportStop));
        match &cmds[1] {
            HostCommand::TransportSeek { frame } => assert_eq!(*frame, 4_800),
            other => panic!("expected a seek, got {other:?}"),
        }
    }
}
