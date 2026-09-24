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
use media::{
    ClipRef, DEFAULT_RING_CAPACITY, FilePlayer, Interner, Mailbox, PlaybackNode, SpliceCmd,
};

pub mod live;
pub mod media_ops;

/// The session's tempo/meter map — re-exported because a shell reads it off the
/// snapshot to do its own beat-domain math (grid snapping, a bar/beat ruler), and
/// a public field needs a nameable type.
pub use engine::TempoMap;

use media_ops::{BounceRecord, MediaSession, PlayerIntent, SpliceIntent};

/// The Host API contract version. The text format's first line must be
/// `host v{N}`; mismatches are refused (kimi review finding 6).
pub const HOST_API_VERSION: u32 = 1;

/// The host registry: plugins, then ports, then parameters — validated per
/// slot by the parser (a port name is not a plugin name).
pub const HOST_PLUGINS: &[&str] = &["euclidean", "scale", "tone", "mixer", "master"];

/// The largest stretch ratio operand (`num` or `den`) the host will render. A tempo
/// match lives well inside this (a 10:1 ratio is already absurd); beyond it the ratio is
/// a typo or an attack, and the render allocates its output from it.
pub const MAX_STRETCH_RATIO: u32 = 1_000;
/// The longest stretch render the host will materialise, in frames — two hours at
/// 48 kHz. Bounded because the render allocates its whole output before writing it.
pub const MAX_STRETCH_FRAMES: u64 = 2 * 60 * 60 * 48_000;

pub const HOST_PORTS: &[&str] = &[
    "triggers", "trigger", "note", "audio", "ch0", "ch1", "ch2", "ch3", "ch4", "ch5", "ch6", "ch7",
];
pub const HOST_PARAMS: &[&str] = &[
    "steps",
    "pulses",
    "rotation",
    "pulses_per_beat",
    "root",
    "note_len",
    "gain",
    "blip_len",
    "channels",
    "master.gain",
    "ch0.gain",
    "ch0.mute",
    "ch0.solo",
    "ch0.pan",
    "ch1.gain",
    "ch1.mute",
    "ch1.solo",
    "ch1.pan",
    "ch2.gain",
    "ch2.mute",
    "ch2.solo",
    "ch2.pan",
    "ch3.gain",
    "ch3.mute",
    "ch3.solo",
    "ch3.pan",
    "ch4.gain",
    "ch4.mute",
    "ch4.solo",
    "ch4.pan",
    "ch5.gain",
    "ch5.mute",
    "ch5.solo",
    "ch5.pan",
    "ch6.gain",
    "ch6.mute",
    "ch6.solo",
    "ch6.pan",
    "ch7.gain",
    "ch7.mute",
    "ch7.solo",
    "ch7.pan",
    // The mastering stage's compressor + limiter (`master.<param>` would collide
    // with the mixer's own `master.gain` fader, so these are the chain's names).
    "threshold",
    "ratio",
    "attack_ms",
    "release_ms",
    "makeup",
    "ceiling",
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
#[derive(Debug, Clone, PartialEq)]
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
    /// **Record a take** from the default input device into the session's pool:
    /// `{take_id}.ch{k}` sources land beside the other pool material, at the session
    /// rate (the device's clock is drift-compensated), ready to be placed on a track.
    /// An **action**, not state: what the log records is the *clip* you make from the
    /// take, not the recording session.
    Record { take_id: String },
    /// Stop the take in progress and finalize it (headers, peaks). A no-op-looking
    /// error when nothing is recording — never a silent half-take.
    RecordStop,
    /// **A gesture**: several arrangement ops that apply as one unit and undo as
    /// **one step**. Members must be arrangement ops sharing one `at_frame` (a
    /// gesture happens at one moment, which is what makes a replay's
    /// skip-after-target rule safe to apply to the whole group).
    ///
    /// The group applies **all-or-nothing**: the whole thing is folded over a
    /// snapshot of the arrangement value first, so a member the model would refuse
    /// means no member is applied and nothing is logged. The engine's own log still
    /// receives each member, so replay and the byte-identical bounce tests are
    /// unaffected by construction — only the *host history* (and therefore
    /// undo/redo, `can_undo` and a saved session) sees one entry.
    Group { commands: Vec<HostCommand> },
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
    Pool { dir: PathBuf },
    /// The session's sample rate. **Context, not an edit**: a session's rate is
    /// fixed when it is created (`HostSession::new_at`), so this line belongs at the
    /// top of a saved session and a live session refuses a *different* rate rather
    /// than pretending to change. It is state, so a load and a replay honour it.
    SessionRate { hz: u32 },
    /// Write the session as a directory: `session.txt` (this log, in the `host v1`
    /// text form) + `pool/`. An **action** (like `Bounce`), not state.
    Save { dir: PathBuf },
    /// Load a session directory written by [`HostCommand::Save`]: the script, then
    /// its journal (a torn trailing line dropped, and reported). An action.
    Load { dir: PathBuf },
    /// **Time-stretch a clip into new pool material.** An *action* (like `record`):
    /// the render writes a new pool source and the logged `ArrangeOp::Stretch`
    /// rewrites the clip's reference, so the arrangement value never gains a
    /// playback-rate property and a replay re-derives the same value without
    /// re-rendering. `num/den` is the ratio as a rational (`output/input`), so the
    /// render is byte-reproducible. Refused for a looped clip, a 1:1 ratio, a region
    /// shorter than the transform's window, and a session with no pool.
    Stretch {
        track: String,
        clip: String,
        num: u32,
        den: u32,
    },
    /// Record the tempo a pool source was performed at (`source_tempo <id> <bpm>`), so
    /// **tempo match** has a ratio to derive. State: the log carries it, a replay
    /// rebuilds it, and it is exposed on the outcome for a shell to read.
    SetSourceTempo { source: String, bpm: f64 },
    /// Render `frames` from the current position and write the master to a
    /// 16-bit WAV.
    Bounce { frames: usize, path: PathBuf },
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
            // A group's members share one frame (validated on the way in), so the
            // group's frame is the first member's — the whole gesture is skipped or
            // applied together by `replay_to`.
            HostCommand::Group { commands } => commands.first().and_then(|c| c.at_frame()),
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
                | HostCommand::Group { .. }
                | HostCommand::SessionRate { .. }
                | HostCommand::SetSourceTempo { .. }
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
    /// The tempo each pool source was performed at (`source_tempo <id> <bpm>`), which
    /// is what **tempo match** derives its ratio from. State: the log carries it, a
    /// replay rebuilds it, and the outcome exposes it to a shell.
    source_tempos: std::collections::HashMap<String, f64>,
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
    /// The **state** commands applied so far, in order — replayed by `seek_to` to
    /// rebuild the session at a target frame. Actions are not recorded.
    ///
    /// One entry is **one gesture**: a bare state command is a one-element entry,
    /// and a [`HostCommand::Group`] pushes its members as a single entry. Undo and
    /// redo move whole entries, so a gesture that takes several ops (a paste, a
    /// trim-to-selection, a stretch) is one undo step.
    history: Vec<Vec<HostCommand>>,
    /// The gestures undone since the last edit, each with the history position it
    /// came from, so a redo reconstructs the same session. Cleared by a new edit.
    redo: Vec<(usize, Vec<HostCommand>)>,
    /// The take in progress, if any (see [`HostSession::record`]). The device stream
    /// lives here because a cpal `Stream` is `!Send` and the session never leaves the
    /// thread that opened it.
    recording: Option<Recording>,
    /// The last finished take, for the shell to report once.
    last_take: Option<TakeReport>,
    /// The session directory, when the session has one (`Save`/`Load`): the journal
    /// — the autosave — is appended here.
    session_dir: Option<PathBuf>,
    /// The last journal write failure. A failed *append* never fails the edit (the
    /// edit is already applied and logged), but it must not be silent.
    journal_error: Option<String>,
    /// What the last `Load` recovered from the journal.
    last_recovery: Option<JournalRecovery>,
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
        Self::new_at(DEFAULT_SAMPLE_RATE)
    }

    /// Create a session at `rate` — the constructor a saved session's
    /// `session_rate` line selects (see [`HostSession::from_script`]). The rate is
    /// **context**: it is fixed here, because the clock, every frame in the log and
    /// the device negotiation all derive from it.
    pub fn new_at(rate: u32) -> Self {
        let mut engine = Engine::new(rate, 120.0, 4);
        engine.register_factory(
            "euclidean",
            plugins::euclidean_factory,
            plugins::euclidean::EUCLIDEAN_PORTS,
            &[],
        );
        engine.register_factory(
            "scale",
            plugins::scale_factory,
            plugins::scale::SCALE_PORTS,
            &[],
        );
        engine.register_factory(
            "tone",
            plugins::tone_factory,
            plugins::tone::TONE_PORTS,
            plugins::tone::TONE_PARAMS,
        );
        engine.register_factory(
            "mixer",
            plugins::mixer_factory,
            plugins::mixer::MIXER_PORTS,
            plugins::mixer::MIXER_PARAMS,
        );
        // The mastering stage: mounted *after* the mixer, patched from it, and
        // claiming the output (a stereo cord — the mixer's bus is interleaved).
        engine.register_factory(
            "master",
            plugins::master_factory,
            plugins::master::MASTER_PORTS,
            plugins::master::MASTER_PARAMS,
        );
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
            source_tempos: std::collections::HashMap::new(),
            wired_tracks: std::collections::HashMap::new(),
            arrange_dirty: false,
            playing: false,
            last_drain: DrainOutcome::default(),
            history: Vec::new(),
            redo: Vec::new(),
            recording: None,
            last_take: None,
            session_dir: None,
            journal_error: None,
            last_recovery: None,
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

    /// Apply a **gesture** ([`HostCommand::Group`]): validate the whole thing on a
    /// snapshot of the arrangement value, then apply and log each member.
    ///
    /// All-or-nothing is the point: a paste that would collide with an existing
    /// clip id, or a trim sequence whose second edge would empty the clip, must
    /// leave the session exactly as it was — not half-edited, and with nothing
    /// logged. Members are arrangement ops sharing one frame, so the group is one
    /// moment in time and `replay_to` can skip or apply it whole.
    fn execute_group(&mut self, commands: &[HostCommand]) -> Result<(), String> {
        if commands.is_empty() {
            return Ok(());
        }
        self.ensure_editor()?;
        if self.pool_resolver.is_none() {
            return Err("arrange requires set_pool first (clips need pool-source paths)".into());
        }

        let mut frame: Option<Option<u64>> = None;
        for cmd in commands {
            let HostCommand::Arrange { at_frame, .. } = cmd else {
                return Err(
                    "a group may only contain arrangement ops (a gesture is one edit at one moment)"
                        .into(),
                );
            };
            match frame {
                None => frame = Some(*at_frame),
                Some(seen) if seen == *at_frame => {}
                Some(_) => return Err("a group's arrangement ops must share one frame".into()),
            }
        }

        // Fold over a snapshot first: the value is a pure transform, so the fold and
        // the real apply agree **as long as the real apply cannot fail after the fold**.
        // That holds today because the only fallible step of `arrange_logged` is a
        // validation the timeline has already performed (the op's gains must be
        // finite). A future op with a field the timeline does *not* validate would
        // reopen this window — recorded in the note, not just here.
        let mut probe = self
            .editor
            .as_ref()
            .ok_or("clip editor not initialized")?
            .snapshot()?;
        for cmd in commands {
            let HostCommand::Arrange { op, .. } = cmd else {
                unreachable!("members were checked above");
            };
            probe = probe
                .apply(op)
                .map_err(|e| format!("gesture refused, nothing applied: {e}"))?;
        }

        let editor = self.editor.as_mut().ok_or("clip editor not initialized")?;
        for cmd in commands {
            let HostCommand::Arrange { op, .. } = cmd else {
                unreachable!("members were checked above");
            };
            editor.apply(&mut self.engine, op)?;
        }
        self.arrange_dirty = true;
        self.media_commands += commands.len();
        Ok(())
    }

    // -- recording: the input device becomes pool material -------------------

    /// Start a take from an already-open source ring — the seam that makes recording
    /// testable without a device (see [`Self::record`] for the device path).
    ///
    /// The take is written by `media::Capture` as one mono float WAV per input channel
    /// (`{take_id}.ch{k}.wav`) plus a `.peaks` sidecar each, **at the session rate**:
    /// the device clock is drift-compensated into session frames, so the material is
    /// immediately arrangeable. The caller owns `source` and pushes interleaved
    /// frames into it; `input_rate` is the clock that ring runs at.
    pub fn start_recording(
        &mut self,
        take_id: &str,
        source: std::sync::Arc<media::Spsc<f32>>,
        input_rate: u32,
        channels: usize,
    ) -> Result<(), String> {
        if self.recording.is_some() {
            return Err("a take is already recording — run `record stop` first".into());
        }
        let pool = self
            .pool_dir
            .clone()
            .ok_or("record requires set_pool first (the take lands in the pool)")?;
        if self.engine.clock.sample_rate == 0 {
            return Err("the session has no sample rate".into());
        }
        // A take id is pool material: re-recording over it would silently change the
        // content of every clip that already references `{take_id}.ch{k}`. Refuse, and
        // say which file is in the way.
        let existing = pool.join(format!("{take_id}.ch0.wav"));
        if existing.is_file() {
            return Err(format!(
                "the pool already has a take '{take_id}' ({}) — record under another id, or remove it",
                existing.display()
            ));
        }
        let capture = media::Capture::start(
            &pool,
            take_id,
            channels,
            self.engine.clock.sample_rate,
            input_rate,
            source,
        )?;
        let sources = (0..channels).map(|k| format!("{take_id}.ch{k}")).collect();
        self.last_take = None;
        self.recording = Some(Recording {
            handle: None,
            capture,
            take_id: take_id.to_string(),
            sources,
        });
        Ok(())
    }

    /// Start a take from the **default input device** — the `record <take_id>` line.
    ///
    /// The device is opened at its own default config (its channel count and rate), and
    /// the take lands in the session's pool at the session rate. Recording is
    /// independent of the transport: the take is not aligned to the playhead (that is
    /// the jam layer's fixed-offset problem), it is material for the arrangement.
    pub fn record(&mut self, take_id: &str) -> Result<(), String> {
        // Fail before opening anything if the take could not be written anyway.
        if self.recording.is_some() {
            return Err("a take is already recording — run `record stop` first".into());
        }
        if self.pool_dir.is_none() {
            return Err("record requires set_pool first (the take lands in the pool)".into());
        }
        let ring = std::sync::Arc::new(media::Spsc::new(1 << 16));
        // One call: the handle carries the **stream's** rate and channel count, so the
        // capture cannot be sized from a config that raced a device swap.
        let handle = media::devices::open_input(std::sync::Arc::clone(&ring))?;
        let rate = handle.sample_rate;
        let channels = (handle.channels as usize).clamp(1, 8);
        self.start_recording(take_id, ring, rate, channels)?;
        if let Some(rec) = self.recording.as_mut() {
            rec.handle = Some(handle);
        }
        Ok(())
    }

    /// Stop the take in progress: drain, finalize the WAV headers, write the peaks
    /// sidecars, and close the device. The takes are pool sources from here on.
    pub fn stop_recording(&mut self) -> Result<TakeReport, String> {
        let Some(rec) = self.recording.take() else {
            return Err("no take is recording".into());
        };
        let Recording {
            handle,
            capture,
            take_id,
            sources,
        } = rec;
        // Stop feeding first, then drain and finalize what the device already delivered.
        drop(handle);
        // Build the report **before** the stop's result is propagated: a take that
        // finalizes with a complaint (a partial tail frame, a peaks write) has still
        // landed in the pool, and the shell must be able to place clips from it.
        let stop = capture.stop();
        let report = TakeReport {
            take_id,
            frames: capture.frames(),
            dropped: capture.dropped(),
            channels: capture.channels(),
            sources,
            sample_rate: self.engine.clock.sample_rate,
        };
        self.last_take = Some(report.clone());
        match stop {
            Ok(()) => Ok(report),
            Err(e) => Err(format!(
                "the take '{}' was finalized ({} frames, {} ch) but stopping reported: {e}",
                report.take_id, report.frames, report.channels
            )),
        }
    }

    /// The take in progress, for a live "recording" indicator.
    pub fn recording(&self) -> Option<RecordingStatus> {
        self.recording.as_ref().map(|rec| RecordingStatus {
            take_id: rec.take_id.clone(),
            frames: rec.capture.frames(),
            dropped: rec.capture.dropped(),
            channels: rec.capture.channels(),
        })
    }

    /// The last finished take, until the next one starts.
    pub fn last_take(&self) -> Option<&TakeReport> {
        self.last_take.as_ref()
    }

    /// Keep the finished take for the outcome (the shell reports it once).
    fn status_take(&mut self, take: &TakeReport) {
        self.last_take = Some(take.clone());
    }

    // -- time-stretch (alpha slice E2) ---------------------------------------

    /// The tempo each pool source was performed at (`source_tempo <id> <bpm>`), for a
    /// shell's tempo match.
    pub fn source_tempos(&self) -> &std::collections::HashMap<String, f64> {
        &self.source_tempos
    }

    /// **Time-stretch a clip into new pool material.** The render writes a new pool
    /// source (a deterministic name, so stretching twice with the same ratio reuses
    /// the same material instead of piling up copies), and the logged
    /// [`media::ArrangeOp::Stretch`] then points the clip at it — one history entry,
    /// so `undo` restores the old reference and the rendered source stays in the pool
    /// as working material.
    ///
    /// The ratio is `output/input` as a rational. A 1:1 ratio is refused (it would
    /// copy, not stretch), as are a looped clip and a region shorter than the
    /// transform's window (the WSOLA core explains why: nothing to overlap).
    ///
    /// The render allocates its output, so both ends are **bounded** (see
    /// [`MAX_STRETCH_RATIO`] / [`MAX_STRETCH_FRAMES`]): an unbounded `num` is a typo or
    /// an attack, and the process must refuse it rather than die in the allocator.
    pub fn stretch(&mut self, track: &str, clip: &str, num: u32, den: u32) -> Result<(), String> {
        if num == 0 || den == 0 {
            return Err(format!("stretch ratio must be non-zero (got {num}/{den})"));
        }
        if num == den {
            return Err("a 1:1 stretch would copy the material — nothing to do".into());
        }
        if num > MAX_STRETCH_RATIO || den > MAX_STRETCH_RATIO {
            return Err(format!(
                "stretch ratio {num}/{den} is beyond the host's limit of \
                 {MAX_STRETCH_RATIO}:1 (a tempo match lives well inside it)"
            ));
        }
        let Some(dir) = self.pool_dir.clone() else {
            return Err("stretch requires a pool (set_pool first)".into());
        };
        let timeline = self.arrangement()?;
        let c = timeline
            .tracks
            .iter()
            .find(|t| t.id == track)
            .and_then(|t| t.clips.iter().find(|c| c.id == clip))
            .cloned()
            .ok_or_else(|| format!("no clip '{clip}' on track '{track}'"))?;
        if c.loop_len.is_some() {
            return Err("cannot stretch a looped clip (loop phase is not representable)".into());
        }
        let resolver = self
            .pool_resolver
            .clone()
            .ok_or("stretch requires a pool (set_pool first)")?;
        let path =
            resolver(&c.source).ok_or_else(|| format!("pool source '{}' is missing", c.source))?;

        // Read the clip's region (channel 0: a pool source is mono; a split import
        // keeps each channel its own source). A clip's `src_len` is not checked against
        // the file, so read at most what the file has: a declared length longer than the
        // material must not size the allocation (the read is truncated below anyway).
        let mut reader = media::wav::WavReader::open(&path)
            .map_err(|e| format!("stretch {}: {e}", path.display()))?
            .with_channel(0)?;
        reader.seek_frames(c.src_start)?;
        let want = c
            .src_len
            .min(reader.total_frames().saturating_sub(c.src_start)) as usize;
        let mut region = Vec::new();
        region.try_reserve_exact(want).map_err(|e| {
            format!(
                "stretch: cannot read {want} frames of '{}' into memory: {e}",
                c.source
            )
        })?;
        region.resize(want, 0.0);
        let read = reader.read_into(&mut region);
        region.truncate(read);
        if region.is_empty() {
            return Err(format!(
                "clip '{clip}' reads no frames from '{}' (src_start {})",
                c.source, c.src_start
            ));
        }

        let mut stretcher = media::Stretch::for_len(num, den, region.len() as u64)?;
        if (region.len() as u64) < stretcher.min_region_frames() {
            return Err(format!(
                "the clip's region is {} frames — shorter than the stretch window ({}); \
                 it is too short to stretch (a stretch needs overlap to work with)",
                region.len(),
                stretcher.min_region_frames()
            ));
        }
        // The output is bounded before it is allocated: the ratio cap above fixes
        // `expected <= region.len() * MAX_STRETCH_RATIO`, and the frame cap refuses a
        // render longer than the host will materialise.
        let expected = (region.len() as u64).saturating_mul(num as u64) / den as u64;
        if expected > MAX_STRETCH_FRAMES {
            return Err(format!(
                "the stretch would render {expected} frames — beyond the host's limit of \
                 {MAX_STRETCH_FRAMES} (two hours at 48 kHz)"
            ));
        }
        let mut rendered = Vec::new();
        rendered
            .try_reserve_exact(expected as usize + 4_096)
            .map_err(|e| format!("stretch: cannot allocate {expected} frames of output: {e}"))?;
        stretcher.process(&region, &mut rendered);
        stretcher.flush(&mut rendered);
        if rendered.is_empty() {
            return Err("the stretch produced no frames".into());
        }

        // Deterministic id: the same **region** at the same ratio is the same source.
        // The region is part of the id, not just the source: two clips of one source at
        // different offsets render *different* material, and an id keyed on the source
        // alone would make the second render silently overwrite the first clip's — a
        // bug this key exists to prevent (`a_stretch_id_keys_on_the_region_not_the_source`).
        // The region is the *actual* read (`region.len()`), not the clip's declared
        // `src_len`: the content is what the id must key on, and a clip may declare more
        // material than its source holds.
        let id = format!(
            "{}.stretch.{}_{}.{num}_{den}",
            c.source,
            c.src_start,
            region.len()
        );
        let pool = media::Pool::open(&dir)?;
        let frames = pool.write_source(&id, &rendered, self.engine.clock.sample_rate)?;

        // The op is the state (undoable, replayable); the render was the action.
        self.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::Stretch {
                track: track.to_string(),
                clip: clip.to_string(),
                source: id,
                src_len: frames,
                num,
                den,
            },
            at_frame: None,
        })
    }

    // -- persistence: a session is a directory -------------------------------

    /// The session directory, when this session has one (`Save`/`Load` wrote or
    /// read it). The journal — the autosave — lives here.
    pub fn session_dir(&self) -> Option<&std::path::Path> {
        self.session_dir.as_deref()
    }

    /// The last journal write failure, if any. An edit is never *failed* by a
    /// journal error (it is already applied and logged), but autosave must not fail
    /// silently either.
    pub fn journal_error(&self) -> Option<&str> {
        self.journal_error.as_deref()
    }

    /// What the last [`HostSession::load_session`] recovered from the journal.
    pub fn last_recovery(&self) -> Option<&JournalRecovery> {
        self.last_recovery.as_ref()
    }

    /// Write the session as a **directory**: `session.txt` — the replayable log in
    /// the `host v1` text form — plus `pool/`, the material the log refers to. The
    /// pool is copied when the session's pool lives elsewhere, so the directory is
    /// self-contained and can be moved.
    ///
    /// The script is written atomically (temp + rename) and the journal is reset to
    /// that baseline only after it landed, so a crash mid-save leaves the *previous*
    /// session intact.
    pub fn save(&mut self, dir: &std::path::Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("save {}: {e}", dir.display()))?;
        let target_pool = dir.join(POOL_DIR);

        if let Some(src) = self.pool_dir.clone()
            && src != target_pool
        {
            copy_pool(&src, &target_pool)?;
            // Point the live session at the copy **and re-base the log**: the pool is
            // context, and a replay (undo, seek) rebuilds from the history — if the
            // history kept the old path, an undo after a save would re-adopt it (and
            // fail outright if the original was moved or deleted).
            self.set_pool(target_pool.clone())?;
            rebase_pool(&mut self.history, &target_pool);
        }

        let text = self.script_text(dir)?;
        // **The file must be able to reproduce this session**, so parse our own output
        // and compare it to the history before writing anything. A command the text
        // form cannot express (a path with whitespace, a region play, a future op)
        // refuses the save instead of writing a file that opens as a *different*
        // session.
        let mut check = parse_script(&text).map_err(|e| {
            format!(
                "save: the session text does not parse ({e}) — refusing to write a lossy session"
            )
        })?;
        resolve_session_paths(&mut check, dir);
        let expected: Vec<HostCommand> = self
            .history
            .iter()
            .map(|entry| {
                if entry.len() == 1 {
                    entry[0].clone()
                } else {
                    HostCommand::Group {
                        commands: entry.clone(),
                    }
                }
            })
            .collect();
        // The first parsed command is the `session_rate` header.
        if check.get(1..).unwrap_or(&[]) != expected.as_slice() {
            return Err(
                "save: the session text does not round-trip — refusing to write a lossy session"
                    .into(),
            );
        }

        let tmp = dir.join("session.txt.tmp");
        std::fs::write(&tmp, text.as_bytes())
            .map_err(|e| format!("save {}: {e}", tmp.display()))?;
        // Push it to disk before the rename, so the rename cannot publish a file whose
        // contents are still only in the page cache.
        std::fs::File::open(&tmp)
            .and_then(|f| f.sync_all())
            .map_err(|e| format!("sync {}: {e}", tmp.display()))?;
        let script = dir.join(SESSION_FILE);
        std::fs::rename(&tmp, &script).map_err(|e| format!("save {}: {e}", script.display()))?;

        // The script is the baseline; the journal restarts from it. A crash between
        // the rename and this truncation leaves a *stale* journal, which replays on top
        // of the new script — tolerated, because its commands are already in the script
        // and are therefore refused and reported, not fatal (see `apply_journal`).
        std::fs::write(dir.join(JOURNAL_FILE), b"")
            .map_err(|e| format!("save journal in {}: {e}", dir.display()))?;
        self.session_dir = Some(dir.to_path_buf());
        self.journal_error = None;
        self.last_recovery = None;
        Ok(())
    }

    /// The whole session as a `host v1` script: the state commands that rebuild it,
    /// gestures bracketed, and a pool path **relative to `dir`** when the pool lives
    /// inside it — which is what lets a saved session be moved.
    fn script_text(&self, dir: &std::path::Path) -> Result<String, String> {
        let mut out = String::from("host v1\n");
        out.push_str(&format!("session_rate {}\n", self.engine.clock.sample_rate));
        for entry in &self.history {
            write_entry(&mut out, entry, Some(dir))?;
        }
        Ok(out)
    }

    /// Load a session directory: `session.txt`, then its journal (the autosave since
    /// the last save). A torn trailing journal line — a crash mid-write — is dropped
    /// and **reported**, never a parse error that costs the session.
    pub fn load_session(&mut self, dir: &std::path::Path) -> Result<(), String> {
        let script_path = dir.join(SESSION_FILE);
        let script = std::fs::read_to_string(&script_path)
            .map_err(|e| format!("load {}: {e}", script_path.display()))?;
        let mut commands = parse_script(&script)?;
        resolve_session_paths(&mut commands, dir);

        let mut loaded = HostSession::from_script(&commands)?;
        // Apply the journal with no session dir set, so replaying it does not append
        // to itself; the history still grows (a later save keeps the edits).
        let recovery = loaded.apply_journal(dir)?;
        loaded.session_dir = Some(dir.to_path_buf());
        loaded.last_recovery = Some(recovery);
        loaded.playing = false; // a load is a fresh, stopped session
        *self = loaded;
        Ok(())
    }

    /// Apply the journal on top of the loaded script, tolerating a torn final line.
    fn apply_journal(&mut self, dir: &std::path::Path) -> Result<JournalRecovery, String> {
        let mut report = JournalRecovery::default();
        let path = dir.join(JOURNAL_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(report); // no journal yet: the script is the whole session
        };

        // A crash can cut the append anywhere: mid-line, or at a line boundary
        // *inside* a gesture. The journal is a sequence of complete entries, so the
        // recovery rule is "replay the complete ones" — never "refuse to open the
        // session because the last one is incomplete".
        let mut lines: Vec<&str> = text.lines().collect();
        // A final line with no newline is a partial write: drop it.
        if !text.ends_with('\n') && !lines.is_empty() {
            lines.pop();
            report.torn_lines += 1;
        }
        // …and an entry whose `group begin` has no `group end` was cut mid-gesture:
        // drop it (and anything after it, which cannot exist).
        if let Some(open) = lines.iter().rposition(|l| l.trim() == "group begin")
            && !lines[open + 1..].iter().any(|l| l.trim() == "group end")
        {
            report.torn_lines += lines.len() - open;
            lines.truncate(open);
        }
        if lines.iter().all(|l| l.trim().is_empty()) {
            return Ok(report);
        }

        let commands = parse_script(&format!("host v1\n{}\n", lines.join("\n")))?;
        for cmd in &commands {
            // A journal entry the session refuses (a stale journal from a crash in the
            // save's window, or a pool that moved) is **dropped and reported**, never
            // fatal.
            match self.execute(cmd) {
                Ok(()) => report.applied += 1,
                Err(e) => {
                    report.refused += 1;
                    if report.refused_reason.is_none() {
                        report.refused_reason = Some(e);
                    }
                }
            }
        }
        Ok(report)
    }

    /// Append a committed gesture to the journal — the autosave. Best effort: the
    /// edit is already applied, so a failure is recorded ([`Self::journal_error`]),
    /// not raised.
    fn journal_append(&mut self, entry: &[HostCommand]) {
        let Some(dir) = self.session_dir.clone() else {
            return;
        };
        let mut text = String::new();
        // The journal records *edits*, with the paths the session actually used. A
        // command with no text form cannot be in the history (a save would have
        // refused it), so a failure here means nothing to write.
        if write_entry(&mut text, entry, None).is_err() || text.is_empty() {
            return;
        }
        let path = dir.join(JOURNAL_FILE);
        let result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write;
                file.write_all(text.as_bytes())?;
                file.flush()
            });
        match result {
            Ok(()) => self.journal_error = None,
            Err(e) => self.journal_error = Some(format!("journal {}: {e}", path.display())),
        }
    }

    /// Resolve a parsed clip (len 0 = "open the file at apply") to its real
    /// length."""
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
            HostCommand::SetParam {
                plugin,
                param,
                value,
                ..
            } => self.engine.set_param(plugin, param, *value),
            HostCommand::SetTempo {
                bpm, beats_per_bar, ..
            } => self.engine.set_tempo(*bpm, *beats_per_bar),
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
                    return Err(format!(
                        "play channel ch{channel} is beyond the mixer's {channels} channels"
                    ));
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
                let ports = vec![Port {
                    name: "audio",
                    direction: Direction::Out,
                    kind: SignalKind::Audio,
                    channels: 1,
                }];
                let id = match self.engine.graph.out_node {
                    Some(mixer) => self
                        .engine
                        .graph
                        .insert_before(mixer, node, ports)
                        .map_err(|e| format!("play node: {e}"))?,
                    None => self.engine.graph.add_node(node, ports),
                };
                self.player_mailbox = Some(mailbox);
                self.player_underruns = Some(underruns);
                self.player_deferred = Some(deferred);
                self.pending_cords.push((id, *channel));
                self.media
                    .lock()
                    .map_err(|_| "media session poisoned")?
                    .player = Some(intent);
                self.media_commands += 1;
                Ok(())
            }
            HostCommand::Splice {
                at_frame,
                clip,
                crossfade,
            } => {
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
                mailbox
                    .lock()
                    .map_err(|_| "player mailbox poisoned")?
                    .push_back(SpliceCmd {
                        at_frame: *at_frame,
                        incoming,
                        crossfade: *crossfade,
                    });
                self.media
                    .lock()
                    .map_err(|_| "media session poisoned")?
                    .splices
                    .push(intent);
                self.media_commands += 1;
                Ok(())
            }
            HostCommand::Stretch {
                track,
                clip,
                num,
                den,
            } => self.stretch(track, clip, *num, *den),
            HostCommand::SetSourceTempo { source, bpm } => {
                if !bpm.is_finite() || *bpm <= 0.0 {
                    return Err(format!(
                        "source_tempo must be finite and positive, got {bpm}"
                    ));
                }
                if !self.source_tempos.contains_key(source) && self.source_tempos.len() >= 1_000 {
                    return Err("too many source tempos recorded (max 1000)".into());
                }
                // The command is **state**, so the history (and the save, and a replay
                // on seek) already carries it — inserting here is enough: the rebuild
                // reapplies this arm.
                self.source_tempos.insert(source.clone(), *bpm);
                Ok(())
            }
            HostCommand::Record { take_id } => self.record(take_id),
            HostCommand::RecordStop => {
                let take = self.stop_recording()?;
                self.status_take(&take);
                Ok(())
            }
            // Context, validated: a session's rate is set at construction.
            HostCommand::SessionRate { hz } => {
                if *hz == 0 {
                    Err("session_rate must be non-zero".into())
                } else if *hz == self.engine.clock.sample_rate {
                    Ok(())
                } else {
                    Err(format!(
                        "this session runs at {} Hz — a rate is fixed when the session is created \
                         (`new_at`/`from_script`), so {} Hz needs a new session",
                        self.engine.clock.sample_rate, hz
                    ))
                }
            }
            HostCommand::Save { dir } => self.save(dir),
            HostCommand::Load { dir } => self.load_session(dir),
            HostCommand::Group { commands } => self.execute_group(commands),
            HostCommand::Arrange { op, .. } => {
                self.ensure_editor()?;
                if self.pool_resolver.is_none() {
                    return Err(
                        "arrange requires set_pool first (clips need pool-source paths)".into(),
                    );
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
                let (op, fields) =
                    media_ops::encode_pool(&mut self.media_intern, &dir.to_string_lossy());
                self.engine.arrange_logged(op, fields)?;
                self.media
                    .lock()
                    .map_err(|_| "media session poisoned")?
                    .pool_dir = Some(dir.clone());
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
                let mut w =
                    media::WavWriter::create(path, self.engine.clock.sample_rate, channels)?;
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
                self.media
                    .lock()
                    .map_err(|_| "media session poisoned")?
                    .bounces
                    .push(record);
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
        let mixer = self
            .engine
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
        let Some(resolver) = self.pool_resolver.clone() else {
            return Ok(());
        };
        let Some(editor) = &self.editor else {
            return Ok(());
        };
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
                return Err(format!(
                    "track '{}' has no mixer channel ch{ti} (channels={channels})",
                    track.id
                ));
            }
            let node = media::ArrangerNode::new(
                track.clone(),
                &resolver,
                media::DEFAULT_RING_CAPACITY,
                self.engine.clock.sample_rate,
                from_frame,
            )?;
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
                vec![engine::Port {
                    name: "audio",
                    direction: engine::Direction::Out,
                    kind: engine::SignalKind::Audio,
                    channels: 1,
                }],
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
        // **Materialize everything scheduled before measuring anything.** The aligned
        // render below derives its head trim from the *wired* graph, and `wire_pending`
        // only flushes when it has player cords to lay, `wire_arranger` only when the
        // arrangement is dirty — so a mid-session `mount master` (no new cords, nothing
        // dirty) would otherwise be measured as absent and the trim would come out
        // zero, inserting a full transit of silence at the head of the bounce. (Found
        // by the slice's gate, with an executed repro.)
        self.engine.flush_scheduled();
        self.wire_pending()?;
        self.wire_arranger()?;
        // Aligned: the master bus's lookahead is processing latency, not thirty
        // milliseconds of silence at the head of every bounce.
        Ok(self
            .engine
            .render_with_drain_aligned(frames, DrainPolicy::Tails, MAX_DRAIN_FRAMES))
    }

    /// The offline bounce budget: the master may be stereo (L/R), so a frame
    /// costs up to 2 * 4 bytes; a malformed `bounce` must not OOM.
    fn check_bounce_budget(frames: usize) -> Result<(), String> {
        let budget = frames
            .saturating_mul(std::mem::size_of::<f32>())
            .saturating_mul(2);
        if budget > Self::MAX_BOUNCE_BYTES {
            return Err(format!(
                "bounce of {frames} frames exceeds the ~{:.0} MiB budget",
                Self::MAX_BOUNCE_BYTES / (1 << 20)
            ));
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
        self.engine
            .ctx
            .get::<std::sync::Arc<MeterBank>>("mixer.meters")
            .cloned()
    }

    /// The mastering stage's meters, when a `master` plugin is mounted — the
    /// compressor + limiter's output peaks and gain reduction.
    pub fn master_meters(&self) -> Option<std::sync::Arc<engine::plugins::MasterMeters>> {
        self.engine
            .ctx
            .get::<std::sync::Arc<engine::plugins::MasterMeters>>("master.meters")
            .cloned()
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

    /// The session's tempo/meter map (a clone: the engine owns the live one).
    /// Shells read it off the snapshot to quantize in the beat domain — the grid is
    /// UI state, so the *shell* does the snapping, not the host.
    pub fn tempo_map(&self) -> engine::TempoMap {
        self.engine.clock.tempo_map.clone()
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
    HostSession::from_script(script)
}

/// The session sample rate when a script does not declare one.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;

impl HostSession {
    /// Build a session from a parsed script, honouring a `session_rate` line.
    ///
    /// The rate has to be known *before* the engine exists (the clock, the frames in
    /// the log and the device negotiation all derive from it), so it is read here
    /// rather than applied as an edit — `apply` refuses a *different* rate on a live
    /// session for exactly that reason.
    pub fn from_script(commands: &[HostCommand]) -> Result<Self, String> {
        let rate = commands
            .iter()
            .find_map(|cmd| match cmd {
                HostCommand::SessionRate { hz } => Some(*hz),
                _ => None,
            })
            .unwrap_or(DEFAULT_SAMPLE_RATE);
        if rate == 0 {
            return Err("session_rate must be non-zero".into());
        }
        let mut session = HostSession::new_at(rate);
        for cmd in commands {
            // `execute`, not `process`: a script built by `process` alone would have
            // the *state* but an empty history — so a session opened from a file could
            // not be undone, and saving it again would write only what happened after
            // the load. (`execute` appends to the journal only when a session
            // directory is set, and a session being built or loaded has none.)
            session.execute(cmd)?;
        }
        Ok(session)
    }

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
        // A rebuild replaces the session, which would drop a take in progress. Stop it
        // properly first: the take is finalized (its WAV + peaks land in the pool) **and**
        // reported, instead of vanishing with the old session.
        if self.recording.is_some() {
            let _ = self.stop_recording();
        }
        let history = self.history.clone();
        let redo = std::mem::take(&mut self.redo);
        let playing = self.playing;
        let mut rebuilt = HostSession::new_at(self.engine.clock.sample_rate);
        for entry in &history {
            // State that takes effect *after* the target is not yet in force at
            // `frame`. Applying it would render the clock past the target — which
            // makes a backward seek a no-op, because `process` renders up to the
            // command's `at_frame`. The skip is per **entry**, so a gesture is
            // never half-applied: the rebuild reconstructs the session as it was at
            // `frame`.
            if entry
                .first()
                .and_then(|c| c.at_frame())
                .is_some_and(|at| at > frame)
            {
                continue;
            }
            for cmd in entry {
                rebuilt.process(cmd)?;
            }
        }
        rebuilt.render_to(frame)?;
        rebuilt.playing = playing;
        rebuilt.redo = redo;
        // **Persistence survives a rebuild.** A replay replaces the session, so the
        // session directory (and the autosave state) has to come across or the
        // journal would silently stop after the first undo/seek.
        rebuilt.last_take = self.last_take.take();
        rebuilt.session_dir = self.session_dir.clone();
        rebuilt.journal_error = self.journal_error.take();
        rebuilt.last_recovery = self.last_recovery.take();
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
        // One *entry* is one gesture: a group of arrangement ops undoes as a unit.
        let Some(pos) = self.history.iter().rposition(|entry| {
            entry
                .iter()
                .any(|c| matches!(c, HostCommand::Arrange { .. }))
        }) else {
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
        self.history.iter().any(|entry| {
            entry
                .iter()
                .any(|c| matches!(c, HostCommand::Arrange { .. }))
        })
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
        if let HostCommand::Mount {
            plugin: "mixer",
            params,
            ..
        } = cmd
        {
            // The mixer's channel count is the contract's own state. A fraction
            // (e.g. channels=4.5) must be refused, not silently truncated (the
            // review's "float cast" nit), and it must be a sane positive count.
            let channels = params
                .iter()
                .find(|(name, _)| *name == "channels")
                .map(|(_, v)| {
                    if !v.is_finite() || *v <= 0.0 || v.fract() != 0.0 {
                        return Err(format!(
                            "mixer channels must be a positive whole number, got {v}"
                        ));
                    }
                    Ok(*v as usize)
                })
                .transpose()?
                .unwrap_or(4);
            if channels > MIXER_CHANNELS_MAX {
                return Err(format!(
                    "mixer channels {channels} exceeds the max {MIXER_CHANNELS_MAX}"
                ));
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
                // One entry = one gesture: a group's members land as one entry, a bare
                // command as a one-element entry.
                let entry = match cmd {
                    HostCommand::Group { commands } => commands.clone(),
                    other => vec![other.clone()],
                };
                // Autosave: the journal is appended per committed gesture, so a crash
                // costs at most the gesture in flight.
                self.journal_append(&entry);
                self.history.push(entry);
                self.redo.clear();
            }
        }
        r
    }
}

// ------------------------------------------------------------ persistence

/// The session script's file name inside a session directory.
const SESSION_FILE: &str = "session.txt";
/// The autosave journal inside a session directory.
const JOURNAL_FILE: &str = "journal.txt";
/// The pool's subdirectory inside a session directory.
const POOL_DIR: &str = "pool";

/// A take in progress: the capture demux writing the pool, plus the input device
/// stream that feeds it (kept here because a cpal `Stream` is `!Send`).
struct Recording {
    handle: Option<media::devices::InputHandle>,
    capture: media::Capture,
    take_id: String,
    /// The pool source ids the take will produce (`take.ch0`, `take.ch1`, …).
    sources: Vec<String>,
}

/// A finished take: what the shell needs to say what happened, and what the pool
/// listing will show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeReport {
    pub take_id: String,
    /// Session frames written per channel.
    pub frames: u64,
    /// Source frames the capture had to drop (the ring was full).
    pub dropped: u64,
    pub channels: usize,
    /// The pool source ids to place on a track (`{take_id}.ch{k}`).
    pub sources: Vec<String>,
    pub sample_rate: u32,
}

/// A take in progress, for a live indicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingStatus {
    pub take_id: String,
    pub frames: u64,
    pub dropped: u64,
    pub channels: usize,
}

/// What a load recovered from the journal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalRecovery {
    /// Journal commands replayed on top of the script.
    pub applied: usize,
    /// Lines dropped because their **entry was incomplete** — a partial final line,
    /// or a gesture whose `group end` never made it to disk. Dropped, and reported,
    /// rather than refusing to open the session.
    pub torn_lines: usize,
    /// Journal commands the session **refused** (a stale journal from a crash inside the
    /// save's window, or a path that moved). Dropped and reported, not fatal: refusing
    /// to open a session because of an autosave line is the worse failure.
    pub refused: usize,
    /// The first refusal, for a shell to show.
    pub refused_reason: Option<String>,
}

/// Write one history **entry** (one gesture) as script lines. A multi-command entry
/// is bracketed with `group begin`/`group end`, so the gesture structure survives a
/// save (and replays as one undo step).
fn write_entry(
    out: &mut String,
    entry: &[HostCommand],
    session_dir: Option<&std::path::Path>,
) -> Result<(), String> {
    let grouped = entry.len() > 1;
    if grouped {
        out.push_str("group begin\n");
    }
    for cmd in entry {
        let Some(line) = format_command(cmd, session_dir) else {
            return Err(format!(
                "the host v1 text form cannot express {cmd:?} — refusing to write a session that \
                 would lose it"
            ));
        };
        out.push_str(&line);
        out.push('\n');
    }
    if grouped {
        out.push_str("group end\n");
    }
    Ok(())
}

/// Re-point every `pool` command in the history at `pool` (recursing into gestures).
/// The pool is **context**, not an edit: after a save copies the pool into the session
/// directory, the log must name the copy, or a later replay would rebuild against the
/// original path.
fn rebase_pool(history: &mut [Vec<HostCommand>], pool: &std::path::Path) {
    fn one(cmd: &mut HostCommand, pool: &std::path::Path) {
        match cmd {
            HostCommand::Pool { dir } => *dir = pool.to_path_buf(),
            HostCommand::Group { commands } => {
                for member in commands {
                    one(member, pool);
                }
            }
            _ => {}
        }
    }
    for entry in history {
        for cmd in entry {
            one(cmd, pool);
        }
    }
}

/// Format one command in the `host v1` text form, or `None` when it has none (a
/// pure action, which a log never records). `session_dir` makes a `pool` path
/// relative to the session directory when the pool lives inside it.
pub fn format_command(cmd: &HostCommand, session_dir: Option<&std::path::Path>) -> Option<String> {
    let frame = |at: &Option<u64>| at.map(|f| format!(" @{f}")).unwrap_or_default();
    Some(match cmd {
        HostCommand::Mount {
            plugin,
            params,
            at_frame,
        } => {
            let mut line = format!("mount {plugin}");
            for (key, value) in params {
                line.push_str(&format!(" {key}={}", fmt_f32(*value)));
            }
            line.push_str(&frame(at_frame));
            line
        }
        HostCommand::Patch { from, to, at_frame } => format!(
            "patch {}.{} {}.{}{}",
            from.0,
            from.1,
            to.0,
            to.1,
            frame(at_frame)
        ),
        HostCommand::SetParam {
            plugin,
            param,
            value,
            at_frame,
        } => format!(
            "set_param {plugin} {param} {}{}",
            fmt_f32(*value),
            frame(at_frame)
        ),
        HostCommand::SetTempo {
            bpm,
            beats_per_bar,
            at_frame,
        } => format!("set_tempo {bpm} {beats_per_bar}{}", frame(at_frame)),
        HostCommand::Unmount { plugin, at_frame } => format!("unmount {plugin}{}", frame(at_frame)),
        HostCommand::Pool { dir } => format!("pool {}", pool_text(dir, session_dir)),
        HostCommand::SessionRate { hz } => format!("session_rate {hz}"),
        HostCommand::Arrange { op, at_frame } => {
            format!("arrange {}{}", format_arrange(op), frame(at_frame))
        }
        // The `v1` text form plays a **whole file** (`play <path> ch<N>`), so a region
        // play has no representation at all: return `None`, and `save` refuses loudly
        // rather than write a session that would open as a different one.
        HostCommand::Play {
            clip,
            channel,
            at_frame,
        } => {
            if clip.start != 0 || clip.len != 0 {
                return None;
            }
            format!(
                "play {} ch{channel}{}",
                clip.path.display(),
                frame(at_frame)
            )
        }
        HostCommand::Splice {
            at_frame,
            clip,
            crossfade,
        } => {
            if clip.start != 0 || clip.len != 0 {
                return None;
            }
            format!("splice {at_frame} {} {crossfade}", clip.path.display())
        }
        HostCommand::Group { commands } => {
            let mut out = String::from("group begin");
            for member in commands {
                out.push('\n');
                out.push_str(&format_command(member, session_dir)?);
            }
            out.push_str("\ngroup end");
            out
        }
        HostCommand::SetSourceTempo { source, bpm } => {
            format!("source_tempo {source} {}", fmt_f64(*bpm))
        }
        // Actions (and the live transport) are not part of a log.
        HostCommand::TransportPlay
        | HostCommand::TransportStop
        | HostCommand::TransportSeek { .. }
        | HostCommand::Undo
        | HostCommand::Redo
        | HostCommand::Record { .. }
        | HostCommand::RecordStop
        | HostCommand::Bounce { .. }
        | HostCommand::Stretch { .. }
        | HostCommand::Save { .. }
        | HostCommand::Load { .. } => return None,
    })
}

/// Format an arrangement op as the `arrange …` operands (the inverse of
/// `parse_arrange`).
pub fn format_arrange(op: &media::ArrangeOp) -> String {
    use media::ArrangeOp as Op;
    match op {
        Op::AddTrack { track } => format!("add_track {track}"),
        Op::RemoveTrack { track } => format!("remove_track {track}"),
        Op::RenameTrack { track, to } => format!("rename_track {track} {to}"),
        Op::MoveTrack { track, index } => format!("move_track {track} {index}"),
        Op::Reverse { track, clip } => format!("reverse {track} {clip}"),
        Op::Stretch {
            track,
            clip,
            source,
            src_len,
            num,
            den,
        } => format!("stretch {track} {clip} {source} {src_len} {num} {den}"),
        Op::AddClip { track, clip } => {
            let mut line = format!(
                "add_clip {track} {} {} {} {} {} {} {} {}",
                clip.id,
                clip.source,
                clip.src_start,
                clip.src_len,
                clip.at_frame,
                clip.fade_in,
                clip.fade_out,
                fmt_f32(clip.gain)
            );
            if let Some(loop_len) = clip.loop_len {
                line.push_str(&format!(" {loop_len}"));
            }
            line
        }
        Op::RazorSplit {
            track,
            clip,
            new_left,
            new_right,
            at_frame,
        } => format!("razor_split {track} {clip} {new_left} {new_right} {at_frame}"),
        Op::Trim {
            track,
            clip,
            edge,
            by_frames,
        } => {
            let edge = match edge {
                media::Edge::Start => "start",
                media::Edge::End => "end",
            };
            format!("trim {track} {clip} {edge} {by_frames}")
        }
        Op::MoveClip {
            track,
            clip,
            at_frame,
        } => format!("move_clip {track} {clip} {at_frame}"),
        Op::MoveClipToTrack {
            from,
            clip,
            to,
            at_frame,
        } => format!("move_clip_to_track {from} {clip} {to} {at_frame}"),
        Op::Duplicate {
            track,
            clip,
            new_id,
        } => format!("duplicate {track} {clip} {new_id}"),
        Op::Delete { track, clip } => format!("delete {track} {clip}"),
        Op::SetClipGain { track, clip, gain } => {
            format!("set_clip_gain {track} {clip} {}", fmt_f32(*gain))
        }
        Op::SetClipFade {
            track,
            clip,
            fade_in,
            fade_out,
        } => format!("set_clip_fade {track} {clip} {fade_in} {fade_out}"),
        Op::LoopRegion { track, clip, times } => format!("loop_region {track} {clip} {times}"),
        Op::ChopClip {
            track,
            clip,
            times,
            prefix,
        } => format!("chop {track} {clip} {times} {prefix}"),
    }
}

/// `f32` in the shortest form that parses back to the same bits — the log's values
/// must survive a save/load round trip exactly.
fn fmt_f32(value: f32) -> String {
    format!("{value:?}")
}

/// `f64` in the shortest form that parses back to the same value (a tempo is a
/// measurement, and the log must not round it differently than the session did).
fn fmt_f64(value: f64) -> String {
    format!("{value:?}")
}

/// The `pool` operand: relative to the session directory when the pool lives inside
/// it, absolute otherwise.
fn pool_text(dir: &std::path::Path, session_dir: Option<&std::path::Path>) -> String {
    if let Some(root) = session_dir
        && let Ok(rel) = dir.strip_prefix(root)
    {
        return rel.to_string_lossy().into_owned();
    }
    dir.to_string_lossy().into_owned()
}

/// A session script's relative `pool` path resolves against the session directory.
fn resolve_session_paths(commands: &mut [HostCommand], dir: &std::path::Path) {
    for cmd in commands {
        match cmd {
            HostCommand::Pool { dir: pool } if pool.is_relative() => {
                *pool = dir.join(&*pool);
            }
            HostCommand::Group { commands } => resolve_session_paths(commands, dir),
            _ => {}
        }
    }
}

/// Copy a pool's material (WAVs and their peak sidecars) into `dst`, skipping files
/// that are already there at the same size.
fn copy_pool(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("save pool {}: {e}", dst.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("pool {}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let keep = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("wav") | Some("peaks")
        );
        if !keep || !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        let target = dst.join(name);
        // Skip only when size *and* modification time match: a file edited in place
        // at the same length must still be copied.
        let same = std::fs::metadata(&target)
            .ok()
            .zip(std::fs::metadata(&path).ok())
            .is_some_and(|(a, b)| a.len() == b.len() && a.modified().ok() == b.modified().ok());
        if !same {
            std::fs::copy(&path, &target)
                .map_err(|e| format!("save pool {}: {e}", target.display()))?;
        }
    }
    Ok(())
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
/// group begin                                 # a gesture: one undo step
/// arrange trim t0 c0 start 4800
/// arrange trim t0 c0 end -4800
/// group end
/// record jam1                                 # capture the input device into the pool
/// record stop                                 # finalize the take
/// session_rate 48000                          # the session's rate (context)
/// save /data/mysong.d                         # write the session directory
/// load /data/mysong.d                         # read it back (+ its journal)
/// pool /data/takes                           # the media pool dir (adopting it
///                                             # resamples foreign-rate sources)
/// arrange add_track t0 @0                    # the clip editor (P1.3.4)
/// arrange add_clip t0 c0 s1 0 4000 0 0 0 1.0 @0
/// bounce 6000 /abs/out.wav
/// ```
pub fn parse_script(text: &str) -> Result<Vec<HostCommand>, String> {
    let mut lines = text.lines();
    let mut commands: Vec<HostCommand> = Vec::new();
    // `group begin` … `group end` brackets one **gesture**: its members are pushed
    // into `commands` by the arms below and folded into a `Group` here, so a saved
    // script round-trips the gesture structure (and a replay undoes it as one step).
    let mut group: Option<Vec<HostCommand>> = None;

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
        return Err(format!(
            "bad version line '{version_line}' (want '{expected}')"
        ));
    }

    for (lineno, raw) in lines.enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let at = header_line + lineno + 1; // header line + loop offset (lineno is 0-based)
        let mut words: Vec<&str> = line.split_whitespace().collect();
        // Optional trailing modifiers, in either order: `@frame` says *when* the
        // command is applied, `snap=<frames>` quantizes the frame operand it
        // carries (the grid is UI state — the log records the frame it produced).
        let mut at_frame = None;
        let mut snap: Option<u64> = None;
        loop {
            match words.last().copied() {
                Some(tok) if tok.starts_with('@') && at_frame.is_none() => {
                    let f = tok[1..]
                        .parse::<u64>()
                        .map_err(|_| format!("line {at}: bad frame '{tok}'"))?;
                    words.pop();
                    at_frame = Some(f);
                }
                Some(tok) if tok.starts_with("snap=") && snap.is_none() => {
                    snap = Some(parse_snap(tok, at)?);
                    words.pop();
                }
                _ => break,
            }
        }
        let kind = words.first().copied().unwrap_or("");
        // Which commands consumed the `snap=` modifier (one that cannot use it is
        // a parse error, never a silent no-op).
        let mut snap_used = false;
        // What this line pushes lands after `mark`; a fold at the end of the line
        // moves it into the open group (if any).
        let mark = commands.len();
        match kind {
            "group" => match word(&words, 1, at)? {
                "begin" => {
                    if group.is_some() {
                        return Err(format!("line {at}: `group begin` inside a group"));
                    }
                    group = Some(Vec::new());
                }
                "end" => {
                    let members = group
                        .take()
                        .ok_or_else(|| format!("line {at}: `group end` without `group begin`"))?;
                    if members.is_empty() {
                        return Err(format!("line {at}: an empty group changes nothing"));
                    }
                    commands.push(HostCommand::Group { commands: members });
                }
                other => {
                    return Err(format!("line {at}: group takes begin|end, got '{other}'"));
                }
            },
            "mount" => {
                let plugin = in_list(HOST_PLUGINS, word(&words, 1, at)?, "plugin")?;
                let mut params = Vec::new();
                for w in &words[2..] {
                    let Some((k, v)) = w.split_once('=') else {
                        return Err(format!("line {at}: bad param '{w}' (want k=v)"));
                    };
                    let value = v
                        .parse::<f32>()
                        .map_err(|_| format!("line {at}: bad value '{v}'"))?;
                    params.push((in_list(HOST_PARAMS, k, "param")?, value));
                }
                commands.push(HostCommand::Mount {
                    plugin,
                    params,
                    at_frame,
                });
            }
            "patch" => {
                exact(&words, 3, at, "patch")?;
                let from = port_ref(&words, 1, at)?;
                let to = port_ref(&words, 2, at)?;
                commands.push(HostCommand::Patch { from, to, at_frame });
            }
            "set_param" => {
                exact(&words, 4, at, "set_param")?;
                let plugin = in_list(HOST_PLUGINS, word(&words, 1, at)?, "plugin")?;
                let param = in_list(HOST_PARAMS, word(&words, 2, at)?, "param")?;
                let value = word(&words, 3, at)?
                    .parse::<f32>()
                    .map_err(|_| format!("line {at}: bad value"))?;
                commands.push(HostCommand::SetParam {
                    plugin,
                    param,
                    value,
                    at_frame,
                });
            }
            "set_tempo" => {
                exact(&words, 3, at, "set_tempo")?;
                let bpm = word(&words, 1, at)?
                    .parse()
                    .map_err(|_| format!("line {at}: bad bpm"))?;
                let beats = word(&words, 2, at)?
                    .parse()
                    .map_err(|_| format!("line {at}: bad beats"))?;
                commands.push(HostCommand::SetTempo {
                    bpm,
                    beats_per_bar: beats,
                    at_frame,
                });
            }
            "unmount" => {
                exact(&words, 2, at, "unmount")?;
                let plugin = in_list(HOST_PLUGINS, word(&words, 1, at)?, "plugin")?;
                commands.push(HostCommand::Unmount { plugin, at_frame });
            }
            "transport" => {
                let what = word(&words, 1, at)?;
                match what {
                    // (the `seek` arm below consumes `snap=`, so it is marked used
                    // there rather than here)
                    "play" => {
                        exact(&words, 2, at, "transport play")?;
                        commands.push(HostCommand::TransportPlay);
                    }
                    "stop" => {
                        exact(&words, 2, at, "transport stop")?;
                        commands.push(HostCommand::TransportStop);
                    }
                    "seek" => {
                        exact(&words, 3, at, "transport seek")?;
                        let frame = word(&words, 2, at)?
                            .parse::<u64>()
                            .map_err(|_| format!("line {at}: transport seek needs a frame"))?;
                        let frame = match snap {
                            Some(step) => {
                                snap_used = true;
                                media::quantize_frames(frame, step)
                            }
                            None => frame,
                        };
                        commands.push(HostCommand::TransportSeek { frame });
                    }
                    other => {
                        return Err(format!(
                            "line {at}: unknown transport '{other}' (want play|stop|seek)"
                        ));
                    }
                }
            }
            "undo" => {
                exact(&words, 1, at, "undo")?;
                commands.push(HostCommand::Undo);
            }
            "redo" => {
                exact(&words, 1, at, "redo")?;
                commands.push(HostCommand::Redo);
            }
            "play" => {
                exact(&words, 3, at, "play")?;
                let clip = ClipRef {
                    path: PathBuf::from(word(&words, 1, at)?),
                    start: 0,
                    len: 0,
                };
                let channel = channel_of(word(&words, 2, at)?, at)?;
                commands.push(HostCommand::Play {
                    clip,
                    channel,
                    at_frame,
                });
            }
            "splice" => {
                exact(&words, 4, at, "splice")?;
                let at_frame = word(&words, 1, at)?
                    .parse()
                    .map_err(|_| format!("line {at}: bad frame"))?;
                let clip = ClipRef {
                    path: PathBuf::from(word(&words, 2, at)?),
                    start: 0,
                    len: 0,
                };
                let crossfade = word(&words, 3, at)?
                    .parse()
                    .map_err(|_| format!("line {at}: bad crossfade"))?;
                commands.push(HostCommand::Splice {
                    at_frame,
                    clip,
                    crossfade,
                });
            }
            "record" => {
                exact(&words, 2, at, "record")?;
                let take_id = word(&words, 1, at)?;
                if take_id == "stop" {
                    commands.push(HostCommand::RecordStop);
                } else {
                    commands.push(HostCommand::Record {
                        take_id: take_id.to_string(),
                    });
                }
            }
            "bounce" => {
                exact(&words, 3, at, "bounce")?;
                let frames = word(&words, 1, at)?
                    .parse()
                    .map_err(|_| format!("line {at}: bad frames"))?;
                let path = PathBuf::from(word(&words, 2, at)?);
                commands.push(HostCommand::Bounce { frames, path });
            }
            "stretch" => {
                exact(&words, 5, at, "stretch")?;
                let track = word(&words, 1, at)?.to_string();
                let clip = word(&words, 2, at)?.to_string();
                let num = word(&words, 3, at)?
                    .parse::<u32>()
                    .map_err(|_| format!("line {at}: stretch num must be a whole number"))?;
                let den = word(&words, 4, at)?
                    .parse::<u32>()
                    .map_err(|_| format!("line {at}: stretch den must be a whole number"))?;
                commands.push(HostCommand::Stretch {
                    track,
                    clip,
                    num,
                    den,
                });
            }
            "source_tempo" => {
                exact(&words, 3, at, "source_tempo")?;
                let source = word(&words, 1, at)?.to_string();
                let bpm = word(&words, 2, at)?
                    .parse::<f64>()
                    .map_err(|_| format!("line {at}: source_tempo needs a bpm"))?;
                commands.push(HostCommand::SetSourceTempo { source, bpm });
            }
            "pool" => {
                exact(&words, 2, at, "pool")?;
                let dir = PathBuf::from(word(&words, 1, at)?);
                commands.push(HostCommand::Pool { dir });
            }
            "session_rate" => {
                exact(&words, 2, at, "session_rate")?;
                let hz = word(&words, 1, at)?
                    .parse::<u32>()
                    .map_err(|_| format!("line {at}: bad session_rate (want Hz)"))?;
                commands.push(HostCommand::SessionRate { hz });
            }
            "save" => {
                exact(&words, 2, at, "save")?;
                let dir = PathBuf::from(word(&words, 1, at)?);
                commands.push(HostCommand::Save { dir });
            }
            "load" => {
                exact(&words, 2, at, "load")?;
                let dir = PathBuf::from(word(&words, 1, at)?);
                commands.push(HostCommand::Load { dir });
            }
            "arrange" => {
                let op = parse_arrange(&words[1..], at, snap)?;
                snap_used = snap.is_some();
                commands.push(HostCommand::Arrange { op, at_frame });
            }
            other => return Err(format!("line {at}: unknown command '{other}'")),
        }
        if snap.is_some() && !snap_used {
            return Err(format!(
                "line {at}: snap=<frames> applies to a command with a frame operand (arrange add_clip|move_clip|move_clip_to_track|razor_split, transport seek)"
            ));
        }
        // Fold this line's command(s) into the open gesture. A `group end` pushed
        // the closed group itself and is not folded (nesting is refused above).
        if kind != "group"
            && let Some(members) = group.as_mut()
        {
            members.extend(commands.drain(mark..));
        }
    }
    if group.is_some() {
        return Err("unterminated `group begin` (want a matching `group end`)".into());
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
    let mut at_frame = None;
    let mut snap: Option<u64> = None;
    loop {
        match words.last().copied() {
            Some(tok) if tok.starts_with('@') && at_frame.is_none() => {
                let f = tok[1..]
                    .parse::<u64>()
                    .map_err(|_| format!("bad frame '{tok}'"))?;
                words.pop();
                at_frame = Some(f);
            }
            Some(tok) if tok.starts_with("snap=") && snap.is_none() => {
                snap = Some(parse_snap(tok, 1)?);
                words.pop();
            }
            _ => break,
        }
    }
    let operands = if words.first() == Some(&"arrange") {
        &words[1..]
    } else {
        &words[..]
    };
    let op = parse_arrange(operands, 1, snap)?;
    Ok((op, at_frame))
}

/// Parse an `arrange` command's operands (everything after the `arrange`
/// keyword) into an [`ArrangeOp`]. Grammar per op:
///
/// ```text
/// arrange add_track t0 @0
/// arrange rename_track t0 lead @0
/// arrange move_track t0 1 @0
/// arrange reverse t0 c0 @0
/// arrange stretch t0 c0 c0.stretch.3_2 6000 3 2 @0
/// stretch t0 c0 3 2                 # the render form: write the material, then log
/// source_tempo jam.ch0 90
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
/// arrange move_clip t0 c0 48213 snap=480 @0   # → move_clip t0 c0 48000 (a snapped edit)
/// ```
///
/// `snap=<frames>` is an additive modifier on any command with a frame operand
/// (`add_clip`, `move_clip`, `move_clip_to_track`, `razor_split`, `transport
/// seek`): the frame is rounded to the nearest multiple of `<frames>` **at parse
/// time**, so a script snaps exactly like the shell and the log still records one
/// absolute frame. A grid is never stored, and `snap=0` is refused.
fn parse_arrange(words: &[&str], at: usize, snap: Option<u64>) -> Result<media::ArrangeOp, String> {
    let op = words
        .first()
        .copied()
        .ok_or_else(|| format!("line {at}: arrange needs an op"))?;
    // Which operand is the op's **frame** — the one `snap=<frames>` quantizes.
    // A frame the shell produced is a frame the script can reproduce: the op is
    // logged already snapped, so replay is untouched and no grid is stored.
    let frame_operand = match op {
        "add_clip" => Some(6),
        "move_clip" => Some(3),
        "move_clip_to_track" => Some(4),
        "razor_split" => Some(5),
        _ => None,
    };
    if snap.is_some() && frame_operand.is_none() {
        return Err(format!(
            "line {at}: arrange {op} has no frame operand for snap="
        ));
    }
    // helpers: operand i is `words[i]` — `words[0]` is the op name, so the first
    // real operand (e.g. add_track's track) is `words[1]` = operand 1. Error
    // messages number operands the way a script author counts them (`i`, not i+1).
    let s = |i: usize| -> Result<String, String> {
        words
            .get(i)
            .copied()
            .map(str::to_owned)
            .ok_or_else(|| format!("line {at}: arrange {op} missing operand {i}"))
    };
    let u = |i: usize| -> Result<u64, String> {
        let value: u64 = s(i)?
            .parse()
            .map_err(|_| format!("line {at}: arrange {op} operand {i} must be a frame/count"))?;
        Ok(match snap {
            Some(step) if frame_operand == Some(i) => media::quantize_frames(value, step),
            _ => value,
        })
    };
    let i64 = |i: usize| -> Result<i64, String> {
        s(i)?
            .parse()
            .map_err(|_| format!("line {at}: arrange {op} operand {i} must be an integer"))
    };
    let f = |i: usize| -> Result<f32, String> {
        let v: f32 = s(i)?
            .parse()
            .map_err(|_| format!("line {at}: arrange {op} operand {i} must be a number"))?;
        if !v.is_finite() {
            return Err(format!(
                "line {at}: arrange {op} operand {i} must be finite (a NaN/inf gain or count reaches the audio path)"
            ));
        }
        Ok(v)
    };
    // strict arity: a wrong operand COUNT is a parse error (the wire schema must
    // not silently accept trailing junk). `add_clip`'s `[loop_len]` is the one
    // documented optional trailing operand.
    let arity = |n: usize| -> Result<(), String> {
        if words.len() != n {
            return Err(format!(
                "line {at}: arrange {op} expects {n} words (op + operands), got {}",
                words.len()
            ));
        }
        Ok(())
    };

    match op {
        "add_track" => {
            arity(2)?;
            Ok(media::ArrangeOp::AddTrack { track: s(1)? })
        }
        "remove_track" => {
            arity(2)?;
            Ok(media::ArrangeOp::RemoveTrack { track: s(1)? })
        }
        "rename_track" => {
            arity(3)?;
            Ok(media::ArrangeOp::RenameTrack {
                track: s(1)?,
                to: s(2)?,
            })
        }
        "move_track" => {
            arity(3)?;
            Ok(media::ArrangeOp::MoveTrack {
                track: s(1)?,
                index: usize::try_from(u(2)?)
                    .map_err(|_| format!("line {at}: arrange move_track index is too large"))?,
            })
        }
        "reverse" => {
            arity(3)?;
            Ok(media::ArrangeOp::Reverse {
                track: s(1)?,
                clip: s(2)?,
            })
        }
        "stretch" => {
            // The **logged** form: it names the materialised source and its length, so
            // a replay reproduces the clip's reference without re-rendering. A script
            // usually asks for the render instead (`stretch <track> <clip> <num> <den>`
            // as a command); this op form is what the log and a saved session carry.
            arity(7)?;
            Ok(media::ArrangeOp::Stretch {
                track: s(1)?,
                clip: s(2)?,
                source: s(3)?,
                src_len: u(4)?,
                num: u32::try_from(u(5)?)
                    .map_err(|_| format!("line {at}: arrange stretch num does not fit a u32"))?,
                den: u32::try_from(u(6)?)
                    .map_err(|_| format!("line {at}: arrange stretch den does not fit a u32"))?,
            })
        }
        "add_clip" => {
            // add_clip track c0 source src_start src_len at_frame fade_in fade_out gain [loop_len]
            // operands (words[0]=op): track(1) id(2) source(3) src_start(4) src_len(5)
            //                       at_frame(6) fade_in(7) fade_out(8) gain(9) loop_len(10)
            if !(10..=11).contains(&words.len()) {
                return Err(format!(
                    "line {at}: arrange add_clip expects 10 or 11 words (op + 9 operands + optional loop_len), got {}",
                    words.len()
                ));
            }
            let clip = media::Clip {
                reversed: false,
                id: s(2)?,
                source: s(3)?,
                src_start: u(4)?,
                src_len: u(5)?,
                at_frame: u(6)?,
                fade_in: u(7)?,
                fade_out: u(8)?,
                gain: f(9)?,
                loop_len: words
                    .get(10)
                    .map(|x| x.parse::<u64>())
                    .transpose()
                    .map_err(|_| format!("line {at}: arrange add_clip loop_len must be a count"))?
                    .filter(|v| *v != 0),
            };
            Ok(media::ArrangeOp::AddClip { track: s(1)?, clip })
        }
        "razor_split" => {
            arity(6)?;
            Ok(media::ArrangeOp::RazorSplit {
                track: s(1)?,
                clip: s(2)?,
                new_left: s(3)?,
                new_right: s(4)?,
                at_frame: u(5)?,
            })
        }
        "trim" => {
            arity(5)?;
            Ok(media::ArrangeOp::Trim {
                track: s(1)?,
                clip: s(2)?,
                edge: match s(3)?.as_str() {
                    "start" => media::Edge::Start,
                    "end" => media::Edge::End,
                    other => {
                        return Err(format!(
                            "line {at}: arrange trim edge must be start|end, got '{other}'"
                        ));
                    }
                },
                by_frames: i64(4)?,
            })
        }
        "move_clip" => {
            arity(4)?;
            Ok(media::ArrangeOp::MoveClip {
                track: s(1)?,
                clip: s(2)?,
                at_frame: u(3)?,
            })
        }
        "move_clip_to_track" => {
            arity(5)?;
            Ok(media::ArrangeOp::MoveClipToTrack {
                from: s(1)?,
                clip: s(2)?,
                to: s(3)?,
                at_frame: u(4)?,
            })
        }
        "duplicate" => {
            arity(4)?;
            Ok(media::ArrangeOp::Duplicate {
                track: s(1)?,
                clip: s(2)?,
                new_id: s(3)?,
            })
        }
        "delete" => {
            arity(3)?;
            Ok(media::ArrangeOp::Delete {
                track: s(1)?,
                clip: s(2)?,
            })
        }
        "set_clip_gain" => {
            arity(4)?;
            Ok(media::ArrangeOp::SetClipGain {
                track: s(1)?,
                clip: s(2)?,
                gain: f(3)?,
            })
        }
        "set_clip_fade" => {
            arity(5)?;
            Ok(media::ArrangeOp::SetClipFade {
                track: s(1)?,
                clip: s(2)?,
                fade_in: u(3)?,
                fade_out: u(4)?,
            })
        }
        "loop_region" => {
            arity(4)?;
            Ok(media::ArrangeOp::LoopRegion {
                track: s(1)?,
                clip: s(2)?,
                times: u32::try_from(u(3)?).map_err(|_| {
                    format!("line {at}: arrange loop_region operand 3 must fit a u32 (times)")
                })?,
            })
        }
        "chop" => {
            arity(5)?;
            Ok(media::ArrangeOp::ChopClip {
                track: s(1)?,
                clip: s(2)?,
                times: u32::try_from(u(3)?).map_err(|_| {
                    format!("line {at}: arrange chop operand 3 must fit a u32 (times)")
                })?,
                prefix: s(4)?,
            })
        }
        other => Err(format!("line {at}: unknown arrange op '{other}'")),
    }
}

/// A command with a fixed word count: an **extra** word is a typo, not something to
/// ignore. Silently ignoring it is how `record bad id!` becomes a take called `bad`
/// (found by a test that then opened the real input device).
fn exact(words: &[&str], want: usize, at: usize, what: &str) -> Result<(), String> {
    if words.len() != want {
        return Err(format!(
            "line {at}: {what} takes {} operand(s), got {}",
            want - 1,
            words.len() - 1
        ));
    }
    Ok(())
}

/// Parse a `snap=<frames>` modifier. A zero step is refused rather than treated
/// as "no grid": omitting the modifier *is* "no grid", and a zero divisor would
/// quantize every frame to zero.
fn parse_snap(token: &str, at: usize) -> Result<u64, String> {
    let step = token["snap=".len()..]
        .parse::<u64>()
        .map_err(|_| format!("line {at}: bad snap modifier '{token}' (want snap=<frames>)"))?;
    if step == 0 {
        return Err(format!(
            "line {at}: snap=0 is not a grid — omit the modifier for no snapping"
        ));
    }
    Ok(step)
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
    let s = words
        .get(idx)
        .ok_or_else(|| format!("line {at}: expected a plugin.port reference"))?;
    let Some((plugin, port)) = s.split_once('.') else {
        return Err(format!(
            "line {at}: bad port reference '{s}' (want plugin.port)"
        ));
    };
    Ok((
        in_list(HOST_PLUGINS, plugin, "plugin")?,
        in_list(HOST_PORTS, port, "port")?,
    ))
}

fn channel_of(s: &str, at: usize) -> Result<usize, String> {
    let Some(idx) = s.strip_prefix("ch") else {
        return Err(format!("line {at}: bad channel '{s}' (want ch0..ch7)"));
    };
    let n: usize = idx
        .parse()
        .map_err(|_| format!("line {at}: bad channel '{s}'"))?;
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
    use std::panic::{AssertUnwindSafe, catch_unwind};

    /// No `Arrange` command has run, so there is no editor: `arrangement()` is
    /// `Ok(default)` — the legitimate "nothing built yet" case, never an error.
    #[test]
    fn arrangement_without_an_editor_is_ok_default() {
        let session = HostSession::new();
        assert_eq!(
            session
                .arrangement()
                .expect("no editor must be Ok, never Err"),
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
        session
            .ensure_editor()
            .expect("the editor registers its op handlers");
        let editor = session
            .editor
            .as_mut()
            .expect("ensure_editor built the editor");

        // A valid track + clip so the overflowing SetClipFade reaches the
        // overflow add (an absent clip would refuse before it).
        let mut engine = Engine::new(48_000, 120.0, 4);
        editor.register(&mut engine).expect("register op handlers");
        editor
            .apply(
                &mut engine,
                &media::ArrangeOp::AddTrack { track: "t0".into() },
            )
            .expect("add track");
        editor
            .apply(
                &mut engine,
                &media::ArrangeOp::AddClip {
                    track: "t0".into(),
                    clip: media::Clip {
                        reversed: false,
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
        assert!(
            poisoned,
            "the overflowing SetClipFade must panic (overflow check) to poison the lock"
        );

        // The session survived the contained panic; its snapshot now errors and
        // arrangement() must propagate that Err — never Ok(empty).
        let err = session
            .arrangement()
            .expect_err("a poisoned editor must Err, not return an empty Timeline");
        assert!(
            err.contains("poison"),
            "the error is the snapshot poison, got: {err}"
        );
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
        s.execute(&HostCommand::TransportSeek { frame: 4_800 })
            .expect("seek");
        let p = s.position();
        assert_eq!(p.frame, 4_800, "seek forward renders to the target");
        assert!((p.seconds - 0.1).abs() < 1e-9, "4800 frames @48k = 0.1 s");
        assert!((p.beat - 0.2).abs() < 1e-6, "120 bpm, 0.1 s = 0.2 beats");
        assert!(
            (p.bpm - 120.0).abs() < 1e-9,
            "the tempo map reports 120 bpm"
        );
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
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: Some(0),
        })
        .expect("add track");

        s.execute(&HostCommand::TransportSeek { frame: 9_600 })
            .expect("seek forward");
        assert_eq!(s.position().frame, 9_600);
        s.execute(&HostCommand::TransportSeek { frame: 1_000 })
            .expect("seek backward");
        assert_eq!(
            s.position().frame,
            1_000,
            "backward seek rebuilds to the target"
        );
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
        assert!(matches!(
            op,
            media::ArrangeOp::MoveClip {
                at_frame: 9_600,
                ..
            }
        ));
        assert_eq!(at, Some(48_000));

        let (op, at) =
            parse_arrange_line("trim t0 c0 end -24000").expect("parse without the keyword");
        assert!(matches!(
            op,
            media::ArrangeOp::Trim {
                by_frames: -24_000,
                ..
            }
        ));
        assert_eq!(at, None);

        assert!(parse_arrange_line("").is_err(), "an empty line is refused");
        assert!(
            parse_arrange_line("arrange nonsense t0").is_err(),
            "an unknown op is refused"
        );
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
                reversed: false,
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
        media::ArrangeOp::MoveClip {
            track: "t0".into(),
            clip: id.into(),
            at_frame: at,
        }
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
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("add track");
        s.execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("add clip");

        assert!(s.can_undo(), "edits are undoable");
        assert!(!s.can_redo(), "nothing is undone yet");

        // Play to 9600 so we can prove an undo does not rewind the playhead.
        s.execute(&HostCommand::TransportSeek { frame: 9_600 })
            .expect("seek");
        s.execute(&HostCommand::Arrange {
            op: move_clip("c0", 4_800),
            at_frame: None,
        })
        .expect("move");
        assert_eq!(
            s.arrangement().expect("tl").tracks[0].clips[0].at_frame,
            4_800
        );

        assert!(
            s.undo().expect("undo works"),
            "undo reports an edit was undone"
        );
        assert_eq!(
            s.arrangement().expect("tl").tracks[0].clips[0].at_frame,
            0,
            "the move is reverted"
        );
        assert_eq!(
            s.position().frame,
            9_600,
            "undo keeps the playhead where it was"
        );
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
        s.execute(&HostCommand::Arrange {
            op: move_clip("c0", 20_000),
            at_frame: None,
        })
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
    fn transport_lines_parse() {
        let cmds = parse_script("host v1\ntransport play\ntransport seek 4800\ntransport stop\n")
            .expect("transport lines parse");
        assert!(matches!(&cmds[0], HostCommand::TransportPlay));
        assert!(matches!(&cmds[2], HostCommand::TransportStop));
        match &cmds[1] {
            HostCommand::TransportSeek { frame } => assert_eq!(*frame, 4_800),
            other => panic!("expected a seek, got {other:?}"),
        }
    }

    // ---- gestures: one entry, one undo, all-or-nothing ----

    /// Build a session with a mixer, a pool holding one take, and one 4800-frame
    /// clip at frame 0 on `t0`.
    fn session_with_clip(name: &str) -> (HostSession, std::path::PathBuf) {
        let pool = std::env::temp_dir().join(format!("host-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_take(&pool, "s1", 48_000);
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mount mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("add track");
        s.execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("add clip");
        (s, pool)
    }

    /// A session with a mixer, a pool, one track and one clip at frame 0.
    fn session_with_pool(_name: &str, pool: &std::path::Path) -> HostSession {
        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mount mixer");
        s.execute(&HostCommand::Pool {
            dir: pool.to_path_buf(),
        })
        .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("add track");
        s.execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("add clip");
        s
    }

    fn clip_of(s: &HostSession) -> media::Clip {
        s.arrangement().expect("tl").tracks[0].clips[0].clone()
    }

    fn gesture(ops: Vec<media::ArrangeOp>) -> HostCommand {
        HostCommand::Group {
            commands: ops
                .into_iter()
                .map(|op| HostCommand::Arrange { op, at_frame: None })
                .collect(),
        }
    }

    /// **One gesture is one undo.** A group of two trims lands as a single history
    /// entry: one `undo` restores the clip whole (not half-trimmed), and one `redo`
    /// re-applies the whole gesture.
    #[test]
    fn a_group_is_one_undo_step() {
        let (mut s, pool) = session_with_clip("gesture-undo");
        let before = clip_of(&s);

        // Two edges move: the exact shape that used to take two undos.
        s.execute(&gesture(vec![
            media::ArrangeOp::Trim {
                track: "t0".into(),
                clip: "c0".into(),
                edge: media::Edge::Start,
                by_frames: 1_200,
            },
            media::ArrangeOp::Trim {
                track: "t0".into(),
                clip: "c0".into(),
                edge: media::Edge::End,
                by_frames: -600,
            },
        ]))
        .expect("the gesture applies");

        let after = clip_of(&s);
        assert_eq!(
            (after.at_frame, after.src_start, after.src_len),
            (1_200, 1_200, 3_000),
            "both trims landed"
        );

        assert!(s.undo().expect("undo works"), "the gesture is undoable");
        assert_eq!(clip_of(&s), before, "one undo restores the whole gesture");
        assert!(s.can_redo(), "and it can be redone");

        assert!(s.redo().expect("redo works"));
        assert_eq!(clip_of(&s), after, "redo re-applies the whole gesture");

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// **All-or-nothing.** A member the model refuses means no member is applied and
    /// nothing is logged — the session is exactly as it was.
    #[test]
    fn a_refused_group_changes_nothing() {
        let (mut s, pool) = session_with_clip("gesture-atomic");
        let before = clip_of(&s);
        let events = s.event_count();
        let commands = s.media_command_count();

        // The second trim would consume the whole clip — the model refuses it.
        let refused = s.execute(&gesture(vec![
            media::ArrangeOp::Trim {
                track: "t0".into(),
                clip: "c0".into(),
                edge: media::Edge::Start,
                by_frames: 1_200,
            },
            media::ArrangeOp::Trim {
                track: "t0".into(),
                clip: "c0".into(),
                edge: media::Edge::End,
                by_frames: -4_800,
            },
        ]));
        assert!(refused.is_err(), "the group is refused");
        let message = refused.expect_err("an error");
        assert!(
            message.contains("nothing applied"),
            "the refusal says so: {message}"
        );
        assert_eq!(clip_of(&s), before, "the first trim was rolled back");
        assert_eq!(s.event_count(), events, "and nothing was logged");
        assert_eq!(
            s.media_command_count(),
            commands,
            "no media command counted"
        );
        assert!(s.undo().is_ok(), "the session is still usable");

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// A gesture is one moment over arrangement ops: a non-arrange member, or members
    /// at different frames, is refused (a group must be skippable/appliable whole by
    /// `replay_to`).
    #[test]
    fn a_group_is_one_frame_of_arrangement_ops() {
        let (mut s, pool) = session_with_clip("gesture-shape");

        let mixed_frames = s.execute(&HostCommand::Group {
            commands: vec![
                HostCommand::Arrange {
                    op: move_clip("c0", 1_000),
                    at_frame: None,
                },
                HostCommand::Arrange {
                    op: move_clip("c0", 2_000),
                    at_frame: Some(4_800),
                },
            ],
        });
        assert!(mixed_frames.is_err(), "one gesture happens at one frame");

        let not_arrangement = s.execute(&HostCommand::Group {
            commands: vec![HostCommand::SetParam {
                plugin: "mixer",
                param: "master.gain",
                value: 0.5,
                at_frame: None,
            }],
        });
        assert!(not_arrangement.is_err(), "a group is arrangement ops only");

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// The text form round-trips gestures, and grouping changes the **host history**,
    /// not the engine's log: the same ops grouped and ungrouped produce the same
    /// events and the same audio.
    #[test]
    fn a_grouped_script_round_trips_and_logs_the_same_events() {
        let pool = std::env::temp_dir().join(format!("host-group-script-{}", std::process::id()));
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_take(&pool, "s1", 48_000);

        let head = format!(
            "host v1\nmount mixer channels=2 @0\npool {}\narrange add_track t0\narrange add_clip t0 c0 s1 0 4800 0 0 0 1.0\n",
            pool.display()
        );
        let grouped = format!(
            "{head}group begin\narrange trim t0 c0 start 1200\narrange trim t0 c0 end -600\ngroup end\n"
        );
        let loose = format!("{head}arrange trim t0 c0 start 1200\narrange trim t0 c0 end -600\n");

        // The markers parse into one Group command.
        let parsed = parse_script(&grouped).expect("the grouped script parses");
        match parsed.last().expect("a command") {
            HostCommand::Group { commands } => assert_eq!(commands.len(), 2, "two members"),
            other => panic!("expected a group, got {other:?}"),
        }

        let grouped_session = run_script(&parse_script(&grouped).expect("parse")).expect("run");
        let loose_session = run_script(&parse_script(&loose).expect("parse")).expect("run");
        assert_eq!(
            grouped_session.arrangement().expect("tl"),
            loose_session.arrangement().expect("tl"),
            "grouping does not change the value"
        );
        assert_eq!(
            grouped_session.event_count(),
            loose_session.event_count(),
            "grouping does not change the engine's log"
        );

        // …and it is one undo step, where the loose form takes two.
        let mut grouped_session = grouped_session;
        assert!(grouped_session.undo().expect("undo"));
        assert_eq!(
            clip_of(&grouped_session).src_len,
            4_800,
            "one undo restores the grouped gesture whole"
        );

        // Malformed group syntax is refused loudly.
        for bad in [
            "host v1\ngroup begin\narrange add_track t0\n",
            "host v1\ngroup end\n",
            "host v1\ngroup begin\ngroup begin\n",
            "host v1\ngroup begin\ngroup end\n",
        ] {
            assert!(parse_script(bad).is_err(), "refused: {bad:?}");
        }

        let _ = std::fs::remove_dir_all(&pool);
    }

    // ---- persistence: a session is a directory ----

    /// A tone take (not silence) so a bounce can be asserted *audible*.
    /// A 0.6 sine with a **spike** at `spike_at` (three frames to 1.4): the spike is
    /// the alignment marker an offline render must not shift, and the sine is what the
    /// compressor's steady-state gain reduction shows up in.
    fn write_spiked_tone(dir: &std::path::Path, id: &str, frames: usize, spike_at: usize) {
        let path = dir.join(format!("{id}.wav"));
        let mut w = media::WavWriter::create_float(&path, 48_000, 1).expect("wav writer");
        let samples: Vec<f32> = (0..frames)
            .map(|i| {
                let v = (i as f32 * 0.05).sin() * 0.6;
                if i >= spike_at && i < spike_at + 3 {
                    1.4
                } else {
                    v
                }
            })
            .collect();
        w.write(&samples).expect("write spiked tone");
        w.finalize().expect("finalize spiked tone");
    }

    fn write_tone(dir: &std::path::Path, id: &str, frames: usize, rate: u32) {
        let path = dir.join(format!("{id}.wav"));
        let mut w = media::WavWriter::create(&path, rate, 1).expect("wav writer");
        let samples: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        w.write(&samples).expect("write tone");
        w.finalize().expect("finalize tone");
    }

    fn bounce(session: &mut HostSession, frames: usize, path: &std::path::Path) -> Vec<f32> {
        session
            .execute(&HostCommand::Bounce {
                frames,
                path: path.to_path_buf(),
            })
            .expect("bounce");
        let mut r = media::WavReader::open(path).expect("bounce file");
        let mut audio = vec![0.0f32; r.total_frames() as usize];
        let n = r.read_into(&mut audio);
        audio.truncate(n);
        audio
    }

    /// The text form round-trips **every** state command — the property a saved
    /// session rests on. A new op or a changed operand must be taught to the
    /// formatter, and this is where it fails.
    #[test]
    fn the_text_form_round_trips_every_state_command() {
        let clip = media::Clip {
            reversed: false,
            id: "c0".into(),
            source: "s1".into(),
            src_start: 100,
            src_len: 4_800,
            at_frame: 200,
            fade_in: 10,
            fade_out: 20,
            gain: 0.75,
            loop_len: Some(2_400),
        };
        let ops = vec![
            media::ArrangeOp::AddTrack { track: "t0".into() },
            media::ArrangeOp::AddClip {
                track: "t0".into(),
                clip: clip.clone(),
            },
            media::ArrangeOp::RazorSplit {
                track: "t0".into(),
                clip: "c0".into(),
                new_left: "L".into(),
                new_right: "R".into(),
                at_frame: 2_400,
            },
            media::ArrangeOp::Trim {
                track: "t0".into(),
                clip: "c0".into(),
                edge: media::Edge::Start,
                by_frames: -120,
            },
            media::ArrangeOp::MoveClip {
                track: "t0".into(),
                clip: "c0".into(),
                at_frame: 9_600,
            },
            media::ArrangeOp::MoveClipToTrack {
                from: "t0".into(),
                clip: "c0".into(),
                to: "t1".into(),
                at_frame: 9_600,
            },
            media::ArrangeOp::Duplicate {
                track: "t0".into(),
                clip: "c0".into(),
                new_id: "c1".into(),
            },
            media::ArrangeOp::Delete {
                track: "t0".into(),
                clip: "c1".into(),
            },
            media::ArrangeOp::SetClipGain {
                track: "t0".into(),
                clip: "c0".into(),
                gain: 0.5,
            },
            media::ArrangeOp::SetClipFade {
                track: "t0".into(),
                clip: "c0".into(),
                fade_in: 64,
                fade_out: 128,
            },
            media::ArrangeOp::LoopRegion {
                track: "t0".into(),
                clip: "c0".into(),
                times: 3,
            },
            media::ArrangeOp::ChopClip {
                track: "t0".into(),
                clip: "c0".into(),
                times: 4,
                prefix: "pre".into(),
            },
            media::ArrangeOp::RemoveTrack { track: "t1".into() },
            media::ArrangeOp::RenameTrack {
                track: "t0".into(),
                to: "lead".into(),
            },
            media::ArrangeOp::MoveTrack {
                track: "t0".into(),
                index: 1,
            },
            media::ArrangeOp::Reverse {
                track: "t0".into(),
                clip: "c0".into(),
            },
        ];

        let mut commands: Vec<HostCommand> = vec![
            HostCommand::Mount {
                plugin: "mixer",
                params: vec![("channels", 2.0)],
                at_frame: Some(0),
            },
            HostCommand::Mount {
                plugin: "tone",
                params: vec![("gain", 0.25), ("blip_len", 1_800.0)],
                at_frame: None,
            },
            HostCommand::Patch {
                from: ("euclidean", "triggers"),
                to: ("scale", "trigger"),
                at_frame: Some(0),
            },
            HostCommand::SetParam {
                plugin: "mixer",
                param: "ch0.gain",
                value: 0.7,
                at_frame: None,
            },
            HostCommand::SetTempo {
                bpm: 137.5,
                beats_per_bar: 3,
                at_frame: Some(0),
            },
            HostCommand::Unmount {
                plugin: "tone",
                at_frame: Some(4_800),
            },
            HostCommand::Pool {
                dir: PathBuf::from("/tmp/pool"),
            },
            HostCommand::SessionRate { hz: 48_000 },
            HostCommand::Play {
                clip: media::ClipRef {
                    path: PathBuf::from("/tmp/a.wav"),
                    start: 0,
                    len: 0,
                },
                channel: 1,
                at_frame: Some(0),
            },
            HostCommand::Splice {
                at_frame: 4_000,
                clip: media::ClipRef {
                    path: PathBuf::from("/tmp/b.wav"),
                    start: 0,
                    len: 0,
                },
                crossfade: 512,
            },
        ];
        commands.extend(
            ops.into_iter()
                .map(|op| HostCommand::Arrange { op, at_frame: None }),
        );
        commands.push(HostCommand::Group {
            commands: vec![
                HostCommand::Arrange {
                    op: media::ArrangeOp::Trim {
                        track: "t0".into(),
                        clip: "c0".into(),
                        edge: media::Edge::Start,
                        by_frames: 100,
                    },
                    at_frame: None,
                },
                HostCommand::Arrange {
                    op: media::ArrangeOp::Trim {
                        track: "t0".into(),
                        clip: "c0".into(),
                        edge: media::Edge::End,
                        by_frames: -100,
                    },
                    at_frame: None,
                },
            ],
        });

        let mut text = String::from("host v1\n");
        for cmd in &commands {
            let line = format_command(cmd, None).expect("a state command has a text form");
            text.push_str(&line);
            text.push('\n');
        }
        let back = parse_script(&text).expect("the formatted script parses");
        assert_eq!(
            back, commands,
            "the text form round-trips every state command"
        );

        // A pure action has no text form, so it can never leak into a log.
        for action in [
            HostCommand::TransportPlay,
            HostCommand::TransportStop,
            HostCommand::Undo,
            HostCommand::Redo,
            HostCommand::Record {
                take_id: "t".into(),
            },
            HostCommand::Bounce {
                frames: 1,
                path: PathBuf::from("/tmp/x.wav"),
            },
            HostCommand::Save {
                dir: PathBuf::from("/tmp/x"),
            },
            HostCommand::Load {
                dir: PathBuf::from("/tmp/x"),
            },
        ] {
            assert!(
                format_command(&action, None).is_none(),
                "{action:?} must not be serializable"
            );
        }
    }

    /// Save → load reproduces the session: the same value, the same folded
    /// parameters, the same tempo, and a **byte-identical bounce**.
    #[test]
    fn save_and_load_round_trips_a_session() {
        let root = std::env::temp_dir().join(format!("host-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("work");
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_tone(&pool, "s1", 48_000, 48_000);

        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::SetTempo {
            bpm: 96.0,
            beats_per_bar: 4,
            at_frame: Some(0),
        })
        .expect("tempo");
        s.execute(&HostCommand::SetParam {
            plugin: "mixer",
            param: "ch0.gain",
            value: 0.6,
            at_frame: None,
        })
        .expect("gain");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        s.execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("clip");
        // A gesture, so the save has to carry group structure too.
        s.execute(&gesture(vec![
            media::ArrangeOp::Trim {
                track: "t0".into(),
                clip: "c0".into(),
                edge: media::Edge::Start,
                by_frames: 1_200,
            },
            media::ArrangeOp::Trim {
                track: "t0".into(),
                clip: "c0".into(),
                edge: media::Edge::End,
                by_frames: -600,
            },
        ]))
        .expect("gesture");

        let dir = root.join("mysong.d");
        s.save(&dir).expect("save");
        assert!(dir.join("session.txt").is_file(), "the script is written");
        assert!(
            dir.join("pool/s1.wav").is_file(),
            "the pool travels with it"
        );
        assert_eq!(s.session_dir(), Some(dir.as_path()));

        let mut loaded = HostSession::new();
        loaded.load_session(&dir).expect("load");
        assert_eq!(
            loaded.arrangement().expect("tl"),
            s.arrangement().expect("tl"),
            "the arrangement round-trips"
        );
        // A `set_tempo` is *scheduled* at its frame, so it takes effect when the clock
        // renders — the bounce below is what applies it.
        assert_eq!(loaded.position().bpm, 120.0, "nothing rendered yet");
        let gain = loaded
            .params()
            .iter()
            .find(|(p, k, _)| *p == "mixer" && *k == "ch0.gain")
            .map(|(_, _, v)| *v);
        assert_eq!(
            gain,
            Some(0.6),
            "the parameters round-trip (folded from the log)"
        );

        // …and the audio is identical, which is the property that matters.
        let a = bounce(&mut s, 4_800, &root.join("a.wav"));
        let b = bounce(&mut loaded, 4_800, &root.join("b.wav"));
        assert!(a.iter().any(|x| x.abs() > 1e-3), "the take is audible");
        assert_eq!(a, b, "save → load bounces the same audio");
        assert_eq!(s.position().bpm, 96.0, "the tempo applied on render");
        assert_eq!(
            loaded.position().bpm,
            96.0,
            "and the loaded session's tempo matches"
        );

        // A reloaded session is a *session*, not just a value: it can be saved again,
        // and the new script still holds the baseline (the load built the history).
        let again = root.join("again.d");
        loaded.save(&again).expect("save again");
        let text = std::fs::read_to_string(again.join("session.txt")).expect("second script");
        assert!(
            text.contains("arrange add_clip t0 c0 s1"),
            "the baseline is in the reloaded history:\n{text}"
        );
        assert!(text.contains("set_tempo 96"), "{text}");

        // A gesture is still one undo step after a reload.
        let before = clip_of(&loaded);
        loaded
            .execute(&gesture(vec![media::ArrangeOp::MoveClip {
                track: "t0".into(),
                clip: "c0".into(),
                at_frame: 24_000,
            }]))
            .expect("move");
        assert!(loaded.undo().expect("undo"));
        assert_eq!(clip_of(&loaded), before);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The session directory is **movable**: the script's pool path is relative, so
    /// renaming the directory keeps the session playable (the plan's path trap).
    /// **The stretch round trip**: a clip's material is rendered into a new pool
    /// source at a rational ratio, the logged op points the clip at it, the pitch is
    /// preserved while the length grows, one undo restores the old reference, and the
    /// deterministic id means stretching twice reuses the same material instead of
    /// piling up copies.
    #[test]
    fn stretching_a_clip_materialises_a_pool_source() {
        let root = std::env::temp_dir().join(format!("host-stretch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("pool");
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_tone(&pool, "s1", 48_000, 48_000);

        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        s.execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("clip");

        // A source tempo is state: the log carries it, the outcome exposes it.
        s.execute(&HostCommand::SetSourceTempo {
            source: "s1".into(),
            bpm: 90.0,
        })
        .expect("source tempo");
        assert_eq!(s.source_tempos().get("s1"), Some(&90.0));

        // Refusals first: a 1:1 ratio copies rather than stretches, a zero is not a
        // ratio, and an unbounded one is refused *before* the allocator sees it (the
        // gate executed `stretch t0 c0 4294967295 1` and the process died with an
        // 824 TB allocation failure — a logged line must not be able to do that).
        assert!(s.stretch("t0", "c0", 1, 1).is_err(), "1:1 is refused");
        assert!(
            s.stretch("t0", "c0", 0, 1).is_err(),
            "a zero ratio is refused"
        );
        assert!(
            s.stretch("t0", "c0", u32::MAX, 1).is_err(),
            "a ratio beyond the host's limit is refused, not attempted"
        );
        assert!(
            s.stretch("t0", "c0", 1, u32::MAX).is_err(),
            "and so is an absurd denominator"
        );

        let before = s.arrangement().expect("arrangement").tracks[0].clips[0].clone();
        s.stretch("t0", "c0", 3, 2).expect("stretch");
        let after = s.arrangement().expect("arrangement").tracks[0].clips[0].clone();
        assert_eq!(
            after.source, "s1.stretch.0_4800.3_2",
            "the clip points at the render"
        );
        assert_eq!(after.src_start, 0);
        assert_eq!(
            after.at_frame, before.at_frame,
            "its place in time is untouched"
        );
        assert!(
            after.src_len > before.src_len,
            "the material grew: {} → {}",
            before.src_len,
            after.src_len
        );

        // The material is in the pool, complete, with its peaks.
        let index = media::Pool::open(&pool)
            .expect("pool")
            .list()
            .expect("list");
        let rendered = index
            .sources
            .iter()
            .find(|source| source.id == "s1.stretch.0_4800.3_2")
            .expect("the render is a pool source");
        assert_eq!(
            rendered.frames, after.src_len,
            "the log's length is the file's"
        );
        assert!(!rendered.peaks_missing && rendered.finalized);
        assert_eq!(rendered.sample_rate, 48_000);

        // **Pitch is preserved**: the zero-crossing *rate* is the original's, while the
        // duration grew (a resample would have raised both together).
        let crossings = |path: &std::path::Path| -> (usize, u64) {
            let mut r = media::WavReader::open(path)
                .expect("wav")
                .with_channel(0)
                .expect("mono");
            let frames = r.total_frames();
            let mut buf = vec![0.0f32; frames as usize];
            let n = r.read_into(&mut buf);
            buf.truncate(n);
            let c = buf
                .windows(2)
                .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
                .count();
            (c, frames)
        };
        let (c0, f0) = crossings(&pool.join("s1.wav"));
        let (c1, f1) = crossings(&pool.join("s1.stretch.0_4800.3_2.wav"));
        let rate = |c: usize, f: u64| c as f64 / f as f64;
        assert!(
            (rate(c1, f1) - rate(c0, f0)).abs() < 0.05 * rate(c0, f0),
            "the pitch must be preserved: {} → {} crossings/frame",
            rate(c0, f0),
            rate(c1, f1)
        );
        // The *clip's region* is what was stretched (the fixture's clip reads 4 800
        // frames of the 48 000-frame tone), and the output is window-aligned.
        let expect = (before.src_len as f64 * 1.5) as u64;
        assert!(
            (f1 as f64 - expect as f64).abs() < 2.5 * 1024.0,
            "the length must follow the ratio: {} frames → {f1}, expected ~{expect}",
            before.src_len
        );
        let _ = f0;

        // One undo restores the old reference (the material is working material and
        // stays in the pool).
        s.execute(&HostCommand::Undo).expect("undo");
        let undone = s.arrangement().expect("arrangement").tracks[0].clips[0].clone();
        assert_eq!(undone.source, "s1");
        assert_eq!(undone.src_len, before.src_len);

        // The same material at the same ratio reuses the same source id (the id is
        // deterministic), so stretching, undoing and stretching again does not pile up
        // copies. (Stretching the *already stretched* clip renders new material — that
        // is a different input, and its own deterministic id says so.)
        s.stretch("t0", "c0", 3, 2).expect("stretch again");
        assert_eq!(
            s.arrangement().expect("arrangement").tracks[0].clips[0].source,
            "s1.stretch.0_4800.3_2"
        );
        let copies = media::Pool::open(&pool)
            .expect("pool")
            .list()
            .expect("list")
            .sources
            .iter()
            .filter(|source| source.id == "s1.stretch.0_4800.3_2")
            .count();
        assert_eq!(copies, 1, "one render, reused");

        // `source_tempo` is **state**: it survives a save/load like the rest of the log
        // (the saved script carries the line, and replaying it rebuilds the map).
        let dir = root.join("session");
        s.execute(&HostCommand::Save { dir: dir.clone() })
            .expect("save");
        let mut reloaded = HostSession::new();
        reloaded.execute(&HostCommand::Load { dir }).expect("load");
        assert_eq!(
            reloaded.source_tempos().get("s1"),
            Some(&90.0),
            "the recorded tempo is part of the document"
        );
        assert_eq!(
            reloaded.arrangement().expect("arrangement").tracks[0].clips[0].source,
            "s1.stretch.0_4800.3_2",
            "and the stretch replays as a reference, not a re-render"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A stretch id keys on the region, not just the source.** Two clips of one pool
    /// source at different offsets are *different material*: an id of
    /// `{source}.stretch.{num}_{den}` makes the second render silently overwrite the
    /// first clip's file, and the first clip goes quiet (reproduced before the fix: the
    /// master dropped from a 0.354 peak to silence). The fixture is half tone, half
    /// silence so the two renders are distinguishable by content.
    #[test]
    fn a_stretch_id_keys_on_the_region_not_the_source() {
        let root = std::env::temp_dir().join(format!("host-stretch-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("pool");
        std::fs::create_dir_all(&pool).expect("pool dir");

        let path = pool.join("s2.wav");
        let mut w = media::WavWriter::create(&path, 48_000, 1).expect("wav writer");
        let samples: Vec<f32> = (0..9_600)
            .map(|i| {
                if i < 4_800 {
                    (i as f32 * 0.05).sin() * 0.5
                } else {
                    0.0
                }
            })
            .collect();
        w.write(&samples).expect("write fixture");
        w.finalize().expect("finalize fixture");

        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        for (id, at, src_start) in [("loud", 0u64, 0u64), ("quiet", 9_600, 4_800)] {
            s.execute(&HostCommand::Arrange {
                op: media::ArrangeOp::AddClip {
                    track: "t0".into(),
                    clip: media::Clip {
                        reversed: false,
                        id: id.into(),
                        source: "s2".into(),
                        src_start,
                        src_len: 4_800,
                        at_frame: at,
                        fade_in: 0,
                        fade_out: 0,
                        gain: 1.0,
                        loop_len: None,
                    },
                },
                at_frame: None,
            })
            .expect("clip");
        }

        s.stretch("t0", "loud", 3, 2)
            .expect("stretch the loud clip");
        s.stretch("t0", "quiet", 3, 2)
            .expect("stretch the quiet clip");

        let timeline = s.arrangement().expect("arrangement");
        let source_of = |id: &str| {
            timeline.tracks[0]
                .clips
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.source.clone())
                .expect("clip")
        };
        assert_eq!(source_of("loud"), "s2.stretch.0_4800.3_2");
        assert_eq!(source_of("quiet"), "s2.stretch.4800_4800.3_2");

        let peak = |path: &std::path::Path| -> f32 {
            let mut r = media::WavReader::open(path).expect("wav");
            let mut buf = vec![0.0f32; r.total_frames() as usize];
            let n = r.read_into(&mut buf);
            buf.truncate(n);
            buf.iter().fold(0.0f32, |m, x| m.max(x.abs()))
        };
        assert!(
            peak(&pool.join("s2.stretch.0_4800.3_2.wav")) > 0.1,
            "the loud clip's render must still be its own material"
        );
        assert_eq!(
            peak(&pool.join("s2.stretch.4800_4800.3_2.wav")),
            0.0,
            "the quiet clip's render is silence — and, crucially, a *different* file"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The mastering chain is a graph node on the bus.** Mounted after the mixer and
    /// patched from it with a stereo cord, it claims the output, so the live pump and
    /// the bounce both flow through it. A loud take proves the compressor pulls the
    /// material down, the limiter holds the ceiling on a spike, the bounce is
    /// **latency-aligned** (a spike at the clip's frame 0 is at frame 0 of the file, not
    /// after the lookahead), and it is byte-identical across runs. Unmounting restores
    /// the mixer as the bus owner.
    #[test]
    fn the_master_chain_sits_on_the_bus_and_bounces_aligned() {
        let root = std::env::temp_dir().join(format!("host-master-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("pool");
        std::fs::create_dir_all(&pool).expect("pool dir");
        // A 0.6 sine with a **spike** at frame 5 000: the clip starts at 5 000, so the
        // spike lands on the clip's frame 0 — the alignment marker — and the steady sine
        // is what the compressor's gain reduction shows up in.
        write_spiked_tone(&pool, "s1", 48_000, 5_000);

        let build = |with_master: bool| -> HostSession {
            let mut s = HostSession::new();
            s.execute(&HostCommand::Mount {
                plugin: "mixer",
                params: vec![("channels", 2.0)],
                at_frame: Some(0),
            })
            .expect("mixer");
            if with_master {
                s.execute(&HostCommand::Mount {
                    plugin: "master",
                    params: vec![],
                    at_frame: Some(0),
                })
                .expect("master");
                s.execute(&HostCommand::Patch {
                    from: ("mixer", "audio"),
                    to: ("master", "audio"),
                    at_frame: Some(0),
                })
                .expect("patch the bus");
                for (param, value) in [
                    ("threshold", -24.0),
                    ("ratio", 8.0),
                    ("attack_ms", 1.0),
                    ("ceiling", -6.0),
                ] {
                    s.execute(&HostCommand::SetParam {
                        plugin: "master",
                        param,
                        value,
                        at_frame: Some(0),
                    })
                    .expect("master param");
                }
            }
            s.execute(&HostCommand::Pool { dir: pool.clone() })
                .expect("pool");
            s.execute(&HostCommand::Arrange {
                op: media::ArrangeOp::AddTrack { track: "t0".into() },
                at_frame: None,
            })
            .expect("track");
            let mut clip = match add_clip("c0", 0) {
                media::ArrangeOp::AddClip { clip, .. } => clip,
                other => panic!("add_clip built {other:?}"),
            };
            clip.src_start = 5_000;
            // Long enough that a bounce *after* the two 16 000-frame renders still has
            // material (the unmount check below renders from the advanced position).
            clip.src_len = 40_000;
            s.execute(&HostCommand::Arrange {
                op: media::ArrangeOp::AddClip {
                    track: "t0".into(),
                    clip,
                },
                at_frame: None,
            })
            .expect("clip");
            s
        };

        let mut dry_session = build(false);
        let dry = bounce(&mut dry_session, 16_000, &root.join("dry.wav"));

        let mut s = build(true);
        let path = root.join("mix.wav");
        // The mount and its params are *scheduled*: the first render applies them.
        let audio = bounce(&mut s, 16_000, &path);
        assert!(
            s.master_meters().is_some(),
            "the mastering stage publishes its meters while mounted"
        );

        // The brickwall: the spike is caught (the dry file carries it well above the
        // ceiling; the master file cannot).
        let ceiling = 10f32.powf(-6.0 / 20.0);
        let peak = audio.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let dry_peak = dry.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(
            dry_peak > ceiling * 1.5,
            "the fixture must exceed the ceiling dry: {dry_peak} vs {ceiling}"
        );
        assert!(
            peak <= ceiling + 1e-4,
            "the brickwall holds on the bounce: {peak} vs {ceiling}"
        );
        assert!(
            peak > ceiling * 0.9,
            "and the spike is not ducked to nothing: {peak}"
        );

        // **Aligned**: the spike is at frame 0 of the file, exactly where the dry
        // bounce has it — not 240+ frames late, and not preceded by the lookahead's
        // silence.
        assert!(
            audio[0].abs() > 0.3 && audio[1].abs() > 0.3,
            "the spike is at frame 0, both sides: {:?}",
            &audio[..4]
        );

        // The compressor works on the steady material: the second half of the master
        // bounce is clearly quieter than the dry one (the spike's first block aside).
        let rms = |v: &[f32], from: usize, to: usize| -> f32 {
            let s: f32 = v[from..to].iter().map(|x| x * x).sum();
            (s / (to - from) as f32).sqrt()
        };
        let (m, d) = (rms(&audio, 8_000, 15_000), rms(&dry, 8_000, 15_000));
        assert!(
            m < d * 0.6,
            "the compressor pulls the mix down: master rms {m} vs dry {d}"
        );

        assert!(
            (audio.len() as i64 - 16_000).abs() < 2_000,
            "the file is the piece (plus tails), not the piece plus latency: {} frames",
            audio.len()
        );
        assert_eq!(
            s.engine.graph.out_channels(),
            2,
            "the master owns the stereo bus"
        );

        // Deterministic: rewind and render the same window — byte-identical (the chain
        // is pure DSP, and the alignment is computed the same way twice).
        s.execute(&HostCommand::TransportSeek { frame: 0 })
            .expect("rewind");
        let again = bounce(&mut s, 16_000, &root.join("mix2.wav"));
        assert_eq!(audio, again, "the mastering chain is deterministic");

        // Unmounting restores the previous bus owner — dropping the mastering stage
        // must not leave the graph silent. (The unmount applies at the next render.)
        s.execute(&HostCommand::Unmount {
            plugin: "master",
            at_frame: None,
        })
        .expect("unmount master");
        let dry_again = bounce(&mut s, 4_800, &root.join("dry2.wav"));
        assert!(s.master_meters().is_none(), "the meters go with the plugin");
        assert_eq!(
            s.engine.graph.out_channels(),
            2,
            "the mixer still owns the bus after the master is unmounted"
        );
        assert!(
            dry_again.iter().any(|x| x.abs() > 0.01),
            "and the bus still carries audio (the mixer was restored as out)"
        );
        // The same material again, but through the mixer alone: the compressed level
        // was a fraction of the dry one, so a restored dry path proves the chain is out
        // of the way (not merely silent).
        let dry_again_peak = dry_again.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let compressed_peak = rms(&audio, 8_000, 15_000) * 4.0; // a generous multiple
        assert!(
            dry_again_peak > compressed_peak,
            "the dry path is back and uncompressed: {dry_again_peak} vs a compressed ~{compressed_peak}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A region the clip only *declares* is not allocated.** `src_len` is the clip's
    /// window, not a fact about the file (`validate_clip` bounds it at `i64::MAX`, not at
    /// the source's length), so a clip claiming ten billion frames must read what the
    /// source has and stretch *that* — the id keys on the material actually rendered, so
    /// two clips whose declared windows both run off the end of the same file share one
    /// render. (The gate found the neighbouring hazard: an unbounded *ratio* reached the
    /// allocator and killed the process; this is the same class of bug on the input side.)
    #[test]
    fn a_declared_region_longer_than_the_source_is_read_to_the_end() {
        let (mut s, pool) = session_with_clip("stretch-region");
        let mut long = match add_clip("c9", 9_600) {
            media::ArrangeOp::AddClip { clip, .. } => clip,
            other => panic!("add_clip built {other:?}"),
        };
        long.src_len = 10_000_000_000;
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddClip {
                track: "t0".into(),
                clip: long,
            },
            at_frame: None,
        })
        .expect("a clip may declare more than the file holds");

        s.stretch("t0", "c9", 3, 2)
            .expect("read to the end, then stretch");
        let c9 = s.arrangement().expect("arrangement").tracks[0]
            .clips
            .iter()
            .find(|c| c.id == "c9")
            .cloned()
            .expect("c9");
        assert_eq!(
            c9.source, "s1.stretch.0_48000.3_2",
            "the id is the material that was rendered (48 000 frames), not the declared length"
        );
        assert!(
            media::Pool::open(&pool)
                .expect("pool")
                .path_for(&c9.source)
                .is_some(),
            "and the render is really in the pool"
        );

        let _ = std::fs::remove_dir_all(&pool);
    }

    /// **A rename and a reorder survive the round trip.** Found by the slice's gate:
    /// the note claimed this end-to-end but only the format/parse round-trip was
    /// committed — which never *applies* the ops. This saves a session with
    /// `rename_track` + `move_track`, loads it back, and checks the order (the mixer
    /// channel each track feeds is its index, so the order is the thing that matters).
    #[test]
    fn a_renamed_and_reordered_session_reloads_in_order() {
        let root = std::env::temp_dir().join(format!("host-tracks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("pool");
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_tone(&pool, "s1", 48_000, 48_000);

        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 4.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        for track in ["t0", "t1"] {
            s.execute(&HostCommand::Arrange {
                op: media::ArrangeOp::AddTrack {
                    track: track.into(),
                },
                at_frame: None,
            })
            .expect("track");
        }
        s.execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("clip");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::RenameTrack {
                track: "t0".into(),
                to: "lead".into(),
            },
            at_frame: None,
        })
        .expect("rename");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::MoveTrack {
                track: "lead".into(),
                index: 1,
            },
            at_frame: None,
        })
        .expect("reorder");

        let order = |s: &HostSession| -> Vec<String> {
            s.arrangement()
                .expect("arrangement")
                .tracks
                .iter()
                .map(|track| track.id.clone())
                .collect()
        };
        assert_eq!(order(&s), vec!["t1", "lead"]);

        let dir = root.join("tracks.d");
        s.save(&dir).expect("save");

        // The saved text names both ops (they are state, so they are in the log)…
        let script = std::fs::read_to_string(dir.join("session.txt")).expect("script");
        assert!(
            script.contains("rename_track t0 lead"),
            "the rename is in the session:\n{script}"
        );
        assert!(
            script.contains("move_track lead 1"),
            "the reorder is in the session:\n{script}"
        );

        // …and loading it back reproduces the same order and the same clip.
        let mut reloaded = HostSession::new();
        reloaded.load_session(&dir).expect("load");
        assert_eq!(
            order(&reloaded),
            vec!["t1", "lead"],
            "the order round-trips"
        );
        let arrangement = reloaded.arrangement().expect("arrangement");
        assert_eq!(
            arrangement.tracks[1].clips.len(),
            1,
            "the clip is in `lead`"
        );
        assert_eq!(arrangement.tracks[1].clips[0].id, "c0");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_saved_session_can_be_moved() {
        let root = std::env::temp_dir().join(format!("host-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("elsewhere");
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_tone(&pool, "s1", 48_000, 48_000);

        let (mut s, _) = {
            let mut s = HostSession::new();
            s.execute(&HostCommand::Mount {
                plugin: "mixer",
                params: vec![("channels", 2.0)],
                at_frame: Some(0),
            })
            .expect("mixer");
            s.execute(&HostCommand::Pool { dir: pool.clone() })
                .expect("pool");
            s.execute(&HostCommand::Arrange {
                op: media::ArrangeOp::AddTrack { track: "t0".into() },
                at_frame: None,
            })
            .expect("track");
            s.execute(&HostCommand::Arrange {
                op: add_clip("c0", 0),
                at_frame: None,
            })
            .expect("clip");
            (s, ())
        };

        let dir = root.join("take1.d");
        s.save(&dir).expect("save");
        let script = std::fs::read_to_string(dir.join("session.txt")).expect("script");
        assert!(
            script.contains("pool pool"),
            "the pool path is relative to the session:\n{script}"
        );
        let reference = bounce(&mut s, 4_800, &root.join("ref.wav"));

        // Move the whole session elsewhere and load it there.
        let moved = root.join("moved.d");
        std::fs::rename(&dir, &moved).expect("rename the session dir");
        let _ = std::fs::remove_dir_all(&pool); // the original pool is gone

        let mut loaded = HostSession::new();
        loaded.load_session(&moved).expect("load from the new path");
        let audio = bounce(&mut loaded, 4_800, &root.join("moved.wav"));
        assert!(
            audio.iter().any(|x| x.abs() > 1e-3),
            "still audible after a move"
        );
        assert_eq!(audio, reference, "and identical");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The journal is the autosave: every committed gesture is appended (with its
    /// group markers), and a torn trailing line — a crash mid-write — is dropped on
    /// load and reported, never a parse error that costs the session.
    #[test]
    fn the_journal_autosaves_and_a_torn_line_is_dropped() {
        let root = std::env::temp_dir().join(format!("host-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("pool");
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_tone(&pool, "s1", 48_000, 48_000);

        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        s.execute(&HostCommand::Arrange {
            op: add_clip("c0", 0),
            at_frame: None,
        })
        .expect("clip");

        let dir = root.join("song.d");
        s.save(&dir).expect("save");
        assert_eq!(
            std::fs::read_to_string(dir.join("journal.txt")).expect("journal"),
            "",
            "a save resets the journal to its baseline"
        );

        // An edit after the save is autosaved, gestures included.
        s.execute(&gesture(vec![
            media::ArrangeOp::Trim {
                track: "t0".into(),
                clip: "c0".into(),
                edge: media::Edge::Start,
                by_frames: 1_200,
            },
            media::ArrangeOp::Trim {
                track: "t0".into(),
                clip: "c0".into(),
                edge: media::Edge::End,
                by_frames: -600,
            },
        ]))
        .expect("gesture");
        let journal = std::fs::read_to_string(dir.join("journal.txt")).expect("journal");
        assert!(
            journal.contains("group begin"),
            "the gesture is bracketed:\n{journal}"
        );
        assert!(
            journal.contains("arrange trim t0 c0 start 1200"),
            "{journal}"
        );
        assert!(journal.contains("group end"), "{journal}");
        assert!(s.journal_error().is_none(), "no journal error");

        // Simulate a crash mid-append: a half-written line with no newline.
        let mut torn = std::fs::read_to_string(dir.join("journal.txt")).expect("journal");
        torn.push_str("arrange trim t0 c0 start 99");
        std::fs::write(dir.join("journal.txt"), torn).expect("torn journal");

        let mut loaded = HostSession::new();
        loaded.load_session(&dir).expect("load with a torn journal");
        let recovery = loaded.last_recovery().cloned().expect("a recovery report");
        assert_eq!(recovery.torn_lines, 1, "the torn line is reported");
        assert_eq!(
            recovery.applied, 1,
            "the intact journal held one gesture (one command)"
        );
        assert_eq!(
            clip_of(&loaded).src_len,
            3_000,
            "the journal's gesture survived"
        );

        // The harder crash: cut *inside* a gesture — one member on disk, no `group
        // end`. The incomplete gesture is dropped (never half-applied) and the
        // session still opens.
        std::fs::write(
            dir.join("journal.txt"),
            "group begin\narrange trim t0 c0 start 1200\n",
        )
        .expect("mid-gesture journal");
        let mut loaded = HostSession::new();
        loaded
            .load_session(&dir)
            .expect("a gesture cut mid-write must not make the session unopenable");
        let recovery = loaded.last_recovery().cloned().expect("a recovery report");
        assert_eq!(recovery.applied, 0, "the incomplete gesture was dropped");
        assert_eq!(recovery.torn_lines, 2, "both of its lines are reported");
        assert_eq!(
            clip_of(&loaded).src_len,
            4_800,
            "and the clip is untouched — not half-trimmed"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A session's rate is context: `session_rate` round-trips, the loaded session
    /// runs at that rate, and a live session refuses to *change* rate.
    #[test]
    fn a_session_at_another_rate_round_trips() {
        let root = std::env::temp_dir().join(format!("host-rate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("pool");
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_tone(&pool, "s1", 44_100, 44_100);

        let script = format!(
            "host v1\nsession_rate 44100\nmount mixer channels=2 @0\npool {}\narrange add_track t0\narrange add_clip t0 c0 s1 0 44100 0 0 0 1.0\n",
            pool.display()
        );
        let mut s = run_script(&parse_script(&script).expect("parse")).expect("run");
        assert_eq!(s.sample_rate(), 44_100);

        let dir = root.join("cd.d");
        s.save(&dir).expect("save");
        let mut loaded = HostSession::new();
        loaded.load_session(&dir).expect("load");
        assert_eq!(loaded.sample_rate(), 44_100, "the rate round-trips");

        let audio = bounce(&mut loaded, 4_800, &root.join("cd.wav"));
        assert!(audio.iter().any(|x| x.abs() > 1e-3), "and it plays");
        let r = media::WavReader::open(&root.join("cd.wav")).expect("bounce");
        assert_eq!(r.sample_rate(), 44_100, "the bounce is at the session rate");

        // A live session cannot change rate — the clock and every frame derive from it.
        assert!(s.execute(&HostCommand::SessionRate { hz: 48_000 }).is_err());
        assert!(s.execute(&HostCommand::SessionRate { hz: 44_100 }).is_ok());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The gate's finding #1**: a rebuild (undo, redo, seek) used to drop the
    /// session directory, so autosave silently stopped after the first undo. The
    /// journal must keep growing through both.
    #[test]
    fn autosave_survives_an_undo_and_a_seek() {
        let root = std::env::temp_dir().join(format!("host-autosave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("pool");
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_tone(&pool, "s1", 48_000, 48_000);

        let mut s = session_with_pool("autosave", &pool);
        let dir = root.join("song.d");
        s.save(&dir).expect("save");
        assert_eq!(s.session_dir(), Some(dir.as_path()));

        let journal = || std::fs::read_to_string(dir.join("journal.txt")).expect("journal");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::MoveClip {
                track: "t0".into(),
                clip: "c0".into(),
                at_frame: 4_800,
            },
            at_frame: None,
        })
        .expect("move");
        assert!(journal().contains("move_clip t0 c0 4800"), "{}", journal());
        let after_move = journal().len();

        // Undo and seek both rebuild the session — the directory must survive.
        assert!(s.undo().expect("undo"));
        assert_eq!(
            s.session_dir(),
            Some(dir.as_path()),
            "an undo must not lose the session directory"
        );
        assert!(s.journal_error().is_none());
        s.execute(&HostCommand::TransportSeek { frame: 2_400 })
            .expect("seek");
        assert_eq!(
            s.session_dir(),
            Some(dir.as_path()),
            "a seek must not lose the session directory"
        );

        // …so the next edit is autosaved, and the journal has grown.
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::SetClipGain {
                track: "t0".into(),
                clip: "c0".into(),
                gain: 0.5,
            },
            at_frame: None,
        })
        .expect("gain");
        let text = journal();
        assert!(text.contains("set_clip_gain t0 c0 0.5"), "{text}");
        assert!(
            text.len() > after_move,
            "the journal kept growing after the rebuild"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The gate's finding #2**: `save` re-pointed the live session at the pool copy
    /// but left the history naming the original, so a later replay re-adopted it (and
    /// failed if the original was gone). An undo after a save must still play.
    #[test]
    fn an_undo_after_a_save_uses_the_session_pool() {
        let root = std::env::temp_dir().join(format!("host-rebase-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("elsewhere");
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_tone(&pool, "s1", 48_000, 48_000);

        let mut s = session_with_pool("rebase", &pool);
        let dir = root.join("song.d");
        s.save(&dir).expect("save");
        let reference = bounce(&mut s, 2_400, &root.join("a.wav"));

        // The original pool disappears — exactly a "save as" that left the source
        // behind — and then the session *rebuilds*: the seek and the undo both replay
        // the history, which must now name the session's own pool copy.
        std::fs::remove_dir_all(&pool).expect("remove the original pool");
        s.execute(&HostCommand::TransportSeek { frame: 0 })
            .expect("a seek after a save replays against the pool copy");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::MoveClip {
                track: "t0".into(),
                clip: "c0".into(),
                at_frame: 4_800,
            },
            at_frame: None,
        })
        .expect("move");

        // The undo rebuilds from history — which must name the session's own pool.
        assert!(s.undo().expect("undo"));
        let audio = bounce(&mut s, 2_400, &root.join("b.wav"));
        assert!(
            audio.iter().any(|x| x.abs() > 1e-3),
            "the replay found the pool copy and played"
        );
        assert_eq!(audio, reference, "and the audio is unchanged");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The gate's findings #3/#4**: a stale journal (a crash between the script
    /// rename and the journal reset) replays on the new baseline. Its commands are
    /// already in the script, so they are refused — dropped and reported, never a
    /// session that will not open.
    #[test]
    fn a_stale_journal_is_dropped_not_fatal() {
        let root = std::env::temp_dir().join(format!("host-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("pool");
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_tone(&pool, "s1", 48_000, 48_000);

        let mut s = session_with_pool("stale", &pool);
        let dir = root.join("song.d");
        s.save(&dir).expect("save");

        // The stale journal re-adds the clip the script already has, then adds a
        // legitimate edit that must survive.
        std::fs::write(
            dir.join("journal.txt"),
            "arrange add_clip t0 c0 s1 0 4800 0 0 0 1.0\narrange set_clip_gain t0 c0 0.25\n",
        )
        .expect("stale journal");

        let mut loaded = HostSession::new();
        loaded
            .load_session(&dir)
            .expect("a stale journal must not stop the load");
        let recovery = loaded.last_recovery().cloned().expect("a report");
        assert_eq!(recovery.refused, 1, "the duplicate add_clip was refused");
        assert!(
            recovery.refused_reason.is_some(),
            "and the refusal is reported"
        );
        assert_eq!(recovery.applied, 1, "the legitimate edit applied");
        assert_eq!(clip_of(&loaded).gain, 0.25, "the edit is in the session");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A save that cannot be reproduced is **refused**, not written. The word-based
    /// `host v1` form cannot express an id with whitespace, and it has no form at all
    /// for a region play — writing either would produce a file that opens as a
    /// *different* session. (A pool *path* with whitespace is fine: the save copies the
    /// pool into the session, so the log names the copy.)
    #[test]
    fn a_save_that_cannot_round_trip_is_refused() {
        let root = std::env::temp_dir().join(format!("host-lossy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("my pool"); // the space is only in the *source* path
        std::fs::create_dir_all(&pool).expect("pool dir");
        write_tone(&pool, "s1", 48_000, 48_000);

        let mut s = session_with_pool("lossy", &pool);
        let dir = root.join("song.d");
        // A clip id with a space is what survives into the log and cannot be written.
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddClip {
                track: "t0".into(),
                clip: media::Clip {
                    reversed: false,
                    id: "c 0".into(),
                    source: "s1".into(),
                    src_start: 0,
                    src_len: 2_400,
                    at_frame: 0,
                    fade_in: 0,
                    fade_out: 0,
                    gain: 1.0,
                    loop_len: None,
                },
            },
            at_frame: None,
        })
        .expect("the timeline itself accepts the id");
        let refused = s.save(&dir);
        assert!(refused.is_err(), "an id with whitespace cannot be saved");
        assert!(
            !dir.join("session.txt").exists(),
            "nothing was written: {}",
            refused.expect_err("the error")
        );

        // A whole-file play saves; a region play does not. (A clean path here: a
        // whitespace path in a `play` line is refused by the same self-check, which is
        // what the case above just proved.)
        let clean = root.join("clean");
        std::fs::create_dir_all(&clean).expect("clean dir");
        write_tone(&clean, "s1", 48_000, 48_000);
        let mut s2 = HostSession::new();
        s2.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s2.execute(&HostCommand::Pool { dir: clean.clone() })
            .expect("pool");
        s2.execute(&HostCommand::Play {
            clip: media::ClipRef {
                path: clean.join("s1.wav"),
                start: 0,
                len: 0,
            },
            channel: 0,
            at_frame: None,
        })
        .expect("whole-file play");
        let dir2 = root.join("ok.d");
        s2.save(&dir2).expect("a whole-file play round-trips");
        assert!(dir2.join("session.txt").is_file());

        // The region case needs its own session: the reference host plays one clip at
        // a time, and the point is a *history* that holds a region play.
        let region = root.join("region.d");
        let mut s3 = HostSession::new();
        s3.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s3.execute(&HostCommand::Pool { dir: clean.clone() })
            .expect("pool");
        s3.execute(&HostCommand::Play {
            clip: media::ClipRef {
                path: clean.join("s1.wav"),
                start: 100,
                len: 50,
            },
            channel: 0,
            at_frame: None,
        })
        .expect("region play");
        let refused = s3.save(&region);
        assert!(
            refused.is_err(),
            "a region play has no host v1 form: {refused:?}"
        );
        assert!(
            !region.join("session.txt").exists(),
            "nothing was written: {}",
            refused.expect_err("the error")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- recording: the input device becomes pool material ----

    /// Feed an interleaved tone into a ring, the way a device callback would.
    fn feed_tone(ring: &media::Spsc<f32>, frames: usize, channels: usize, rate: u32) {
        let mut i = 0usize;
        while i < frames {
            let phase = std::f64::consts::TAU * 440.0 * (i as f64) / rate as f64;
            let sample = (phase.sin() * 0.5) as f32;
            for _ in 0..channels {
                // Bounded: the ring is big, but a full ring is not a test failure.
                let _ = ring.try_push(sample);
            }
            i += 1;
        }
    }

    /// **The loop's first half**: a take recorded through the host lands in the pool
    /// as `{take_id}.ch{k}` sources with peaks, is arrangeable, and is audible — the
    /// whole record → arrange → render path, with the device replaced by the ring
    /// seam.
    #[test]
    fn a_take_records_into_the_pool_and_plays() {
        let root = std::env::temp_dir().join(format!("host-record-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pool = root.join("pool");
        std::fs::create_dir_all(&pool).expect("pool dir");

        let mut s = HostSession::new();
        s.execute(&HostCommand::Mount {
            plugin: "mixer",
            params: vec![("channels", 2.0)],
            at_frame: Some(0),
        })
        .expect("mixer");
        s.execute(&HostCommand::Pool { dir: pool.clone() })
            .expect("pool");

        let rate = s.sample_rate();
        let ring = std::sync::Arc::new(media::Spsc::new(1 << 16));
        s.start_recording("jam", std::sync::Arc::clone(&ring), rate, 2)
            .expect("record starts");
        let status = s.recording().expect("a status while recording");
        assert_eq!(status.take_id, "jam");
        assert_eq!(status.channels, 2);

        // Push a second of interleaved tone, then stop once the demux has drained
        // what the ring accepted. A fixed sleep here is a **race** — a loaded
        // parallel run failed it (found as a flake by slice D1's gate) — and the
        // ring drops what it cannot hold, so "wait for N frames" cannot work: poll
        // the live count instead and wait for it to stop moving (three equal,
        // non-zero readings), with a bound so a stuck demux still fails loudly in
        // the assertion below rather than hanging.
        feed_tone(&ring, rate as usize, 2, rate);
        let mut last = 0u64;
        let mut steady = 0;
        for _ in 0..400 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let now = s.recording().map(|status| status.frames).unwrap_or(0);
            if now > 0 && now == last {
                steady += 1;
                if steady >= 3 {
                    break;
                }
            } else {
                steady = 0;
            }
            last = now;
        }
        let take = s.stop_recording().expect("record stops");
        assert_eq!(take.take_id, "jam");
        assert_eq!(take.channels, 2);
        assert_eq!(
            take.sources,
            vec!["jam.ch0".to_string(), "jam.ch1".to_string()]
        );
        assert!(
            take.frames > rate as u64 / 2,
            "a second of input produced {} frames",
            take.frames
        );
        assert!(s.recording().is_none(), "the take is finished");
        assert_eq!(s.last_take(), Some(&take));

        // The takes are pool sources, with peaks — and they are at the session rate.
        let index = media::Pool::open(&pool)
            .expect("pool")
            .list()
            .expect("list");
        for (k, source) in index.sources.iter().enumerate() {
            assert_eq!(source.id, format!("jam.ch{k}"));
            assert_eq!(source.sample_rate, rate, "the take is at the session rate");
            assert!(!source.peaks_missing, "the peaks are written");
        }

        // …and they play: place ch0 on a track and render it.
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddTrack { track: "t0".into() },
            at_frame: None,
        })
        .expect("track");
        s.execute(&HostCommand::Arrange {
            op: media::ArrangeOp::AddClip {
                track: "t0".into(),
                clip: media::Clip {
                    reversed: false,
                    id: "c0".into(),
                    source: "jam.ch0".into(),
                    src_start: 0,
                    src_len: take.frames.min(rate as u64),
                    at_frame: 0,
                    fade_in: 0,
                    fade_out: 0,
                    gain: 1.0,
                    loop_len: None,
                },
            },
            at_frame: None,
        })
        .expect("clip from the take");
        let audio = bounce(&mut s, 4_800, &root.join("t.wav"));
        assert!(
            audio.iter().any(|x| x.abs() > 1e-3),
            "the recorded take is audible"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Recording is one take at a time, needs somewhere to put it, and stopping
    /// nothing is an error — never a silent half-take or a panic.
    #[test]
    fn recording_refuses_what_it_cannot_do() {
        let root = std::env::temp_dir().join(format!("host-record-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");

        // No pool: the take has nowhere to land.
        let mut s = HostSession::new();
        let ring = std::sync::Arc::new(media::Spsc::<f32>::new(64));
        let refused = s.start_recording("jam", std::sync::Arc::clone(&ring), 48_000, 1);
        assert!(refused.is_err(), "{refused:?}");
        assert!(
            refused.expect_err("the reason").contains("set_pool"),
            "the refusal names the fix"
        );

        // With a pool: one at a time, and a stop with nothing recording is refused.
        s.execute(&HostCommand::Pool { dir: root.clone() })
            .expect("pool");
        s.start_recording("jam", std::sync::Arc::clone(&ring), 48_000, 1)
            .expect("first take starts");
        assert!(
            s.start_recording("jam2", std::sync::Arc::clone(&ring), 48_000, 1)
                .is_err(),
            "one take at a time"
        );
        let take = s.stop_recording().expect("the take stops");
        assert_eq!(take.take_id, "jam");
        assert!(s.stop_recording().is_err(), "stopping nothing is an error");

        // A bad take id is refused by the media layer, with its rule stated.
        let refused = s.start_recording("bad id!", std::sync::Arc::clone(&ring), 48_000, 1);
        assert!(refused.is_err(), "a take id goes into filenames");

        // Re-recording over an existing take is refused: a clip that already references
        // `{take_id}.ch0` must not silently change content.
        let again = s.start_recording("jam", std::sync::Arc::clone(&ring), 48_000, 1);
        assert!(again.is_err(), "an existing take id is refused");
        assert!(
            again
                .expect_err("the reason")
                .contains("already has a take"),
            "and it says which file is in the way"
        );

        // A rebuild (`transport seek`) stops a take in progress — finalized, and the
        // report survives the rebuild so the shell can still place it.
        s.start_recording("later", std::sync::Arc::clone(&ring), 48_000, 1)
            .expect("a second take under a new id");
        s.execute(&HostCommand::TransportSeek { frame: 0 })
            .expect("the seek rebuilds the session");
        assert!(s.recording().is_none(), "the take was stopped, not dropped");
        let interrupted = s
            .last_take()
            .cloned()
            .expect("the interrupted take is reported");
        assert_eq!(interrupted.take_id, "later");
        assert!(
            root.join("later.ch0.wav").is_file(),
            "and its WAV was finalized in the pool"
        );

        // The text form: `record <take_id>` starts, `record stop` stops.
        let parsed = parse_script("host v1\nrecord jam1\nrecord stop\n").expect("parse");
        assert!(matches!(&parsed[0], HostCommand::Record { take_id } if take_id == "jam1"));
        assert!(matches!(&parsed[1], HostCommand::RecordStop));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A grid is UI state, a snapped frame is the log's.** `snap=<frames>` is a
    /// parse-time modifier: it quantizes the frame operand, so a *script* snaps
    /// exactly like the shell without the log ever learning what a grid is. The
    /// formatter therefore writes the snapped frame and no modifier.
    #[test]
    fn the_snap_modifier_quantizes_the_frame_operand() {
        // The plan's example: 48 213 frames on a 480-frame grid is 48 000.
        let (op, at) = parse_arrange_line("arrange move_clip t0 c0 48213 snap=480 @0").unwrap();
        assert_eq!(at, Some(0));
        let media::ArrangeOp::MoveClip { at_frame, .. } = op else {
            panic!("expected move_clip, got {op:?}");
        };
        assert_eq!(at_frame, 48_000);
        assert_eq!(
            format_command(
                &HostCommand::Arrange {
                    op: media::ArrangeOp::MoveClip {
                        track: "t0".into(),
                        clip: "c0".into(),
                        at_frame,
                    },
                    at_frame: Some(0),
                },
                None,
            )
            .as_deref(),
            Some("arrange move_clip t0 c0 48000 @0"),
            "the log records the snapped frame, never the grid"
        );

        // Every op with a frame operand takes it, and the modifier may precede
        // the `@frame` token or follow it.
        let script = "host v1\nmount mixer channels=2 @0\narrange add_clip t0 c0 s1 0 4000 48213 0 0 1.0 snap=24000\narrange razor_split t0 c0 cL cR 48213 snap=480 @0\narrange move_clip_to_track t0 c0 t1 48213 snap=480\ntransport seek 48213 snap=480\n";
        let commands = parse_script(script).expect("the modifier parses anywhere it applies");
        let frames: Vec<u64> = commands
            .iter()
            .filter_map(|c| match c {
                HostCommand::Arrange { op, .. } => match op {
                    media::ArrangeOp::AddClip { clip, .. } => Some(clip.at_frame),
                    media::ArrangeOp::RazorSplit { at_frame, .. } => Some(*at_frame),
                    media::ArrangeOp::MoveClipToTrack { at_frame, .. } => Some(*at_frame),
                    _ => None,
                },
                HostCommand::TransportSeek { frame } => Some(*frame),
                _ => None,
            })
            .collect();
        assert_eq!(frames, vec![48_000, 48_000, 48_000, 48_000]);

        // A modifier that cannot apply is an error, never a silent no-op.
        for bad in [
            "host v1\narrange delete t0 c0 snap=480\n",
            "host v1\nmount mixer channels=2 snap=480 @0\n",
            "host v1\narrange move_clip t0 c0 48213 snap=0\n",
            "host v1\narrange move_clip t0 c0 48213 snap=abc\n",
            "host v1\ntransport stop snap=480\n",
        ] {
            assert!(parse_script(bad).is_err(), "refused: {bad:?}");
        }
    }

    /// **A stray operand is a typo, not something to ignore.** Found the hard way: a
    /// test typed `record bad id!` and the parser silently started a take called `bad`
    /// — and opened the real input device. Every fixed-shape command is strict now.
    #[test]
    fn a_stray_operand_is_a_parse_error() {
        for line in [
            "pool /data/takes extra",
            "record jam extra",
            "save /tmp/x extra",
            "load /tmp/x extra",
            "session_rate 48000 extra",
            "unmount tone extra",
            "bounce 1000 /tmp/x.wav extra",
            "patch euclidean.triggers scale.trigger extra",
            "set_param mixer ch0.gain 0.5 extra",
            "set_tempo 120 4 extra",
            "play /tmp/a.wav ch0 extra",
            "splice 100 /tmp/a.wav 64 extra",
            "undo extra",
            "redo extra",
            "transport play extra",
            "transport stop extra",
            "transport seek 4800 extra",
        ] {
            let script = format!("host v1\n{line}\n");
            assert!(
                parse_script(&script).is_err(),
                "a stray word must be refused: {line}"
            );
        }

        // …and the strict forms themselves still parse.
        let script = "host v1\npool /data/takes\nrecord jam\nrecord stop\nsession_rate 48000\ntransport play\ntransport seek 4800\ntransport stop\nset_param mixer ch0.gain 0.5\nset_tempo 120 4\npatch euclidean.triggers scale.trigger\nunmount tone\nbounce 1000 /tmp/x.wav\nundo\nredo\n";
        assert!(
            parse_script(script).is_ok(),
            "the strict forms must still parse: {:?}",
            parse_script(script).err()
        );
    }
}
