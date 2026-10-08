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
//! (`src/main.rs`) is the composition-seams "headless smoke binary". The
//! shells implement the identical contract — swapping shells swaps only
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
use media::midi::MidiOut;
use media::{
    ClipRef, DEFAULT_RING_CAPACITY, FilePlayer, Interner, Mailbox, PlaybackNode, SpliceCmd,
};

use engine::plugins::SharedMidiSink;

pub mod live;
pub mod media_ops;
pub mod rig;

/// The session's tempo/meter map — re-exported because a shell reads it off the
/// snapshot to do its own beat-domain math (grid snapping, a bar/beat ruler), and
/// a public field needs a nameable type.
pub use engine::TempoMap;

use media_ops::{BounceRecord, MediaSession, PlayerIntent, SpliceIntent};

/// The Host API contract version. The text format's first line must be
/// `host v{N}`; mismatches are refused (kimi review finding 6).
pub const HOST_API_VERSION: u32 = 1;

/// The host registry: plugins, then ports, then parameters — validated per
/// slot by the parser (a port name is not a plugin name). `clock_out` declares
/// no ports or params; it is mounted for its side effect through the host's
/// sink (the midi-clock-out note, slice B).
pub const HOST_PLUGINS: &[&str] = &["euclidean", "scale", "tone", "mixer", "master", "clock_out"];

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
    /// An **action**, not state: opening a device is a side effect, and a replay must
    /// never touch hardware. What the log keeps is the finished [`HostCommand::Take`]
    /// declaration, so a loaded session describes its own material.
    Record {
        take_id: String,
        /// The **declared rig source** to capture (`source add <name> …`), if any.
        /// `None` keeps the original behaviour — the machine's default input —
        /// so every existing script and session still parses and records. A named
        /// source that is unknown, or whose kind has no binding yet, is a loud
        /// refusal: the recorder never silently captures a different device.
        source: Option<String>,
    },
    /// Stop the take in progress and finalize it (headers, peaks). A no-op-looking
    /// error when nothing is recording — never a silent half-take. Commits the
    /// resulting [`HostCommand::Take`] to the session's state.
    RecordStop,
    /// **A finished take**, as state — and *not* a capture. The pool sources the
    /// recording wrote and the shape of what they hold, so a loaded session names its
    /// material and a replay binds it **without opening a device**. What is in the WAV
    /// is not this layer's business: reproducibility starts at the pool file, and any
    /// session that loads that file renders the same bytes.
    ///
    /// The pool ids follow the `{take_id}.ch{k}` convention every clip already
    /// references, so the declaration names the take and its shape rather than each
    /// file; `at_frame` is where the capture started on the timeline and is
    /// deliberately **not** an `@frame` edit time.
    Take {
        take_id: String,
        /// Session frames written per channel.
        frames: u64,
        /// Monitor frames a **monitored** ring could not take — best-effort only, and
        /// zero when nothing monitored the take. It is *not* a measure of the material
        /// (the pool write is authoritative); a fidelity fact the WAV cannot record, so
        /// the declaration keeps it. The token stays `dropped` for `host v1` compatibility.
        dropped: u64,
        channels: usize,
        at_frame: u64,
        /// The **declared source** this take captured, when one was named. Trailing and
        /// optional in the line (`take … source=<name>`), so a session recorded before
        /// the binding slice parses unchanged. `None` means the default input: the
        /// material is still real, the session just cannot say what it heard.
        source: Option<String>,
    },
    /// **A declared rig source** — the set of sources the recorder intends to
    /// capture, as *state* (see the takes slice): the declaration is data, and a
    /// replay rebuilds it with no hardware attached. Identity is the stable
    /// `name` plus a matching *rule*, never a device index (indices renumber
    /// across reboots and replugs). Binding the name to a real device is a
    /// device-bound side effect the clock/binding slice adds; nothing here
    /// opens, binds or probes one.
    SourceAdd {
        name: String,
        kind: &'static str,
        matcher: String,
        channels: usize,
        clock: rig::ClockRole,
    },
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
    /// **Export the whole arrangement** — `bounce`'s sibling, not an overload of it.
    /// `bounce` keeps its hand-computed frame count, 16-bit WAV and byte-identical
    /// test role; `export` is the *deliverable*: it replays to frame 0 (so the
    /// compressor's ballistics and the render are reproducible), measures the
    /// arrangement's own length instead of asking for a count, writes **f32** by
    /// default (bit-exact, golden-file testable) or **s16 with fixed-seed TPDF
    /// dither**, reports peak/RMS, and **refuses rather than writing a clipped file**.
    /// An action (it writes a file), like `Bounce`.
    Export { path: PathBuf, format: ExportFormat },
}

/// The sample format an [`HostCommand::Export`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportFormat {
    /// 32-bit float WAV — the mix exactly as rendered, no quantisation.
    #[default]
    F32,
    /// 16-bit PCM with fixed-seed TPDF dither (a reproducible, decorrelated floor).
    S16,
}

impl ExportFormat {
    /// The word the `host v1` form uses (`export <path> [f32|s16]`).
    pub fn name(self) -> &'static str {
        match self {
            ExportFormat::F32 => "f32",
            ExportFormat::S16 => "s16",
        }
    }

    /// The recorded code (a stable small number, so the log line does not depend on
    /// the spelling of a name).
    pub fn code(self) -> u32 {
        match self {
            ExportFormat::F32 => 0,
            ExportFormat::S16 => 1,
        }
    }

    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(ExportFormat::F32),
            1 => Some(ExportFormat::S16),
            _ => None,
        }
    }
}

impl HostCommand {
    /// The same command with every frame placement removed, members included.
    ///
    /// This is the **export rebuild**'s transform: an export wants the session's
    /// *current* value (order decides, the last writer wins), rendered from the start,
    /// without the clock walking the timeline while the state is applied. A seek keeps
    /// its frame-gated replay — the two questions are different ("what is the session
    /// now" vs "what was it at frame N").
    pub fn at_now(&self) -> HostCommand {
        let now = |_at: &Option<u64>| None;
        match self {
            HostCommand::Mount {
                plugin,
                params,
                at_frame,
            } => HostCommand::Mount {
                plugin,
                params: params.clone(),
                at_frame: now(at_frame),
            },
            HostCommand::Patch { from, to, at_frame } => HostCommand::Patch {
                from: *from,
                to: *to,
                at_frame: now(at_frame),
            },
            HostCommand::SetParam {
                plugin,
                param,
                value,
                at_frame,
            } => HostCommand::SetParam {
                plugin,
                param,
                value: *value,
                at_frame: now(at_frame),
            },
            HostCommand::SetTempo {
                bpm,
                beats_per_bar,
                at_frame,
            } => HostCommand::SetTempo {
                bpm: *bpm,
                beats_per_bar: *beats_per_bar,
                at_frame: now(at_frame),
            },
            HostCommand::Unmount { plugin, at_frame } => HostCommand::Unmount {
                plugin,
                at_frame: now(at_frame),
            },
            HostCommand::Arrange { op, at_frame } => HostCommand::Arrange {
                op: op.clone(),
                at_frame: now(at_frame),
            },
            HostCommand::Play {
                clip,
                channel,
                at_frame,
            } => HostCommand::Play {
                clip: clip.clone(),
                channel: *channel,
                at_frame: now(at_frame),
            },
            HostCommand::Group { commands } => HostCommand::Group {
                commands: commands.iter().map(HostCommand::at_now).collect(),
            },
            // Commands whose frame is **content** (a splice's timeline position) or that
            // have none are already "now": they carry the value, not a placement.
            other => other.clone(),
        }
    }

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
    /// (`bounce`, `export`, the transport ops, `record`) are not. An export *renders*
    /// the session, so replaying it would be both useless and expensive — but its
    /// report is logged (`MediaExport`, like `MediaBounce`).
    ///
    /// `session_rate` is **context, not an edit** (its own doc says so): it is fixed when
    /// the session is created and re-stated by the saved script's header line, so it is
    /// not recorded. Recording it made `save` write the rate twice and every
    /// open-and-save cycle grow the script by a line.
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
                | HostCommand::SetSourceTempo { .. }
                | HostCommand::Take { .. }
                | HostCommand::SourceAdd { .. }
        )
    }
}

/// How much audio a **seek run-in** renders before the target frame, so every stateful
/// node reaches the state a full replay would have had at that point.
///
/// One second is the alpha profile's honest bound: the only stateful nodes on the mix bus
/// are the master's compressor (a 10 ms attack / 120 ms release envelope) and its 5 ms
/// lookahead limiter, all far inside it. A future effect with longer memory (a reverb, a
/// long delay) must **raise this**, and
/// [`a_warm_seek_equals_a_replay`](HostSession) is the test that fails if it is too short:
/// it renders the same window from a warmed seek and from a full replay and compares the
/// bytes.
pub const SEEK_WARMUP_FRAMES: u64 = 48_000;

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

/// The MIDI clock-out status a shell reads (see
/// [`HostSession::midi_status`]): the port being driven and the plugin's
/// overflow counter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiOutStatus {
    /// The port name the sink was opened from; `None` when the process asked
    /// for no MIDI output.
    pub port: Option<String>,
    /// Events the `clock_out` plugin dropped because a block exceeded its
    /// per-block bound — a mount this sane never sees a nonzero count.
    pub overflows: u64,
}

/// The assembled profile: the engine, the registered factories, the media
/// wiring (player nodes into the mixer), and the master bounce path.
pub struct HostSession {
    engine: Engine,
    /// The transport tap the host feeds at the frames `Play`/`Stop` take
    /// effect. Provided under `"transport"` **always** — it is pure
    /// bookkeeping (a queue of frames), no device — so a session mounts
    /// `clock_out` identically with and without gear attached. Fed from the
    /// **apply** path, which is where a command's frame is known; a rebuild
    /// gets a **fresh** log and does not replay `Play`/`Stop` (they are
    /// actions, not state), so the one other feed is the seek's own re-anchor
    /// in [`Self::replay_to_kind`]. Silence on a replay comes from the sink's
    /// absence, never from a special case here.
    transport: Arc<TransportLog>,
    /// The MIDI output sink **slot** — always present, containing the device
    /// when the process asked for one (`--midi-out`/`DSH_MIDI_OUT`, see
    /// [`midi_out_from_process`]) and empty when it did not. Provided under
    /// `"midi.out"` either way, so a session's `clock_out` node can be
    /// **re-filled later without re-mounting**. **Configuration, not session
    /// state**: which device is attached is a fact about the machine the
    /// process runs on, so it is never a logged command and never replayed —
    /// a rebuilt session shares the *same slot object* (the node inside sees
    /// the re-fill), and around any rebuild the host empties the slot, renders
    /// the reconstruction, and restores the device: nothing is sent across a
    /// rebuild (the seek-and-rebuild decision).
    midi_slot: SharedMidiSink,
    /// The port name the sink was opened from, reported on the snapshot so a
    /// shell can show whether gear is being driven.
    midi_port: Option<String>,
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
    /// Cached pool source ids, for the snapshot. Listing the pool reads the
    /// directory, and the snapshot is published on every pump tick, so the listing
    /// is refreshed only where the pool can change (`set_pool`, a finished take)
    /// rather than per frame. A shell reads it to name a new take without
    /// colliding with an existing source.
    pool_ids: Vec<String>,
    /// The snapshot's cached arrangement (see [`Self::refresh_timeline`]): rebuilt where
    /// the arrangement can change, and published behind an `Arc` so a polling shell draws
    /// a timeline every frame without copying it.
    timeline: crate::live::TimelineStatus,
    /// The last successful export's report (length, format, peak, RMS) — a shell
    /// shows what was written without recomputing the render.
    last_export: Option<media_ops::ExportRecord>,
    /// The last seek: its target frame and whether the **warm-up** path ran (a jump that
    /// rendered a one-second run-in rather than the timeline from 0).
    last_seek: Option<(u64, bool)>,
    /// The tempo each pool source was performed at (`source_tempo <id> <bpm>`), which
    /// is what **tempo match** derives its ratio from. State: the log carries it, a
    /// replay rebuilds it, and the outcome exposes it to a shell.
    source_tempos: std::collections::HashMap<String, f64>,
    /// The declared rig: the sources this session intends to capture. Pure
    /// state — the log carries the declarations, and no arm of the apply path
    /// touches a device to produce one.
    sources: Vec<rig::SourceDecl>,
    /// track id → the arranger node mounted for it (avoids re-wiring on a rebuild).
    wired_tracks: std::collections::HashMap<String, engine::NodeId>,
    /// the arrangement value changed since last wiring (re-wire before render).
    arrange_dirty: bool,
    /// The bus went away **while the arrangement was wired**, so the wiring was
    /// retired with it and the session now owes a bus. This is what separates the
    /// two bus-less cases `wire_arranger` has to answer differently: a session that
    /// unmounts the mixer mid-piece and mounts it again later is rendering a window
    /// of its own timeline — silence there, then the wiring rebuilt on the next bus —
    /// while an arrangement with no bus *anywhere* is a session that cannot render,
    /// and says so. Cleared by the reconcile that answers the debt.
    arranger_awaiting_bus: bool,
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
    /// Why the last journal write did not happen, if it did not. The two kinds are kept
    /// apart because they **clear differently**: a *write* failure belongs to the file,
    /// and the next successful append proves the file is writable again; a *refusal*
    /// belongs to the entry, and no later write makes that edit durable — so it stands
    /// until the next [`HostSession::save`], which rewrites the journal from a history
    /// that can then be spelled in full.
    journal_error: Option<JournalFault>,
    /// What the last `Load` recovered from the journal.
    last_recovery: Option<JournalRecovery>,
    /// The media intent value (pool dir, player, splices, bounce records). The
    /// media-op handlers rebuild it on replay, so a replayed log reproduces the
    /// media session — media determinism is in the one log, not a parallel seam.
    media: Arc<Mutex<MediaSession>>,
    /// Interns runtime media strings (paths) to `&'static str` for the log
    /// (spike scale — a serialized log would use a string table).
    media_intern: Interner,
    /// **This session is being built to check a document, not to be used** — the
    /// dry-run mode [`HostSession::save`] validates its own output in, and the
    /// reason it is a *mode* rather than a separate list of checks: the value
    /// doors a reader would meet must be the **same code** the reader runs
    /// (`Engine::set_param`'s finiteness, the timeline's operand rules, the
    /// mixer's channel count), or the writer keeps a second answer to "does this
    /// apply?" and it rots.
    ///
    /// So the whole apply path runs, for real, on a throwaway session — and the
    /// three places whose work is **not about the document** stand down. A
    /// validating walk therefore opens no media file, spawns no reader thread,
    /// warms no decoder, writes no pool and renders no frames: what it still
    /// asks is every question that is about the *document* (does this value pass
    /// the applier's door, is this mixer mounted, is this op legal in this
    /// arrangement), and what it stops asking is every question that is about
    /// *this machine's copy of the material* — a `play` file that has moved, a
    /// decoder that would not warm, an arranger node that would not build. Those
    /// are runtime conditions: a session is not invalid because its material is
    /// not where it was, and a save that refused on them would break saving for
    /// sessions that work.
    validating: bool,
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
        let (midi_port, midi_out) = midi_out_from_process();
        Self::new_at_with(rate, midi_out, midi_port)
    }

    /// Create a session with an explicit MIDI output sink **slot** — the seam
    /// tests mount a **fake** through, using the same `"midi.out"` context key
    /// the real sink uses. A `Some` slot is adopted **as the session's slot
    /// object** (shared, not copied — a rebuild passes this session's slot so
    /// its nodes see a re-fill), `None` mounts a fresh empty one. `midi_port`
    /// is the report-only name (the snapshot shows what is being driven);
    /// pass `None`/`None` for a silent session.
    pub fn new_at_with(
        rate: u32,
        midi_out: Option<SharedMidiSink>,
        midi_port: Option<String>,
    ) -> Self {
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
        // The clock-out plugin: no ports, no params — its effect runs through
        // the context services below (the midi-clock-out note, slice B).
        engine.register_factory(
            "clock_out",
            plugins::clock_out_factory,
            plugins::clock_out::CLOCK_OUT_PORTS,
            &[],
        );
        // The transport tap is provided **always**: it is bookkeeping, not a
        // device, and a session must mount identically with and without gear.
        let transport = Arc::new(TransportLog::new());
        engine
            .ctx
            .provide(plugins::TRANSPORT_KEY, transport.clone());
        // The sink **slot** is provided **always**: an empty slot is the
        // legitimate "no device (yet)" state the plugin mounts under, and the
        // process-lifetime slot object lets the host detach and refill around
        // a rebuild without re-mounting the plugin.
        let midi_slot = midi_out.unwrap_or_else(|| Arc::new(Mutex::new(None)));
        engine.ctx.provide(plugins::MIDI_OUT_KEY, midi_slot.clone());
        let media: Arc<Mutex<MediaSession>> = Arc::new(Mutex::new(MediaSession::default()));
        media_ops::register_handlers(&mut engine, media.clone())
            .expect("media op handlers register once on a fresh engine");
        let mut session = HostSession {
            engine,
            transport,
            midi_slot,
            midi_port,
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
            pool_ids: Vec::new(),
            timeline: crate::live::TimelineStatus::default(),
            last_export: None,
            last_seek: None,
            source_tempos: std::collections::HashMap::new(),
            sources: Vec::new(),
            wired_tracks: std::collections::HashMap::new(),
            arrange_dirty: false,
            arranger_awaiting_bus: false,
            playing: false,
            last_drain: DrainOutcome::default(),
            history: Vec::new(),
            redo: Vec::new(),
            recording: None,
            last_take: None,
            session_dir: None,
            journal_error: None,
            last_recovery: None,
            validating: false,
        };
        // The snapshot's arrangement is a **cached** reconstruction: build it once so a
        // fresh session reports an empty arrangement rather than "unknown", and rebuild
        // it where the arrangement can change (see `refresh_timeline`).
        session.refresh_timeline();
        session
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
        // Validate it exists as a directory. **This refusal is load-bearing**: the
        // rebuild path relies on it to force a refusal when a session's pool is
        // gone (`a_refused_undo_leaves_the_history_alone`), so a caller that wants
        // a fresh pool creates the directory itself.
        let pool = media::Pool::open(&dir)?;
        // A file that cannot be converted is reported, not fatal: it stays at its
        // own rate and the arranger names it if a clip reads it.
        self.pool_conformed = pool.conform(self.engine.clock.sample_rate)?.converted;
        // The pool's own guarded lookup, not a `join` of the id onto the dir: a
        // clip's `source` is a token in the log, so it can carry `../` and would
        // otherwise read a WAV from outside the pool.
        self.pool_resolver = Some(pool.resolver());
        self.pool_dir = Some(dir);
        // A rebind invalidates the cache: what the previous pool held belongs to it,
        // not to this one. List from the handle already in hand rather than opening
        // the directory a second time, and if the listing fails the cache stays empty
        // — "unknown", which the host's free-id check covers — instead of carrying
        // the old pool's names across.
        self.pool_ids.clear();
        self.refresh_pool_ids_from(&pool);
        Ok(())
    }

    /// Re-list the bound pool's source ids into the cache the snapshot publishes.
    ///
    /// Called where the pool can change, not per frame: `Pool::list` reads the
    /// directory. **A failed refresh keeps the previous ids**: the pool that is bound
    /// has not changed, and forgetting what it holds would make a shell's auto-namer
    /// restart at `take-1` and eat a refusal for a name that should have been
    /// skipped. The host's free-id check is the safety net against overwrite, not
    /// against forgetting.
    fn refresh_pool_ids(&mut self) {
        let Some(dir) = self.pool_dir.clone() else {
            return;
        };
        if let Ok(pool) = media::Pool::open(&dir) {
            self.refresh_pool_ids_from(&pool);
        }
    }

    /// Take the ids from a pool in hand, leaving the cache alone if the listing fails.
    /// The caller owns whether "left alone" is meaningful: `set_pool` clears first,
    /// because it rebinds.
    fn refresh_pool_ids_from(&mut self, pool: &media::Pool) {
        let Ok(index) = pool.list() else {
            return;
        };
        let mut ids: Vec<String> = index.sources.into_iter().map(|s| s.id).collect();
        // A `.wav` that exists but cannot be read is reported in `errors`, not
        // `sources` — and it still **occupies its name**. Keep the id anyway: a shell
        // names a new take against this list, so an id missing here means it proposes
        // one the free-id check then refuses, which is the symptom the cache exists to
        // prevent. (The merge gate found the hole; `Pool::list`'s per-source error path
        // is otherwise invisible to the cache.)
        ids.extend(index.errors.into_iter().filter_map(|(path, _)| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".wav"))
                .map(str::to_string)
        }));
        ids.sort();
        ids.dedup();
        self.pool_ids = ids;
    }

    /// The pool source ids, cached — what a shell reads to name a new take
    /// without colliding with a source the session already holds.
    pub fn pool_ids(&self) -> &[String] {
        &self.pool_ids
    }

    /// Rebuild the snapshot's cached arrangement.
    ///
    /// [`Self::arrangement`] *reconstructs* the timeline from the editor's log, so it is
    /// called where the arrangement can change — a committed state command ([`Self::execute`]),
    /// a load ([`Self::from_script`]), and a rebuild's adoption (`carry_over`) — never per
    /// pump tick and never per replayed step of a long history. The snapshot carries the
    /// result behind an `Arc`, so a shell's poll clones a pointer.
    fn refresh_timeline(&mut self) {
        self.timeline = match self.arrangement() {
            Ok(timeline) => crate::live::TimelineStatus {
                timeline: Some(std::sync::Arc::new(timeline)),
                error: None,
            },
            Err(e) => crate::live::TimelineStatus {
                timeline: None,
                error: Some(e),
            },
        };
    }

    /// The snapshot's arrangement cache, as published (see [`Self::refresh_timeline`]).
    pub fn timeline_status(&self) -> &crate::live::TimelineStatus {
        &self.timeline
    }

    /// The sources the last [`set_pool`](Self::set_pool) brought to the session
    /// rate (empty when the pool already fitted). A shell shows this once, as a
    /// fact about the load — the pool is not converted again afterwards.
    pub fn pool_conformed(&self) -> &[media::Conform] {
        &self.pool_conformed
    }

    /// Adopt a pool **without conforming it** — the door half of [`Self::set_pool`],
    /// for a dry run (`validating`). Same validation (the path must be a directory,
    /// which is what the reader will ask when it opens the file) and the same guarded
    /// resolver, so every arrangement op behind it still sees a pool; the *write* is
    /// what stands down. `conform` reads every source's header and rewrites the ones
    /// at a foreign rate, so a save that ran it would read the whole pool and write
    /// into the session directory it is in the middle of publishing. No verdict is
    /// lost: a source that cannot be converted is reported, not fatal, so adopting a
    /// pool for real never refuses a document over its material either.
    fn note_pool(&mut self, dir: PathBuf) -> Result<(), String> {
        let pool = media::Pool::open(&dir)?; // validate it exists as a directory
        self.pool_resolver = Some(pool.resolver());
        self.pool_dir = Some(dir);
        Ok(())
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
        // The origin is read here, once: the declaration must say where the capture
        // started even though the capture itself is never replayed.
        let at_frame = self.engine.clock.frame();
        self.last_take = None;
        self.recording = Some(Recording {
            handle: None,
            capture,
            take_id: take_id.to_string(),
            // The device-less seam (`start_recording`) and its tests have no source;
            // `record` fills this in when a declared source was named.
            source: None,
            sources,
            at_frame,
        });
        Ok(())
    }

    /// Start a take from a capture source — the `record <take_id> [source=<name>]` line.
    ///
    /// The device is opened at its own default config (its channel count and rate), and
    /// the take lands in the session's pool at the session rate. Recording is
    /// independent of the transport: the take is not aligned to the playhead (that is
    /// the jam layer's fixed-offset problem), it is material for the arrangement.
    ///
    /// With `source = None` this is the **default input device**, exactly as before the
    /// binding slice. With a name, the declared [`rig::SourceDecl`] is resolved to a
    /// real capture path and the name travels into the take's report and declaration:
    /// a session then says which source it recorded, not just what the machine's
    /// default happened to be. An unknown name, or a kind with no binding yet
    /// (`jack`, `osc`), is refused — never a silent fallback.
    pub fn record(&mut self, take_id: &str, source: Option<&str>) -> Result<(), String> {
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
        let handle = match source {
            None => media::devices::open_input(std::sync::Arc::clone(&ring))?,
            Some(name) => {
                // Resolve the declaration first: the refusal has to name the rig, and
                // it must happen before any device is touched.
                let decl = self
                    .sources
                    .iter()
                    .find(|s| s.name == name)
                    .cloned()
                    .ok_or_else(|| {
                        let declared: Vec<&str> =
                            self.sources.iter().map(|s| s.name.as_str()).collect();
                        if declared.is_empty() {
                            format!(
                                "no source '{name}' is declared — declare one with \
                                 `source add {name} kind=… match=… channels=… clock=…`"
                            )
                        } else {
                            format!(
                                "no source '{name}' is declared (declared: {})",
                                declared.join(", ")
                            )
                        }
                    })?;
                match decl.kind {
                    "alsa" => media::devices::open_input_device(
                        std::sync::Arc::clone(&ring),
                        &decl.matcher,
                    )?,
                    "pulse" => media::devices::open_input_pulse_source(
                        std::sync::Arc::clone(&ring),
                        &decl.matcher,
                    )?,
                    other => {
                        return Err(format!(
                            "source '{name}' is declared kind '{other}', which has no binding yet \
                             (kinds with a binding: alsa, pulse)"
                        ));
                    }
                }
            }
        };
        let rate = handle.sample_rate;
        let channels = (handle.channels as usize).clamp(1, media::capture::CAPTURE_CHANNELS_SANITY);
        self.start_recording(take_id, ring, rate, channels)?;
        if let Some(rec) = self.recording.as_mut() {
            rec.handle = Some(handle);
            rec.source = source.map(str::to_string);
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
            source,
            sources,
            at_frame,
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
            monitor_dropped: capture.dropped(),
            channels: capture.channels(),
            source,
            sources,
            sample_rate: self.engine.clock.sample_rate,
            at_frame,
        };
        self.last_take = Some(report.clone());
        // The take's sources are in the pool now: refresh the cache so the next
        // take is named against what actually exists.
        self.refresh_pool_ids();
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
            monitor_dropped: rec.capture.dropped(),
            channels: rec.capture.channels(),
            source: rec.source.clone(),
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

    /// The declared rig: the sources this session intends to capture (see
    /// [`HostCommand::SourceAdd`]). Declarations only — binding them to
    /// devices is a later slice's side effect, never this accessor's business.
    pub fn sources(&self) -> &[rig::SourceDecl] {
        &self.sources
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

    /// Why a journal write did not happen, if one did not. An edit is never *failed* by
    /// a journal fault (it is already applied and logged), but autosave must not fail
    /// silently either.
    ///
    /// **One string, two faults, and the message says which** — the field is a
    /// `JournalFault`, and both variants report a loss of the *autosave* for an edit,
    /// which is all a shell's status line has to say. They differ in what clears them:
    ///
    /// - a **write** failure is the journal file's, and the next successful append (or a
    ///   `save`, which rewrites the journal) clears it;
    /// - a **refusal** (an edit the `host v1` form cannot carry) **outlives every
    ///   successful write**, because that edit is still in no durable record and a save
    ///   refuses the same history. It does **not** outlive the edit: it is re-derived
    ///   from the history whenever the history changes (`undo`, `redo`, a seek), so it
    ///   cannot report an edit that is no longer in the session.
    pub fn journal_error(&self) -> Option<&str> {
        self.journal_error.as_ref().map(JournalFault::message)
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
    ///
    /// **Nothing is written unless the text both reads back and is one the applier
    /// accepts.** The file is parsed, compared with the history, and then walked on a
    /// fresh session by [`HostSession::from_script`] — the walk `load_session` runs — so
    /// `save` cannot produce a directory the reader refuses over a *value*. **Every**
    /// refusal happens before the journal is touched: the journal is the only durable
    /// record of the edits since the last save, and losing it to a refused save would
    /// cost the tail to buy nothing. The rule is deliberately stronger than the
    /// journal's ([`HostSession::journal_error`]): the journal is best-effort and the load
    /// side drops and reports an entry it cannot apply, while this is the baseline the
    /// whole session is rebuilt from and has to open.
    ///
    /// **The walk is a dry run, and that is the whole correction to how this check was
    /// built first.** The rule is about the *document*: a value the form spells
    /// faithfully and the applier refuses (`set_param … NaN`, a param out of range, an op
    /// illegal in this arrangement) is a file no reader will open. It is **not** about
    /// this machine's copy of the material: a `play` whose file has moved, a decoder
    /// that would not warm, an arranger node that would not build — those are runtime
    /// conditions, and a save that failed on them broke sessions that work. So the
    /// check opens no file, spawns no thread, writes no pool and renders no frames; see
    /// [`HostSession::validating`] for the three places that stands down and why each is
    /// material rather than document.
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
        // session. The comparison is the same *serialised* one `journal_append` uses
        // (`same_commands`): an `f32` operand that cannot compare equal to itself (a
        // `NaN`) is in the form, and comparing it with `PartialEq` refused every save of
        // the session for good — the wedge the autosave half of this rule shared.
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
        if !same_commands(check.get(1..).unwrap_or(&[]), &expected) {
            return Err(
                "save: the session text does not round-trip — refusing to write a lossy session"
                    .into(),
            );
        }

        // **…and it must not merely parse, it must be one the *applier* accepts.** The
        // rule `save` rests on is "whatever is written can be read back **and every value
        // in it passes the reader's own door**", not "parses to the same commands": a line
        // can spell a value faithfully and still be refused — a `NaN` param, whose only
        // spelling is `NaN` and whose only verdict is `parameter '…' must be finite`. So
        // the script is walked, on a fresh session, by the same walk `load_session` runs
        // (`HostSession::from_script`), in its **dry-run** mode: the value doors are the
        // reader's own code (the engine's `set_param`, the timeline's op rules, the
        // mixer's channel count) rather than a writer-side list that would rot, and the
        // material is left alone. Any operand the *form* carries and the *applier* refuses
        // would otherwise land here as a `session.txt` that `load_session` refuses — a
        // bricked directory that needs a hand-edit of the text to recover. The door that
        // stops the value reaching the history in the first place is the engine's; this is
        // the *writer's* half, and it is the half that cannot be reasoned about one op at a
        // time.
        //
        // **Before anything is written, and before the journal is truncated** — the order
        // is the point. The journal *is* the autosave: truncating it and then failing
        // would destroy the tail of edits that are in no other record, to no purpose (the
        // script they belong to is not being replaced). So every refusal here happens with
        // the directory exactly as it was: the previous `session.txt` still opens, and the
        // journal still holds every edit since it was written.
        if let Err(fault) = HostSession::from_script_with(&check, None, None, true) {
            // Name the **edit** and the reason, not an index: the history and the parsed
            // script are the same commands (the check above just proved it), and the
            // entry is what the user actually made.
            let named = match fault.index.checked_sub(1).and_then(|i| self.history.get(i)) {
                Some(entry) => name_entry(entry, Some(dir)),
                None => "the session_rate header".to_string(),
            };
            return Err(format!(
                "save: the session text does not apply — refusing to write a session that would \
                 not open: {named} is refused when the file is read back: {}",
                fault.reason
            ));
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
        // A save rewrote the journal, so a **write** fault is over. A **refusal** is the
        // history's, and this save has just answered the question it rests on: `script_text`
        // rendered **every** entry in the baseline's own spelling and the round trip read
        // them all back, so every entry is now in `session.txt` and none of them is "in the
        // live session and nowhere else". That is why the report is re-derived below rather
        // than cleared by fiat — the re-derivation now comes out empty **for a reason**, and
        // the pool re-point above (which rewrites the `pool` line to a relative path, so a
        // session directory whose own path holds a space spells it fine) is part of that
        // reason.
        if matches!(self.journal_error, Some(JournalFault::Write(_))) {
            self.journal_error = None;
        }
        self.rederive_journal_fault();
        self.last_recovery = None;
        Ok(())
    }

    /// The whole session as a `host v1` script: the state commands that rebuild it,
    /// gestures bracketed, and a pool path **relative to `dir`** when the pool lives
    /// inside it — which is what lets a saved session be moved.
    ///
    /// An entry the form cannot express is refused here, in the save's own words, before
    /// the write — `write_entry` states the fact about the form, the caller says what it
    /// does about it.
    fn script_text(&self, dir: &std::path::Path) -> Result<String, String> {
        let mut out = String::from("host v1\n");
        out.push_str(&format!("session_rate {}\n", self.engine.clock.sample_rate));
        for entry in &self.history {
            write_entry(&mut out, entry, Some(dir)).map_err(|e| {
                format!("save: {e} — refusing to write a session that would lose it")
            })?;
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

        // **The journal is replayed entry by entry**, and an entry the `host v1` form
        // cannot parse is **dropped and reported** like a refused one. Parsing the whole
        // tail as one text made a single unspellable line (a hand-edited journal, a
        // write from another host) a `load` that fails — the session on top of it lost
        // as well, which is the one failure an autosave must never cause.
        //
        // **The journal is a document too, so it is applied as one** — the same walk
        // `from_script` and `rebuild` use. A journal is written by the live path, where
        // a re-mount is legal because the name was released when the unmount *applied*;
        // replaying those lines back to back renders nothing between them, so unwalked
        // the re-mount was refused and **the edit was dropped** (reported as `refused`,
        // so the session opened without it — a silent loss of the user's work).
        //
        // A refused entry is still dropped and reported, never fatal, and the walk is
        // left unconditionally so one bad entry cannot leave the session walking.
        self.engine.enter_walk();
        for entry in journal_entries(&lines) {
            let commands = match parse_script(&format!("host v1\n{entry}")) {
                Ok(commands) => commands,
                Err(e) => {
                    report.refused += 1;
                    if report.refused_reason.is_none() {
                        report.refused_reason = Some(e);
                    }
                    continue;
                }
            };
            for cmd in &commands {
                // A journal entry the session refuses (a stale journal from a crash in
                // the save's window, or a pool that moved) is **dropped and reported**,
                // never fatal. `process`, not `execute`: the journal is a document walk
                // too, so its arrangement cache is rebuilt once at the end (below) rather
                // than once per replayed entry.
                match self.process(cmd) {
                    Ok(()) => report.applied += 1,
                    Err(e) => {
                        report.refused += 1;
                        if report.refused_reason.is_none() {
                            report.refused_reason = Some(e);
                        }
                    }
                }
            }
        }
        self.engine.leave_walk();
        // The journal's edits are in the session now: rebuild the arrangement cache once.
        self.refresh_timeline();
        Ok(report)
    }

    /// Append a committed gesture to the journal — the autosave. Best effort: the
    /// edit is already applied, so a failure is recorded ([`Self::journal_error`]),
    /// not raised.
    fn journal_append(&mut self, entry: &[HostCommand]) {
        let Some(dir) = self.session_dir.clone() else {
            return;
        };
        // The journal records *edits*, with the paths the session actually used (no
        // session directory), and **what it cannot carry it names**: the predicate is
        // `entry_fault`, the same one the outstanding report is re-derived from, so the
        // write and the report can never disagree about what is spellable.
        //
        // **The journal is read back by `parse_script`, so it is checked the way `save`
        // checks its script** — otherwise the autosave is the one way an operand the
        // word-based form cannot carry (a `play` path with whitespace or a `#`) reaches
        // a session directory, and it takes the *whole* session unopenable with it, not
        // just that edit. So the entry is not written, and the refusal is reported: the
        // edit stands in the live session, and a save refuses the same history.
        //
        // The journal's rule stops at *reads back as itself*, and deliberately does not
        // go on to *applies*: an entry the session itself would refuse (a value the
        // engine declines) is written here, and the load side drops it and reports it
        // per entry. `save` is where the stronger rule lives — the baseline has to open.
        let mut text = String::new();
        if let Some(why) = entry_fault(entry, None, &mut text) {
            self.journal_error = Some(JournalFault::Refused(refusal_report(&why, &text)));
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
            // A **write** failure is the journal file's, so a later successful append
            // proves the file is writable again and clears it. A **refusal** is this
            // entry's and is deliberately *not* cleared here: the edit that could not be
            // written is still not durable anywhere, and a save refuses it too, so a
            // later success must not erase the report of what was lost.
            Ok(()) => {
                if matches!(&self.journal_error, Some(JournalFault::Write(_))) {
                    self.journal_error = None;
                }
            }
            Err(e) => {
                self.journal_error = Some(JournalFault::Write(format!(
                    "journal {}: {e}",
                    path.display()
                )));
            }
        }
    }

    /// Re-read the outstanding **refusal** from the history, after the history has
    /// changed under it (a replay: undo, redo, a seek — and a save's pool re-point).
    ///
    /// A refusal is a fact about an *edit*, and the history is what says whether that
    /// edit is still there. Carried across a rebuild unchanged, it becomes a false alarm
    /// — the mirror of the lost alarm it replaced, and the one users actually hit: undo
    /// the offending edit and the session is durable again, yet the status line reads
    /// "autosave failed" after every subsequent edit, over an edit that is no longer in
    /// the session. The only thing that used to clear it was the next `save` — which now
    /// *refuses* a history holding such an entry, so it never would have.
    ///
    /// So the report is **re-derived — not latched, and not merely cleared**: an entry
    /// that is still in the history and still in no durable record keeps its report (which
    /// is what stops a later successful write from erasing it), one that is gone takes it
    /// with it, and one a **redo** put back brings it back. Two guards keep this honest:
    ///
    /// - **no session directory, no report** — there is no autosave to have refused
    ///   anything. An unspellable edit in a directory-less session is a `save` refusal,
    ///   and `save` says so by name when it happens.
    /// - **a write fault stands** — it is the file's, and a history edit is no evidence
    ///   about a file. (A `save` rewrote the file, so it clears that one itself.)
    ///
    /// The scan is one `parse_script` per entry (two for the entries the journal cannot
    /// spell, the second asking the save's spelling), against a rebuild that has already
    /// re-applied every one of them — and `save` pays the same scan, so it is not a new
    /// cost class on the undo path.
    fn rederive_journal_fault(&mut self) {
        if self.session_dir.is_none() || matches!(self.journal_error, Some(JournalFault::Write(_)))
        {
            return;
        }
        // The session directory is the one this method's own guard has just established.
        let dir = self.session_dir.as_deref().expect("checked above");
        self.journal_error = outstanding_refusal(&self.history, dir);
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
            } => {
                // **The finiteness rule is the engine's**, and this arm routes to it: a
                // non-finite value is refused (`parameter '<name>' must be finite, got
                // NaN`) before it is logged, so it never reaches the history, the
                // journal or a saved session. The host keeps no second copy — the engine's
                // `set_param` already speaks for every command in this match, and two
                // rules for one value is one that drifts
                // (`a_non_finite_param_is_refused_at_the_door_and_leaves_nothing_behind`).
                self.engine.set_param(plugin, param, *value)
            }
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
                    // …and the **arranger wiring goes with the bus**: the mixer's
                    // disposer removes only the mixer node, so every `ArrangerNode`
                    // would otherwise stay mounted with its cords dropped — rendering
                    // into nothing, reading its pool file every block, and still
                    // counted by `underruns()`. Only a *wired* arrangement marks the
                    // session as owing a bus (see `arranger_awaiting_bus`): that is
                    // what makes a re-mount reconcile, and what turns a bus-less
                    // window of the timeline into silence rather than a refusal.
                    let was_wired = !self.wired_tracks.is_empty();
                    self.retire_arranger_wiring();
                    if was_wired {
                        self.arrange_dirty = true;
                        self.arranger_awaiting_bus = true;
                    }
                }
                r
            }
            HostCommand::TransportPlay => {
                // Feed the transport tap from the **apply** path, at the frame
                // the command takes effect (the clock's current position, where
                // the next render starts). The sink's absence is what makes a
                // replay silent. `Start` when play begins from the start of the
                // timeline, `Continue` when it resumes from a non-zero
                // position: on the wire, MIDI `Start` tells a follower to
                // return to its song start, so a play from frame 0 is the only
                // one whose position that agrees with; every other play
                // resumes from where we are, which is what `Continue` means.
                //
                // A `play` is an **action**, not state (`is_state`), so a
                // rebuild never re-applies it and never re-feeds this — which is
                // why the tap lives on the session rather than in the history,
                // and why a seek feeds its own re-anchor instead
                // ([`Self::replay_to_kind`]).
                let frame = self.engine.clock.frame();
                let transport = if frame == 0 {
                    engine::Transport::Start
                } else {
                    engine::Transport::Continue
                };
                self.transport.push(frame, transport);
                self.playing = true;
                Ok(())
            }
            HostCommand::TransportStop => {
                let frame = self.engine.clock.frame();
                self.transport.push(frame, engine::Transport::Stop);
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
                // **The doors above are the document's; the file is not** (see
                // `validating`). Opening the WAV to resolve a whole-file region and
                // starting the reader are facts about the *material*: a `play` whose
                // file has moved, or whose ring cannot warm, is a runtime condition
                // and a save must not refuse the session over it. A dry run asks the
                // document's question — the mixer, the channel, one player at a time —
                // and records the state a later `splice` in the same script reads,
                // without opening anything.
                let clip = if self.validating {
                    clip.clone()
                } else {
                    Self::resolve_clip(clip)?
                };
                let intent = PlayerIntent {
                    path: clip.path.to_string_lossy().into_owned(),
                    start: clip.start,
                    len: clip.len,
                    channel: *channel,
                };
                // Open + warm before logging, so a bad path never enters the log
                // (a refused command is never logged) — but only when there is a
                // session to play into.
                let player = if self.validating {
                    None
                } else {
                    let player = FilePlayer::start(clip, DEFAULT_RING_CAPACITY)?;
                    Self::warm_player(&player, DEFAULT_RING_CAPACITY)?; // deterministic, no race
                    Some(player)
                };
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
                let node = PlaybackNode::new(player, mailbox.clone());
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
                // As in `Play`: a dry run reads the document's doors (a player to
                // splice into, the clip's region) and leaves the material alone.
                let clip = if self.validating {
                    clip.clone()
                } else {
                    Self::resolve_clip(clip)?
                };
                let intent = SpliceIntent {
                    at_frame: *at_frame,
                    path: clip.path.to_string_lossy().into_owned(),
                    start: clip.start,
                    len: clip.len,
                    crossfade: *crossfade,
                };
                let incoming = if self.validating {
                    None
                } else {
                    let incoming = FilePlayer::start(clip, DEFAULT_RING_CAPACITY)?;
                    Self::warm_player(&incoming, DEFAULT_RING_CAPACITY)?;
                    Some(incoming)
                };
                let (op, fields) = media_ops::encode_splice(&mut self.media_intern, &intent);
                self.engine.arrange_logged(op, fields)?;
                if let Some(incoming) = incoming {
                    let mailbox = self.player_mailbox.as_ref().expect("checked above");
                    mailbox
                        .lock()
                        .map_err(|_| "player mailbox poisoned")?
                        .push_back(SpliceCmd {
                            at_frame: *at_frame,
                            incoming,
                            crossfade: *crossfade,
                        });
                }
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
                // The id is written as a bare word (`source_tempo <id> <bpm>`), so it
                // takes the same discipline as a declared source's name: a session that
                // cannot spell its ids cannot be reopened.
                if !media::valid_name(source) {
                    return Err(format!(
                        "'{source}' is not usable as a source name (one token, no '#', not an \
                         @frame or snap= modifier)"
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
            HostCommand::Record { take_id, source } => self.record(take_id, source.as_deref()),
            HostCommand::RecordStop => {
                // A finalizing complaint still leaves a take in the pool, so the
                // **declaration** is state either way. `stop_recording` keeps the report
                // on the session before it propagates the complaint, so the declaration
                // is committed even then — a document that dropped it would lose the take
                // on reload — and the complaint is returned *after*, because the caller
                // still has to hear it. `was_recording` keeps a stop with nothing
                // recording from re-declaring the take before it.
                let was_recording = self.recording.is_some();
                let stopped = self.stop_recording();
                if was_recording && let Some(take) = self.last_take.clone() {
                    // The capture was the side effect; the declaration is the state. It
                    // is committed through the same path every other state command uses,
                    // so the save and the journal name the take that just landed — while
                    // `RecordStop` itself stays an action the replay never re-runs (it
                    // would open a device).
                    self.commit_state(vec![HostCommand::Take {
                        take_id: take.take_id.clone(),
                        frames: take.frames,
                        dropped: take.monitor_dropped,
                        channels: take.channels,
                        at_frame: take.at_frame,
                        source: take.source.clone(),
                    }]);
                }
                stopped?;
                Ok(())
            }
            // A take declaration, replayed or loaded: the audio is already pool material,
            // so binding it touches no device and reads no file. The pool ids follow the
            // `{take_id}.ch{k}` convention, which is why the declaration names the shape
            // rather than each source.
            //
            // **The id cache needs no refresh here.** A `Take` is only ever logged after
            // the `Pool` that bound the directory — recording refuses without one — and a
            // replay cannot add files to that directory; `set_pool` is the one place the
            // binding changes and the one place an existing directory is listed. A
            // hand-written document that declares a take with no pool has an empty cache
            // already, and nothing to name takes against.
            HostCommand::Take {
                take_id,
                frames,
                dropped,
                channels,
                at_frame,
                source,
            } => {
                // The declaration names its own pool sources, so `channels` **sizes** the
                // vector built below: a line carrying a width no capture could have
                // recorded must be refused, never sized. The bound is the capture sanity
                // bound — the same rule `SourceAdd` and `media::Capture::start` apply.
                if !(1..=media::capture::CAPTURE_CHANNELS_SANITY).contains(channels) {
                    let sanity = media::capture::CAPTURE_CHANNELS_SANITY;
                    return Err(format!(
                        "take '{take_id}' channels must be 1..={sanity}, got {channels}"
                    ));
                }
                self.status_take(&TakeReport {
                    take_id: take_id.clone(),
                    frames: *frames,
                    monitor_dropped: *dropped,
                    channels: *channels,
                    source: source.clone(),
                    sources: (0..*channels).map(|k| format!("{take_id}.ch{k}")).collect(),
                    sample_rate: self.engine.clock.sample_rate,
                    at_frame: *at_frame,
                });
                Ok(())
            }
            // A declared rig source: record the declaration, touch nothing. The
            // name uses the same discipline as a take id (`media::valid_name`) —
            // a source name has to stay nameable in the whitespace-split log —
            // and a duplicate is a bug, not a merge.
            HostCommand::SourceAdd {
                name,
                kind,
                matcher,
                channels,
                clock,
            } => {
                if !media::valid_name(name) {
                    return Err(format!(
                        "'{name}' is not usable as a source name (one token, no '#', not an \
                         @frame or snap= modifier)"
                    ));
                }
                if self.sources.iter().any(|s| s.name == *name) {
                    return Err(format!(
                        "source '{name}' is already declared — a duplicate address is a bug, \
                         not a merge"
                    ));
                }
                // The matcher is written as a bare operand (`match=…`) and the
                // tokenizer is whitespace-based, so it must be one token the parser
                // reads back: a space or a `#` in it is a session that will not open.
                if !media::valid_name(matcher) {
                    return Err(format!(
                        "the matcher '{matcher}' is not usable in the host v1 log (one token, \
                         no whitespace, no '#')"
                    ));
                }
                if !(1..=media::capture::CAPTURE_CHANNELS_SANITY).contains(channels) {
                    let sanity = media::capture::CAPTURE_CHANNELS_SANITY;
                    return Err(format!(
                        "source '{name}' channels must be 1..={sanity}, got {channels}"
                    ));
                }
                self.sources.push(rig::SourceDecl {
                    name: name.clone(),
                    kind,
                    matcher: matcher.clone(),
                    channels: *channels,
                    clock: *clock,
                });
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
                // A dry run adopts the pool without conforming it — a save must not
                // write material into the directory it is publishing (`note_pool`).
                if self.validating {
                    self.note_pool(dir.clone())?;
                } else {
                    self.set_pool(dir.clone())?;
                }
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
            HostCommand::Export { path, format } => self.export(path, *format),
            HostCommand::Bounce { frames, path } => {
                // Offline bounce: render + drain buffered tails. A capped drain
                // is a *different, truncated* piece, so it fails loud rather than
                // writing a quietly shortened file.
                //
                // **A bounce drives no gear**, for the reason `export` gives: an
                // offline render exists to produce a file, and a `bounce` is the
                // command a script reaches for when it writes the master. The slot
                // is empty across the render — the node's ticks are computed from
                // the block's own frame either way, and its transport tap is left
                // alone until a block has a device to speak through — and the
                // device is back before the refusal below, so a capped drain
                // leaves the live session driving gear as it found it.
                let device = self.detach_midi();
                let rendered = self.render_with_drain(*frames);
                self.restore_midi(device);
                let (out, drain) = rendered?;
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

    /// Retire the arranger wiring: every `ArrangerNode` leaves the graph, and the
    /// underrun counters go with it. `wire_arranger` calls this on every reconcile
    /// and `unmount mixer` calls it when the **bus** goes away — the mixer's own
    /// disposer removes only the mixer node, and `Graph::remove_node` keeps every
    /// cord that does not touch the node it removes, so without this an arranger
    /// would stay mounted with its cords dropped: rendering every block, reading its
    /// pool file, and counted by `underruns()`, all into a bus that is not there.
    fn retire_arranger_wiring(&mut self) {
        let old_nodes: Vec<engine::NodeId> = self.wired_tracks.drain().map(|(_, id)| id).collect();
        for id in old_nodes {
            self.engine.graph.remove_node(id);
        }
        self.arranger_underruns.clear();
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
        let Some(mixer) = self.engine.node_of("mixer") else {
            // **A bus-less window is a shape of the session's own timeline** (the
            // mixer unmounted mid-piece, mounted again later): the wiring was retired
            // with the bus, so there is nothing to build and nothing to say — the
            // window renders the silence a session with no bus renders, and the debt
            // (`arranger_awaiting_bus`) is settled by the next bus. An arrangement
            // with no bus *anywhere* is a different question, and the `Err` under it
            // is that answer.
            if self.arranger_awaiting_bus {
                return Ok(());
            }
            return Err("arrange requires the mixer to be mounted (mount mixer channels=N)".into());
        };

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
        // declared). Retire the previous wiring — nodes *and* their counters — then
        // place each new node.
        self.retire_arranger_wiring();

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
        // The debt is settled: the wiring matches the value again, on a bus.
        self.arranger_awaiting_bus = false;
        Ok(())
    }

    /// The reference host renders the whole bounce into one f32 buffer; bound the
    /// *bytes* so a malformed `bounce 999999999999` cannot OOM (≈1.5 h @48 kHz).
    const MAX_BOUNCE_BYTES: usize = 1 << 30;

    /// Render `frames` from the current position (wiring pending cords/arranger
    /// first). A wiring failure (e.g. the mixer was unmounted after a `play`) is
    /// a clean `Err`, never a panic in a host.
    pub fn render(&mut self, frames: usize) -> Result<Vec<f32>, String> {
        Self::check_bounce_budget("a render", frames)?;
        self.wire_pending()?;
        self.wire_arranger()?;
        Ok(self.engine.render(frames))
    }

    /// Like [`render`](Self::render), but drain buffered tails after the timeline
    /// — the **offline bounce** shape. The live transport uses `render` (a hard
    /// cut): a stopped device is paused, so there is nowhere for a tail to ring.
    pub fn render_with_drain(&mut self, frames: usize) -> Result<(Vec<f32>, DrainOutcome), String> {
        // The drain can add up to MAX_DRAIN_FRAMES on top of `frames`.
        Self::check_bounce_budget("a render", frames.saturating_add(MAX_DRAIN_FRAMES))?;
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

    /// The offline render budget: the master may be stereo (L/R), so a frame costs up
    /// to 2 * 4 bytes, and the whole mix is held in memory (the render is one buffer, not
    /// a stream). ~1 GiB is therefore about **46 minutes** of stereo at 48 kHz — a
    /// deliberate bound (a malformed frame count must not OOM) that a long jam will
    /// eventually reach, which is why the refusal names the *command* the user ran and
    /// the memory reason, not just a number.
    fn check_bounce_budget(what: &str, frames: usize) -> Result<(), String> {
        let budget = frames
            .saturating_mul(std::mem::size_of::<f32>())
            .saturating_mul(2);
        if budget > Self::MAX_BOUNCE_BYTES {
            return Err(format!(
                "{what} of {frames} frames (~{:.0} s of stereo) exceeds the ~{:.0} MiB \
                 in-memory render budget",
                frames as f64 / 48_000.0,
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

    /// What the last [`HostCommand::Export`] wrote: the measured length, format,
    /// peak and RMS. `None` until one runs (and it is *not* cleared by a later
    /// failure — a shell reports the last thing that was written).
    pub fn last_export(&self) -> Option<&media_ops::ExportRecord> {
        self.last_export.as_ref()
    }

    /// The last seek and how it was served: `(target, warmed)`. `warmed` means the jump
    /// rendered only a [`SEEK_WARMUP_FRAMES`] run-in instead of the timeline from 0.
    pub fn last_seek(&self) -> Option<(u64, bool)> {
        self.last_seek
    }

    /// **Export the whole arrangement** to `path` — `bounce`'s deliverable sibling.
    ///
    /// The render runs on a **rebuilt clone** of the session, so an export is
    /// side-effect free: the transport does not move, the graph is not disturbed, and a
    /// take in progress is not finalized by a file-write gesture (the gate measured the
    /// playhead jumping to the arrangement's end, and `replay_to` stopping a recording).
    /// The clone applies **every** state command at the present instant (order decides),
    /// so the export is the session's *current* value rendered from the start — a
    /// limiter mounted halfway through a session masters the whole file rather than
    /// silently missing from the deliverable. The length is the arrangement's own
    /// ([`media::Timeline::end_frame`]), never a hand-computed count — and that call is
    /// fallible on purpose: an arrangement holding a clip whose span cannot be
    /// represented is **refused by name** rather than exported at a wrapped length.
    /// The output is
    /// **f32** (bit-exact) or **s16 with fixed-seed TPDF dither** (reproducible), and a
    /// mix whose peak exceeds full scale is **refused with nothing written** — "never a
    /// clipped file" is a property of the command, not of the user's care.
    pub fn export(&mut self, path: &std::path::Path, format: ExportFormat) -> Result<(), String> {
        // **An export must never drive gear** — it is side-effect free by
        // contract. The slot is emptied across the clone's rebuild *and* its
        // offline render, then the device goes back (even when the export
        // fails), so the live session keeps driving gear afterwards.
        let device = self.detach_midi();
        let result = self.export_detached(path, format);
        self.restore_midi(device);
        result
    }

    /// `export` with the sink slot already detached — see the wrapper above.
    fn export_detached(
        &mut self,
        path: &std::path::Path,
        format: ExportFormat,
    ) -> Result<(), String> {
        // The clone: current state, clock at 0, and no rendering while the state is
        // applied (`at_now`), so this is cheap even for a long session.
        let mut fresh = self.rebuild(None)?;
        // `?` on the arrangement's end: a clip whose span cannot be represented has no
        // export length, and the two wrong answers (a wrapped sum, a `Frame::MAX`-frame
        // render) are both worse than a refusal that names the clip. No op admits such a
        // clip — this is the `Timeline` that came back from the session, named.
        let frames = self.arrangement()?.end_frame()?;
        if frames == 0 {
            return Err("nothing to export: the arrangement has no clips".into());
        }
        if frames > usize::MAX as u64 {
            return Err(format!(
                "the arrangement is {frames} frames long — too long"
            ));
        }
        let (mut out, drain) = fresh.render_with_drain(frames as usize)?;
        if drain.capped {
            return Err(format!(
                "export drain hit the {MAX_DRAIN_FRAMES}-frame bound with output still pending — \
                 the tail would be truncated"
            ));
        }
        // Measure, counting non-finite samples instead of folding them away: `f32::max`
        // *ignores* a NaN, so a naive peak would report 0 for a mix of NaNs and the
        // export would write them.
        let mut peak = 0.0f32;
        let mut sum_sq = 0.0f64;
        let mut non_finite = 0usize;
        for x in &out {
            if x.is_finite() {
                peak = peak.max(x.abs());
                sum_sq += (*x as f64) * (*x as f64);
            } else {
                non_finite += 1;
            }
        }
        let rms = (sum_sq / out.len().max(1) as f64).sqrt() as f32;
        if non_finite > 0 {
            return Err(format!(
                "export refused: {non_finite} of {} samples are not finite (a NaN or infinity \
                 reached the mix) — the file was not written",
                out.len()
            ));
        }
        if peak > 1.0 {
            // Refuse rather than write it: a file whose samples leave full scale is
            // silently clipped by every 16-bit player and by most converters. An
            // existing file at `path` is left alone — a failed export never destroys
            // what an earlier one wrote.
            let db = if peak > 0.0 {
                20.0 * peak.log10()
            } else {
                f32::NEG_INFINITY
            };
            return Err(format!(
                "export would clip: the mix peaks at {peak:.4} ({db:.1} dBFS) — lower the mix or \
                 mount a master chain with a ceiling; the file was not written"
            ));
        }
        let rate = fresh.engine.clock.sample_rate;
        let channels = fresh.engine.graph.out_channels().max(1) as u16;
        match format {
            ExportFormat::F32 => {
                let mut w = media::WavWriter::create_float(path, rate, channels)?;
                w.write(&out)?;
                w.finalize()?;
            }
            ExportFormat::S16 => {
                // Fixed-seed TPDF dither: reproducible, and the quantisation error's
                // mean and its correlation with the signal both go to zero.
                let mut dither = media::TpdfDither::new(media::DITHER_SEED);
                dither.quantize_s16(&mut out);
                let mut w = media::WavWriter::create(path, rate, channels)?;
                w.write(&out)?;
                w.finalize()?;
            }
        }
        let record = media_ops::ExportRecord {
            frames,
            format: format.code(),
            peak,
            rms,
            drained_frames: drain.tail_frames,
        };
        let (op, fields) = media_ops::encode_export(&record);
        self.engine.arrange_logged(op, fields)?;
        self.media
            .lock()
            .map_err(|_| "media session poisoned")?
            .exports
            .push(record.clone());
        self.last_drain = drain;
        self.last_export = Some(record);
        self.media_commands += 1;
        Ok(())
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

    /// The MIDI clock-out status: the port being driven (configuration, not
    /// state — `None` when the process asked for no device) and the
    /// `clock_out` plugin's overflow counter, read back through the plugin's
    /// `"clock_out.overflows"` context service (the mixer's `mixer.meters`
    /// pattern). The counter is **always** surfaced — ticks are generated
    /// whether or not a sink is present, so a dropped tick is loud even
    /// gearless.
    pub fn midi_status(&self) -> MidiOutStatus {
        MidiOutStatus {
            port: self.midi_port.clone(),
            overflows: self
                .engine
                .ctx
                .get::<Arc<std::sync::atomic::AtomicU64>>(plugins::CLOCK_OUT_OVERFLOWS_KEY)
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(0),
        }
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

/// The MIDI output this process asked for, if any: `--midi-out <port>` on the
/// command line (the host binary's flag, documented in `src/main.rs`) or
/// `DSH_MIDI_OUT=<substring>` in the environment. The operand is a
/// case-insensitive **substring** of a port name (`media::midi::MidiOut::open`
/// fails loudly listing what matched otherwise).
///
/// This is **configuration, not session state**: which device is attached is a
/// fact about the machine the process runs on, so it is read once per session
/// construction here and never logged — a replayed session carries the
/// `mount clock_out` declaration but never the device. A port that cannot be
/// opened is reported on stderr and the session runs **silent**: refusing to
/// start over a missing device would make the session hardware-dependent,
/// which the purity rule exists to prevent.
fn midi_out_from_process() -> (Option<String>, Option<SharedMidiSink>) {
    let mut args = std::env::args().skip(1);
    let mut requested: Option<String> = None;
    while let Some(arg) = args.next() {
        if arg == "--midi-out" {
            requested = args.next();
        } else if let Some(port) = arg.strip_prefix("--midi-out=") {
            requested = Some(port.to_string());
        }
    }
    let requested = requested.or_else(|| std::env::var("DSH_MIDI_OUT").ok());
    let Some(port) = requested else {
        return (None, None);
    };
    match MidiOut::open(&port) {
        Ok(out) => (
            Some(port),
            Some(Arc::new(Mutex::new(Some(
                Box::new(out) as Box<dyn MidiSink>
            )))),
        ),
        Err(e) => {
            eprintln!("host: the requested MIDI output '{port}' could not be opened: {e}");
            eprintln!("host: continuing without a MIDI output (clock-out stays silent)");
            (None, None)
        }
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

/// A command the document walk **refused**: which one (its index in the script), and
/// why. A *reader* only needs the reason — that is what `from_script` returns — but a
/// **writer** has to name the edit it is refusing to write
/// ([`HostSession::save`](crate::HostSession::save)), and an index alone is not a name.
struct ScriptFault {
    index: usize,
    reason: String,
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
        // A load *is* the session taking over the machine, so it gets the process's own
        // MIDI output (a rebuilt session, and a save's self-check, do not — see
        // `from_script_with`).
        let (midi_port, midi_out) = midi_out_from_process();
        Self::from_script_with(commands, midi_out, midi_port, false).map_err(|fault| fault.reason)
    }

    /// [`Self::from_script`] over an explicit MIDI output sink, saying **which**
    /// command the walk refused rather than only why, and in a **mode**: a reader
    /// (`validating: false`) or a **dry run** (`true`, the mode `save` checks its own
    /// output in).
    ///
    /// The index is what makes this a *writer's* check: `save` builds its own output
    /// through here, so its refusal can name the edit that would not apply, and not
    /// merely the reason it would not. A `None` sink is a silent session — a save
    /// validates the file, so it must not open a device, and must not take the live
    /// session's (`load` opens its own because it adopts the session).
    ///
    /// **A dry run is the same walk with the material left out** (see
    /// [`HostSession::validating`]): every value door runs, because it is the
    /// reader's own door; nothing opens a file, spawns a thread, warms a decoder,
    /// writes a pool or renders. The earlier version of this check built the
    /// session outright, and building *is* running — a `play` in the script
    /// started a `FilePlayer` and blocked on `warm_player`'s ten-second deadline,
    /// and a `play` whose file had moved failed the save. Both are facts about the
    /// material, not about the document.
    fn from_script_with(
        commands: &[HostCommand],
        midi_out: Option<SharedMidiSink>,
        midi_port: Option<String>,
        validating: bool,
    ) -> Result<Self, ScriptFault> {
        let rate = commands
            .iter()
            .find_map(|cmd| match cmd {
                HostCommand::SessionRate { hz } => Some(*hz),
                _ => None,
            })
            .unwrap_or(DEFAULT_SAMPLE_RATE);
        if rate == 0 {
            return Err(ScriptFault {
                index: 0,
                reason: "session_rate must be non-zero".into(),
            });
        }
        let mut session = HostSession::new_at_with(rate, midi_out, midi_port);
        session.validating = validating;
        // **The script is a document, so it is applied as a document walk**
        // ([`Engine::enter_walk`]) — the same mechanism `rebuild` uses, not a second
        // one. An unplaced command renders nothing, so the apply queue never drains
        // between the script's lines: a `mount … unmount … mount` the live path
        // accepted and recorded (and a `save` therefore wrote) was refused here as a
        // second instance, which made such a session **unopenable** — the platform
        // wrote files it could not read back.
        //
        // Scoped with **no `?` between the enter and the leave**, so a refused command
        // cannot leave a half-built session walking (and the caller keeps its own
        // session either way: `load_session` only adopts a session that built whole).
        session.engine.enter_walk();
        let applied = (|| -> Result<(), (usize, String)> {
            for (index, cmd) in commands.iter().enumerate() {
                // **`process`, not `execute`.** Both apply the command and record it in
                // the history — a script built with neither would have the *state* but an
                // empty history, so the session could not be undone and saving it again
                // would write only what happened after the load. But `execute` also
                // rebuilds the snapshot's arrangement cache, and a load walks the whole
                // document: the cache is built **once** after the walk, below, not once
                // per replayed step. (`execute` is a thin wrapper: `process` plus that
                // refresh. The journal is appended only when a session directory is set,
                // and a session being built or loaded has none.)
                session.process(cmd).map_err(|e| (index, e))?;
            }
            Ok(())
        })();
        session.engine.leave_walk();
        if let Err((index, reason)) = applied {
            return Err(ScriptFault { index, reason });
        }
        // A loaded session's arrangement cache is built once, here: the replay applied the
        // state commands through `process`, which does not refresh per step.
        session.refresh_timeline();
        Ok(session)
    }

    /// Apply a single command onto this persistent session — the incremental
    /// form of [`run_script`], for a live UI that edits one op at a time. A
    /// refused op returns `Err` and changes nothing; the session stays usable.
    pub fn execute(&mut self, cmd: &HostCommand) -> Result<(), String> {
        let result = self.process(cmd);
        // The arrangement cache is rebuilt **once per applied state command**, here and
        // not inside `process`: `process` is also the replay path a rebuild walks, and
        // there the adoption refreshes once instead of once per replayed step.
        if result.is_ok() && cmd.is_state() {
            self.refresh_timeline();
        }
        result
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

    /// Build a fresh session from the state-command history.
    ///
    /// `upto = Some(frame)` is the **seek** reconstruction: state that takes effect after
    /// `frame` is not yet in force there, and the rebuild renders the timeline forward to
    /// `frame`. The skip is per **entry**, so a gesture is never half-applied.
    ///
    /// `upto = None` is the **export** reconstruction: every state command is applied
    /// *at the present instant* (its frame placement removed, order preserved) and the
    /// clock stays at 0. That is what makes an export the session's **current** value
    /// rendered from the start: a compressor or limiter mounted halfway through a
    /// session masters the whole file, rather than silently missing from it — which the
    /// gate showed as an export clipping where the audible session did not.
    ///
    /// Pure with respect to `self`: the caller decides what to adopt and what to carry.
    ///
    /// The history is re-issued as a **document walk** ([`Engine::enter_walk`]): while
    /// the state is being applied, the one-instance-per-name rule is the *history's*
    /// question, not the engine's — which is what lets a `mount … unmount … mount`
    /// gesture survive a rebuild (see [`Self::rebuild_prefix`], which walks the same way).
    ///
    /// The rebuilt session **shares this session's slot object** (and the port
    /// name), so the gear keeps being driven after a seek — but the rebuild's
    /// renders happen with the slot **emptied by the caller**: nothing is sent
    /// across a rebuild (the seek-and-rebuild decision), and the export clone
    /// — which renders after `rebuild` returns — keeps the slot empty through
    /// its own render too.
    fn rebuild(&self, upto: Option<u64>) -> Result<HostSession, String> {
        let mut rebuilt = HostSession::new_at_with(
            self.engine.clock.sample_rate,
            Some(self.midi_slot.clone()),
            self.midi_port.clone(),
        );
        // **The history is a document, so it is re-issued as a document walk** (see
        // [`Engine::enter_walk`]). The loop below renders nothing between the commands
        // in the `at_now` case — and nothing at all for an unplaced command — so the
        // engine's apply queue never drains: a `mount … unmount … mount` history that
        // the live path accepted and recorded (the mixer going away mid-session and
        // coming back is enough) was refused here as a second instance, which took
        // `export` and every seek with it. Under the walk, "is this name live?" is
        // asked of the history, which is the contract [`Engine::replay_from`] keeps
        // for a log.
        //
        // Scoped deliberately: entered and left with **no `?` between them**, so a
        // refused state command cannot leave the rebuilt session walking.
        rebuilt.engine.enter_walk();
        let walked = (|| -> Result<(), String> {
            for entry in &self.history {
                match upto {
                    Some(frame) => {
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
                    None => {
                        // `at_now`: order decides, not timeline position, so applying the
                        // state does not render the clock through the piece.
                        for cmd in entry {
                            rebuilt.process(&cmd.at_now())?;
                        }
                    }
                }
            }
            Ok(())
        })();
        rebuilt.engine.leave_walk();
        walked?;
        rebuilt.render_to(upto.unwrap_or(0))?;
        rebuilt.playing = self.playing;
        rebuilt.redo = self.redo.clone();
        // **Observability survives a rebuild.** A replay replaces the session, so the
        // session directory (and the autosave state) has to come across or the journal
        // would silently stop after the first undo/seek; the last export's report comes
        // across too, because a shell's readout should not vanish under a seek.
        rebuilt.session_dir = self.session_dir.clone();
        rebuilt.journal_error = self.journal_error.clone();
        rebuilt.last_recovery = self.last_recovery.clone();
        rebuilt.last_export = self.last_export.clone();
        rebuilt.last_drain = self.last_drain;
        Ok(rebuilt)
    }

    /// **Whether a seek to `frame` may start from a short run-in** instead of rendering
    /// the timeline from 0.
    ///
    /// It may when the *whole session state* is already in force at the run-in's start
    /// (`frame - SEEK_WARMUP_FRAMES`): a state command placed later (a compressor mounted
    /// mid-piece, an edit stamped after the target) has to be applied at its own frame,
    /// which only a full replay can place. The legacy recorder-player path (`play`/
    /// `splice`) is excluded too — its nodes stream with their own buffering rather than
    /// deriving the read from the block frame.
    fn can_warm_seek(&self, frame: u64) -> bool {
        if frame < SEEK_WARMUP_FRAMES {
            return false;
        }
        let start = frame - SEEK_WARMUP_FRAMES;
        let placed_late = self.history.iter().any(|entry| {
            entry.iter().any(|cmd| {
                matches!(cmd, HostCommand::Play { .. } | HostCommand::Splice { .. })
                    || cmd.at_frame().is_some_and(|at| at > start)
            })
        });
        !placed_late
    }

    /// Rebuild the session from its state-command history and render back to
    /// `frame` — the deterministic reconstruction `seek_to` and undo/redo share.
    /// The transport's playing state and the redo stack survive the rebuild.
    fn replay_to(&mut self, frame: u64) -> Result<(), String> {
        self.replay_to_kind(frame, true)
    }

    /// `replay_to` with the report's honesty kept: `is_seek` is false when the rebuild is
    /// an **edit** (undo/redo), so `last_seek` keeps naming the last real jump instead of
    /// claiming an edit was a seek.
    fn replay_to_kind(&mut self, frame: u64, is_seek: bool) -> Result<(), String> {
        // A rebuild replaces the session, which would drop a take in progress. Stop it
        // properly first: the take is finalized (its WAV + peaks land in the pool) **and**
        // reported, instead of vanishing with the old session.
        if self.recording.is_some() {
            let _ = self.stop_recording();
        }
        // **Seek at scale**: a jump into a long piece does not need the timeline rendered
        // from 0. The state is applied (cheaply — a state command whose frame is already
        // past applies at once), the clock is placed just before the target, and a short
        // run-in gives every stateful node the state a replay would have had. Readers need
        // nothing (their read is a pure function of the block frame), so the run-in only
        // has to cover the bus effects' memory. `warm` reports which path ran.
        let warm = self.can_warm_seek(frame);
        // **Nothing is sent across a rebuild** (the seek-and-rebuild decision):
        // the device leaves the slot for the duration of the reconstruction's
        // renders — rebuild, warm-up run-in, and render-to-target alike — and
        // goes back before `carry_over`, so the adopted session (which shares
        // the same slot object) drives gear again on its next render.
        let device = self.detach_midi();
        let rebuilt = (|| -> Result<HostSession, String> {
            Ok(if warm {
                let mut session = self.rebuild_prefix(frame - SEEK_WARMUP_FRAMES)?;
                session.engine.seek(frame - SEEK_WARMUP_FRAMES);
                session.render_to(frame)?;
                session
            } else {
                self.rebuild(Some(frame))?
            })
        })();
        self.restore_midi(device);
        let mut rebuilt = rebuilt?;
        // **A seek re-anchors a playing follower**, because the rebuilt session
        // gets a brand-new tap (`new_at_with`) and nothing else ever re-feeds it:
        // `TransportPlay` is an *action*, so it is not state, is not in the history,
        // and `rebuild` never re-applies it. Without this the adopted session is
        // still `playing` (`rebuild` carries the flag across), so its ticks simply
        // resume at the new position — and a pulse-counting follower keeps the
        // count it already had, a permanent phase error that **no later message
        // could correct**, because a playing transport is never followed by a
        // `play`. The entry goes into the *rebuilt* log and is sent by the first
        // render after the adoption, with the device already back in the slot: the
        // "nothing is sent across a rebuild" rule is untouched.
        //
        // **The message is the same one a `play` at this frame sends**: `Start`
        // from frame 0, `Continue` from anywhere else. That distinction is the
        // whole point of the two messages, so it cannot be traded away for
        // "re-anchoring": on the wire `Start` does not mean *re-anchor*, it means
        // **"return to song start"** — which is why the `TransportPlay` arm above
        // only sends it at frame 0. At 24 000 a `Start` would not re-zero a
        // follower against the session, it would tell it to jump to its own song
        // top while the session sits half a second in: a second, larger error
        // than the phase one this message exists to correct. So a re-anchor
        // re-declares the *only* thing 24 PPQN can honestly state at a non-zero
        // frame — "keep running, the position moved" — which is `Continue`, and
        // a seek that lands on frame 0 gets the `Start` that frame deserves.
        //
        // What that buys is a follower that re-bases its beat count on a
        // `Continue` after a jump, and what it does not buy is a stated
        // position: only Song Position Pointer can do that, and it is deferred.
        //
        // A **seek only**: undo and redo rebuild *at the current frame*
        // (`replay_to_kind(self.engine.clock.frame(), false)`) over an
        // arrangement edit, which cannot move a tick, so the follower is still in
        // phase and a re-anchor there would be a message about nothing.
        //
        // A **stopped** transport feeds nothing: the follower was stopped by the
        // `Stop` the old session already sent, and a rebuild is not what stopped it.
        if is_seek && self.playing {
            let transport = if frame == 0 {
                engine::Transport::Start
            } else {
                engine::Transport::Continue
            };
            rebuilt.transport.push(frame, transport);
        }
        rebuilt.carry_over(self, warm, frame, is_seek);
        *self = rebuilt;
        Ok(())
    }

    /// Take the device out of the process-lifetime sink **slot** — the detach
    /// side of "empty the slot, render the rebuild, restore the device". The
    /// lock is held only for the `take`; while the slot is empty, every
    /// `clock_out` node sharing it renders its schedule and sends nothing.
    fn detach_midi(&self) -> Option<Box<dyn MidiSink>> {
        self.midi_slot
            .lock()
            .expect("the midi.out sink slot is not poisoned")
            .take()
    }

    /// Put the device back into the sink slot (the refill side of
    /// [`Self::detach_midi`]). The lock is held only for the assignment.
    fn restore_midi(&self, device: Option<Box<dyn MidiSink>>) {
        *self
            .midi_slot
            .lock()
            .expect("the midi.out sink slot is not poisoned") = device;
    }

    /// Adopt a rebuilt session: the fields a replay must carry across (the playing state,
    /// the redo stack, persistence, and the observability a shell reads), plus the report
    /// of how this seek was served.
    fn carry_over(&mut self, from: &mut HostSession, warm: bool, frame: u64, is_seek: bool) {
        self.last_take = from.last_take.take();
        self.playing = from.playing;
        // The MIDI sink slot is configuration carried across (the same arc —
        // in practice the rebuilt session already shares it, so its node keeps
        // seeing the host's refills), with the port name that reports it.
        self.midi_slot = from.midi_slot.clone();
        self.midi_port = from.midi_port.clone();
        self.redo = from.redo.clone();
        self.session_dir = from.session_dir.clone();
        // …and then **re-read against the history this rebuilt session actually has**,
        // because that is the session the fault describes: an undo that removed the
        // offending entry has nothing left to report (`rederive_journal_fault`).
        self.journal_error = from.journal_error.clone();
        self.rederive_journal_fault();
        self.last_recovery = from.last_recovery.clone();
        self.last_export = from.last_export.clone();
        self.last_drain = from.last_drain;
        // A seek reports itself; an **edit** (undo/redo) carries the last real seek's
        // report across rather than blanking it — the shell's "why was that fast?" answer
        // should survive an unrelated undo.
        self.last_seek = if is_seek {
            Some((frame, warm))
        } else {
            from.last_seek
        };
        // The rebuilt session replayed the history through `process`, which refreshes
        // nothing per step; its adopted arrangement is cached once, here.
        self.refresh_timeline();
    }

    /// The **full replay** seek: rebuild from the history and render the timeline from 0.
    /// The warm-up test uses it as the reference the warmed path must equal, byte for
    /// byte — the shipped path reaches it through `replay_to`'s fallback branch, which is
    /// why this is a test-only entry point rather than a second public API.
    #[cfg(test)]
    fn replay_full(&mut self, frame: u64) -> Result<(), String> {
        if self.recording.is_some() {
            let _ = self.stop_recording();
        }
        // Same detach/refill as `replay_to_kind`: the full replay's renders
        // must not reach the gear either.
        let device = self.detach_midi();
        let rebuilt = self.rebuild(Some(frame));
        self.restore_midi(device);
        let mut rebuilt = rebuilt?;
        rebuilt.carry_over(self, false, frame, true);
        *self = rebuilt;
        Ok(())
    }

    /// Rebuild the session with every state command applied — **without rendering the
    /// timeline** (state commands placed in the future apply at once, because the clock
    /// starts at 0 and the placement render is skipped). This is the warm-up seek's
    /// starting state; `can_warm_seek` has already proved that nothing placed after the
    /// run-in's start is in the history, so "at once" is exactly "in force".
    fn rebuild_prefix(&self, _start: u64) -> Result<HostSession, String> {
        // The slot object carries across (the gear is driven again after the
        // jump, once the caller's refill lands) — the same decision the seek
        // reconstruction in `rebuild` makes.
        let mut rebuilt = HostSession::new_at_with(
            self.engine.clock.sample_rate,
            Some(self.midi_slot.clone()),
            self.midi_port.clone(),
        );
        // **A document walk, like `rebuild`** — the run-in renders nothing before it,
        // so the apply queue never drains and the history's own `mount … unmount …
        // mount` would be refused as a second instance. Scoped with no `?` between
        // the enter and the leave.
        rebuilt.engine.enter_walk();
        let walked = (|| -> Result<(), String> {
            // **Tempo is the exception to `at_now`.** It is frame-placed *value* state (the
            // tempo map is a function of the frame), so a change at 60 s must sit at 60 s in
            // the map even when the run-in starts later — otherwise the audio would be right
            // (the render reads frames) but every beat reading, the ruler and the shell's
            // position readout would disagree with a full replay. The segment is pushed
            // directly (no timeline render); the logged command is applied too, so the folded
            // `params` value and the rebuilt log stay identical to the full path's.
            for entry in &self.history {
                for cmd in entry {
                    if let HostCommand::SetTempo {
                        bpm,
                        beats_per_bar,
                        at_frame,
                    } = cmd
                    {
                        // **The clock only ever moves forward here**, exactly as it does
                        // on the live path: `process` renders up to a command's placement
                        // when it is in the future, and when it is in the *past* it leaves
                        // the clock where it is, so `set_tempo` stamps the change at the
                        // current frame rather than at the stale one. Seeking to the
                        // stated frame unconditionally walked the clock **backwards** over
                        // such a command, and every `at_now` command after it was then
                        // stamped *before* the ones before it — a document the walk
                        // refuses as frame-inverted, on a history the live path wrote and
                        // accepted. `max` is that rule, in one expression.
                        let at = at_frame.unwrap_or(0).max(rebuilt.engine.clock.frame());
                        // Place the clock at that frame (no render) and let the
                        // engine log and **schedule** it there; the run-in render delivers
                        // the event at its frame, which pushes the segment — exactly the
                        // sequence a full replay performs, so the map ends up identical
                        // (pushing here as well would double every segment).
                        rebuilt.engine.seek(at);
                        rebuilt.engine.set_tempo(*bpm, *beats_per_bar)?;
                        continue;
                    }
                    // Everything else: `at_now` for the same reason the export uses it — the
                    // state's *order* decides the value, and the clock must not walk the piece.
                    rebuilt.process(&cmd.at_now())?;
                }
            }
            Ok(())
        })();
        rebuilt.engine.leave_walk();
        walked?;
        Ok(rebuilt)
    }

    /// Undo the most recent **arrangement** edit: drop it from the state history
    /// and rebuild **to the current position**, so the playhead does not jump and
    /// the pool/mounts survive. `Ok(false)` when there is nothing to undo.
    ///
    /// **A refused replay changes nothing** — the contract `seek_to` states, owed
    /// here too: see the restore below.
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
        // The rebuild re-applies the *remaining* history, so it can be refused for a
        // reason this edit has nothing to do with — the ordinary one being a pool
        // directory that moved or was deleted. Then the edit goes back where it came
        // from, in *both* stacks, which is what keeps the history describing the
        // session: left out of it, the next `save` would write a script the live
        // arrangement no longer matches, `can_undo`/`can_redo` would report a state
        // that is not there, and the next `undo` would revert a *different* edit. The
        // copy is one gesture, so it is cheaper than a snapshot of the whole history.
        let restore = undone.clone();
        let redo_len = self.redo.len();
        self.redo.push((pos, undone));
        let replayed = self.replay_to_kind(self.engine.clock.frame(), false);
        if let Err(e) = replayed {
            // `truncate` and not a `pop`: a take finalized on the way into the rebuild
            // clears the branch for itself (`commit_state`), and a refusal undoes
            // neither that nor the edit above it.
            self.redo.truncate(redo_len);
            self.history.insert(pos, restore);
            return Err(e);
        }
        Ok(true)
    }

    /// Redo the most recently undone edit, re-inserted at its original history
    /// position so the reconstruction is faithful. `Ok(false)` when nothing is
    /// undone. A **refused replay** takes the entry back off the history and leaves
    /// the redo branch whole, for the reason [`Self::undo`] gives.
    pub fn redo(&mut self) -> Result<bool, String> {
        let Some((pos, cmd)) = self.redo.pop() else {
            return Ok(false);
        };
        let at = pos.min(self.history.len());
        self.history.insert(at, cmd);
        let replayed = self.replay_to_kind(self.engine.clock.frame(), false);
        if let Err(e) = replayed {
            let cmd = self.history.remove(at);
            self.redo.push((pos, cmd));
            return Err(e);
        }
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
    /// Commit one state entry to the session: the autosave journal, the history, and the
    /// redo reset (a new state change makes any undone branch unreachable).
    ///
    /// Every state command reaches here from `process`. The other caller is the take
    /// commit: a finished capture is declared by `record stop`, which is an *action*, so
    /// the declaration is produced by an action rather than by a command of its own — and
    /// it still has to reach the document the save and the journal are built from.
    fn commit_state(&mut self, entry: Vec<HostCommand>) {
        // Autosave: the journal is appended per committed gesture, so a crash costs at
        // most the gesture in flight.
        self.journal_append(&entry);
        self.history.push(entry);
        self.redo.clear();
    }

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
            if channels > MIXER_CHANNELS_SANITY {
                return Err(format!(
                    "mixer channels {channels} exceeds the sanity bound {MIXER_CHANNELS_SANITY}"
                ));
            }
            // Defer committing the channel count until apply succeeds: a refused
            // re-mount must not overwrite the live mixer's channel count (a
            // refused op changes nothing).
            pending_mixer_channels = Some(channels);
        }
        if let Some(frame) = cmd.at_frame() {
            let now = self.engine.clock.frame();
            // **A dry run renders nothing** (`validating`, the mode `save` checks its
            // output in). Reaching a command's frame is *rendering* — which wires the
            // arranger, opens every pool source through a reader thread and warms it.
            // That is the session running, not the document being read, and a save
            // must not do it: a render that cannot wire (the mixer went away, a source
            // moved) would fail the save for a reason the document says nothing about.
            // No verdict is lost by not rendering: every door this walk reaches —
            // `mount`/`patch`/`set_param`'s scheduled-or-mounted rule, the mixer's
            // channel count, the clip editor's ops — asks a question of the *sequence*
            // of commands, and the sequence is walked whole either way.
            if frame > now && !self.validating {
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
                self.commit_state(entry);
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
    /// The declared source this take captured, when one was named. `None` is the
    /// default input — the pre-binding behaviour, kept readable.
    source: Option<String>,
    /// The pool source ids the take will produce (`take.ch0`, `take.ch1`, …).
    sources: Vec<String>,
    /// Where the capture started on the session timeline (the transport frame when
    /// `record` was issued) — the take's origin, which the declaration keeps.
    at_frame: u64,
}

/// A finished take: what the shell needs to say what happened, and what the pool
/// listing will show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeReport {
    pub take_id: String,
    /// Session frames written per channel.
    pub frames: u64,
    /// Monitor frames a monitored ring could not take (best-effort only; **zero when
    /// nothing monitored the take**). Not take loss: the pool write is authoritative.
    pub monitor_dropped: u64,
    pub channels: usize,
    /// The declared source this take captured, if one was named. `None` means the
    /// default input — the take still replays, it just cannot say what it heard.
    pub source: Option<String>,
    /// The pool source ids to place on a track (`{take_id}.ch{k}`).
    pub sources: Vec<String>,
    pub sample_rate: u32,
    /// The transport frame the capture started at (the take's origin).
    pub at_frame: u64,
}

/// A take in progress, for a live indicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingStatus {
    pub take_id: String,
    pub frames: u64,
    /// Monitor frames lost so far; zero while nothing monitors the take.
    pub monitor_dropped: u64,
    pub channels: usize,
    /// The declared source this take is capturing, when one was named.
    pub source: Option<String>,
}

/// Why a journal write did not happen (see [`HostSession::journal_error`]). The edit
/// itself is never *failed* by either — it is already applied and logged — so both are
/// reported rather than raised. They differ in **what they belong to and what clears
/// them**, which is why they are two variants and not one message.
#[derive(Debug, Clone, PartialEq, Eq)]
enum JournalFault {
    /// The entry is **not in the `host v1` text form**, or does not read back as itself,
    /// so it is not written. The edit stands in the live session and a save refuses the
    /// same history — so no later successful append clears this: the edit is still not
    /// durable anywhere. The report is **re-derived from the history**
    /// (`outstanding_refusal`) whenever the history changes, so it dies with its subject:
    /// undoing the edit clears it, and redoing it brings it back. It is made only where
    /// **neither** write path can carry the entry, so its claim ("in the live session and
    /// nowhere else, and a save refuses the history as well") is true of both.
    Refused(String),
    /// The **journal file** refused the write or the flush. The next successful append —
    /// or a save, which rewrites the journal — proves the file is writable again.
    Write(String),
}

impl JournalFault {
    fn message(&self) -> &str {
        match self {
            JournalFault::Refused(message) | JournalFault::Write(message) => message,
        }
    }
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
    /// save's window, or a path that moved), plus entries the `host v1` form cannot
    /// parse at all. Dropped and reported, not fatal: refusing to open a session because
    /// of an autosave line is the worse failure.
    pub refused: usize,
    /// The first refusal, for a shell to show.
    pub refused_reason: Option<String>,
}

impl JournalRecovery {
    /// Whether there is anything to say: at least one journal command applied, was
    /// torn, or was refused. A load of a cleanly-saved session (an empty journal)
    /// has no story, and a caller uses this to stay quiet rather than print zeros.
    pub fn has_story(&self) -> bool {
        self.applied > 0 || self.torn_lines > 0 || self.refused > 0
    }

    /// The recovery as one line, built here on the caller's thread — the same
    /// read-side discipline as [`ApplyFault::describe`]: the record is data, the
    /// sentence is a read. Counts first (they are the facts), then the first
    /// refusal's own words, because a refusal worth showing is worth quoting.
    pub fn describe(&self) -> String {
        let mut line = format!(
            "journal: {} applied, {} torn, {} refused",
            self.applied, self.torn_lines, self.refused
        );
        if let Some(reason) = &self.refused_reason {
            line.push_str(" — first refusal: ");
            line.push_str(reason);
        }
        line
    }
}

/// Write one history **entry** (one gesture) as script lines. A multi-command entry
/// is bracketed with `group begin`/`group end`, so the gesture structure survives a
/// save (and replays as one undo step).
///
/// The refusal is stated as a fact about **the form** and nothing else — what the
/// caller does about it (a save refuses the write; the journal drops the entry and
/// reports it) is the caller's own sentence to add.
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
            return Err(format!("the host v1 text form cannot express {cmd:?}"));
        };
        out.push_str(&line);
        out.push('\n');
    }
    if grouped {
        out.push_str("group end\n");
    }
    Ok(())
}

/// Split journal lines into **entries**: a bare command is one line, a gesture runs
/// from its `group begin` to its `group end`. The journal is a sequence of complete
/// entries and recovery is per entry, so each is parsed on its own — one unspellable
/// line must not take the edits after it with it.
fn journal_entries(lines: &[&str]) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim() == "group begin" {
            // A gesture runs to its `group end`; an unterminated one (which
            // `apply_journal` truncates before calling) runs to the end of the tail, so
            // the parser reports it rather than this function guessing where it ended.
            let last = lines[i + 1..]
                .iter()
                .position(|l| l.trim() == "group end")
                .map_or(lines.len() - 1, |k| i + 1 + k);
            entries.push(lines[i..=last].join("\n"));
            i = last + 1;
        } else if line.trim().is_empty() {
            i += 1; // a blank line between entries is a crash artefact, not an entry
        } else {
            // A bare command is one entry — and so is a `group end` with no
            // `group begin`, as the unparseable entry it is.
            entries.push((*line).to_string());
            i += 1;
        }
    }
    entries
}

/// Whether two command lists are **the same commands** — compared by their *serialised
/// form*, not by the derived `PartialEq`.
///
/// `HostCommand` carries `f32`/`f64`, and IEEE-754 leaves one hole in `==`: a `NaN` is
/// not equal to itself. So the derived comparison answered "no, and no, and no" to a
/// value the log *does* carry — `NaN` spells as `NaN` and reads back as `NaN` — which is
/// a different answer from "the form lost it", and it never recovers. The serialised form
/// compares what the log says, which is the question the round trip is asking. Every
/// other difference the derived comparison caught is a difference in the text too: a
/// field `format_command` never writes, a path the parser read differently, a gesture
/// regrouped.
fn same_commands(parsed: &[HostCommand], expected: &[HostCommand]) -> bool {
    format!("{parsed:?}") == format!("{expected:?}")
}

/// Whether the rendered `text` of one history **entry** reads back as that entry: the
/// same round trip `save` checks over a whole script, for a single gesture.
/// `parse_script` must accept the line(s) *and* return the commands they came from — a
/// line the tokenizer mangles (a `play` path with whitespace) is not the same command,
/// and a journal that carries it cannot be reopened. The `Err` names **which** of the
/// two it was, because "the edit is not in the form" and "the form reads this edit back
/// as something else" are different faults with different fixes.
///
/// **`resolve` is the session directory whose relative `pool` path the text is read
/// back against**, and it is what makes one rendered line comparable to two callers: the
/// journal's `pool /abs/path` reads back as itself, and a save's `pool pool` reads back
/// as the absolute path it resolves to — the same `resolve_session_paths` `save` and
/// `load_session` run on a parsed script, so the two spellings of the same `pool` line
/// both compare equal to the entry that made it.
fn entry_round_trips(
    entry: &[HostCommand],
    text: &str,
    resolve: Option<&std::path::Path>,
) -> Result<(), String> {
    let mut parsed = parse_script(&format!("host v1\n{text}"))
        .map_err(|e| format!("the host v1 text form cannot read it back ({e})"))?;
    if let Some(dir) = resolve {
        resolve_session_paths(&mut parsed, dir);
    }
    let expected: Vec<HostCommand> = if entry.len() == 1 {
        entry.to_vec()
    } else {
        vec![HostCommand::Group {
            commands: entry.to_vec(),
        }]
    };
    if same_commands(&parsed, &expected) {
        Ok(())
    } else {
        Err(format!(
            "the host v1 text form reads it back as {parsed:?}, which is not the edit that was \
             made"
        ))
    }
}

/// Why one history **entry** cannot be carried by a durable record, if it cannot be —
/// the predicate the journal's write and the outstanding report are made of. `text` is
/// cleared and filled with the entry's lines **in the spelling asked for**, so the
/// caller can name the edit it is refusing or dropping without rendering it twice.
///
/// Two faults, in one place, because they clear differently and must not be *decided*
/// differently: the form has no line for one of the entry's commands, or the lines it
/// has do not read back as the entry that was made. `None` means the entry is
/// spellable in the spelling asked for — so that one write path can carry it.
///
/// **`session_dir` is the caller's spelling, and the two callers spell differently**:
/// the journal writes the paths the session *used* (`None`), while a save writes the
/// copy inside the session directory (`Some(dir)`, which is also what makes a saved
/// session movable). `Pool` is the **only** command whose line depends on it (the whole
/// of `pool_text`), and a pool path with a space is unspellable in the journal's
/// spelling and perfectly spellable in a save's, which spells it `pool pool`. A loss
/// may therefore only be reported once **both** spellings have been asked — see
/// [`outstanding_refusal`].
fn entry_fault(
    entry: &[HostCommand],
    session_dir: Option<&std::path::Path>,
    text: &mut String,
) -> Option<String> {
    text.clear();
    match write_entry(text, entry, session_dir) {
        // A command the form cannot express at all. A `save` refuses such a history by
        // name, so this is the rare case — but the journal is the one write path that
        // can meet an entry a save never saw (a session opened from a hand-edited
        // script), and it is reported rather than skipped in silence.
        Err(e) => return Some(e),
        Ok(()) if text.is_empty() => {
            return Some("it has no line in the host v1 text form".into());
        }
        Ok(()) => {}
    }
    entry_round_trips(entry, text, session_dir).err()
}

/// The report an entry the **autosave** dropped gets: **why** it could not be written
/// and **what** the loss means. One construction for the write and the re-derivation, so
/// the two can never describe the same edit differently.
///
/// The claim is a claim about **both** durable records — "it is in the live session and
/// nowhere else, and a save refuses the history as well" — so it may only be made where
/// that is true of both, which is what [`outstanding_refusal`] checks before calling
/// this. An entry the journal cannot spell but a save can (a `pool` line under a
/// directory whose path holds a space) is **not** reported: it is a line the baseline
/// carries, the pool is context rather than an edit (`rebase_pool`), and the old report
/// claimed a save-refusal for a save that had plainly succeeded.
fn refusal_report(why: &str, text: &str) -> String {
    let mut out = String::from(why);
    out.push_str(
        ", so this edit is not autosaved — it is in the live session and nowhere else, \
         and a save refuses the history as well",
    );
    if !text.trim().is_empty() {
        out.push_str(": ");
        out.push_str(text.trim());
    }
    out
}

/// The **outstanding refusal** in `history`: the most recent entry that is in **no**
/// durable record — neither the journal's spelling nor the save's — so a loss worth
/// reporting is still standing. `None` when every entry is spellable by at least one of
/// the two write paths.
///
/// The report is a **property of the history, not a latch** — a latch outlives its
/// subject. `undo`/`redo` rebuild the session from the history, so an edit that is no
/// longer in it is no longer lost, and a report naming it would be a false alarm: the
/// shell's status line would read "autosave failed" after every subsequent edit, over an
/// edit that is not in the session any more. Re-derived, the rule holds from both sides:
/// an entry that *is* still there and still in no record keeps its report (which is what
/// stops a later successful write from erasing it), and one that is gone takes it with
/// it.
///
/// **Both spellings are asked, and the reason is the false alarm it removes.** `Pool` is
/// the only line the session directory changes, so the two spellings differ for one
/// command: a `pool` path holding a space is unspellable in the journal and spellable in
/// a save, which spells it relative to the directory it lives in. Asking only the
/// journal's spelling made a **successful** `save "/…/My Songs/song.d"` manufacture a
/// fault claiming "a save refuses the history as well" over a `pool` line that save had
/// just written into the baseline — a report whose text was false, which is worse than no
/// report, because a user acts on it. The entry is named in the **save's** spelling,
/// because that is the spelling the claim is about.
fn outstanding_refusal(
    history: &[Vec<HostCommand>],
    dir: &std::path::Path,
) -> Option<JournalFault> {
    let mut text = String::new();
    for entry in history.iter().rev() {
        // A record the journal can hold is a record full stop: the edit is durable, so
        // there is no loss to report even if a save would spell it differently.
        if entry_fault(entry, None, &mut text).is_none() {
            continue;
        }
        // Only spellable-in-the-baseline is a standing loss (a `pool` line): the edit
        // is in `session.txt` the moment it is saved, and the pool is context.
        if let Some(why) = entry_fault(entry, Some(dir), &mut text) {
            return Some(JournalFault::Refused(refusal_report(&why, &text)));
        }
    }
    None
}

/// How a writer **names** one history entry in a message: the lines it would be written
/// as, or the command itself when the form has no line for it. The group markers are
/// dropped — a gesture is named by its members, not by its bracketing.
fn name_entry(entry: &[HostCommand], session_dir: Option<&std::path::Path>) -> String {
    let mut text = String::new();
    if write_entry(&mut text, entry, session_dir).is_err() {
        return format!("{entry:?}");
    }
    let text = text.trim();
    let text = text
        .strip_prefix("group begin\n")
        .and_then(|t| t.strip_suffix("\ngroup end"))
        .unwrap_or(text);
    text.replace('\n', "; ")
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
        // `take <take_id> <frames> <dropped> <channels> <at_frame> [source=<name>]` — the
        // pool sources are the `{take_id}.ch{k}` convention, and `at_frame` is the take's
        // origin rather than an edit time, so it is not `@`-suffixed. `source=` is
        // trailing and omitted when the take came from the default input.
        HostCommand::Take {
            take_id,
            frames,
            dropped,
            channels,
            at_frame,
            source,
        } => {
            let provenance = match source {
                Some(s) => format!(" source={s}"),
                None => String::new(),
            };
            format!("take {take_id} {frames} {dropped} {channels} {at_frame}{provenance}")
        }
        // `source add <name> kind=<kind> match=<matcher> channels=<n> clock=<role>` —
        // the matcher is one token, so the line is whitespace-round-trippable (a
        // matcher with a space has no spelling yet).
        HostCommand::SourceAdd {
            name,
            kind,
            matcher,
            channels,
            clock,
        } => format!(
            "source add {name} kind={kind} match={matcher} channels={channels} clock={}",
            clock.as_str()
        ),
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
        | HostCommand::Export { .. }
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
        // A 3-word line **clears** the label (an empty operand cannot be spelled), so a
        // cleared name round-trips instead of printing a line the parser rejects.
        Op::RenameClip { track, clip, name } => {
            if name.is_empty() {
                format!("rename_clip {track} {clip}")
            } else {
                format!("rename_clip {track} {clip} {name}")
            }
        }
        Op::SetMarker { at_frame, name } => format!("set_marker {at_frame} {name}"),
        Op::RemoveMarker { at_frame } => format!("remove_marker {at_frame}"),
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
            // The name is the *last* optional operand, so a named clip with no loop
            // still prints the loop slot (`0` reads as "no loop", exactly as the codec's
            // `loop` field means). A name is one token (`valid_name`), so the line
            // reparses as the same clip.
            if clip.loop_len.is_some() || clip.name.is_some() {
                line.push_str(&format!(" {}", clip.loop_len.unwrap_or(0)));
            }
            if let Some(name) = &clip.name {
                line.push_str(&format!(" {name}"));
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
                // `record <take> [source=<name>]` — the source is the **declared rig
                // name** the take should be captured from; omitted means the default
                // input, which is what every pre-binding script wrote.
                if words.len() != 2 && words.len() != 3 {
                    return Err(format!(
                        "line {at}: record takes 1 or 2 operand(s), got {}",
                        words.len() - 1
                    ));
                }
                let take_id = word(&words, 1, at)?;
                let source = match words.get(2) {
                    None => None,
                    Some(tok) if tok.starts_with("source=") => {
                        Some(keyed("source", tok, at)?.to_string())
                    }
                    Some(tok) => {
                        // A stray word is a typo, not a take id with a space: the second
                        // operand is the provenance modifier, and saying so beats
                        // "unexpected token".
                        return Err(format!(
                            "line {at}: record takes 1 or 2 operand(s) — the second is \
                             source=<name>, got '{tok}'"
                        ));
                    }
                };
                if take_id == "stop" {
                    if source.is_some() {
                        return Err(format!(
                            "line {at}: `record stop` takes no source — the source names what a \
                             take captures"
                        ));
                    }
                    commands.push(HostCommand::RecordStop);
                } else {
                    commands.push(HostCommand::Record {
                        take_id: take_id.to_string(),
                        source,
                    });
                }
            }
            "take" => {
                // `take <id> <frames> <dropped> <channels> <at_frame> [source=<name>]` —
                // the provenance modifier is optional, so every pre-binding session and
                // every existing test line parses unchanged.
                if words.len() != 6 && words.len() != 7 {
                    return Err(format!(
                        "line {at}: take takes 5 or 6 operand(s), got {}",
                        words.len() - 1
                    ));
                }
                let take_id = word(&words, 1, at)?.to_string();
                let frames = word(&words, 2, at)?
                    .parse::<u64>()
                    .map_err(|_| format!("line {at}: bad take length (want frames)"))?;
                let dropped = word(&words, 3, at)?
                    .parse::<u64>()
                    .map_err(|_| format!("line {at}: bad take dropped count (want frames)"))?;
                let channels = word(&words, 4, at)?
                    .parse::<usize>()
                    .map_err(|_| format!("line {at}: bad take channel count"))?;
                let at_frame = word(&words, 5, at)?
                    .parse::<u64>()
                    .map_err(|_| format!("line {at}: bad take origin (want a frame)"))?;
                let source = match words.get(6) {
                    None => None,
                    Some(tok) => {
                        let name = keyed("source", tok, at)?;
                        if !media::valid_name(name) {
                            return Err(format!(
                                "line {at}: '{name}' is not usable as a take's source name (one \
                                 token, no '#', not an @frame or snap= modifier)"
                            ));
                        }
                        Some(name.to_string())
                    }
                };
                commands.push(HostCommand::Take {
                    take_id,
                    frames,
                    dropped,
                    channels,
                    at_frame,
                    source,
                });
            }
            "source" => {
                let sub = word(&words, 1, at)?;
                if sub != "add" {
                    return Err(format!(
                        "line {at}: unknown source op '{sub}' (use `source add`)"
                    ));
                }
                // name(2) kind=(3) match=(4) channels=(5) clock=(6) — a fixed
                // shape, so an extra word (e.g. a matcher with a space) is a
                // typo, not something to ignore. The tokenizer is
                // whitespace-based, so a matcher is a single token; quoted
                // matchers await a tokenizer extension, not one grown here.
                exact(&words, 7, at, "source add")?;
                let name = word(&words, 2, at)?.to_string();
                let kind = keyed("kind", word(&words, 3, at)?, at)?;
                let matcher = keyed("match", word(&words, 4, at)?, at)?.to_string();
                let channels = keyed("channels", word(&words, 5, at)?, at)?
                    .parse::<usize>()
                    .map_err(|_| format!("line {at}: bad source channel count"))?;
                let clock = rig::ClockRole::parse(keyed("clock", word(&words, 6, at)?, at)?)
                    .map_err(|e| format!("line {at}: {e}"))?;
                commands.push(HostCommand::SourceAdd {
                    name,
                    kind: rig::source_kind(kind).map_err(|e| format!("line {at}: {e}"))?,
                    matcher,
                    channels,
                    clock,
                });
            }
            "bounce" => {
                exact(&words, 3, at, "bounce")?;
                let frames = word(&words, 1, at)?
                    .parse()
                    .map_err(|_| format!("line {at}: bad frames"))?;
                let path = PathBuf::from(word(&words, 2, at)?);
                commands.push(HostCommand::Bounce { frames, path });
            }
            "export" => {
                // `export <path> [f32|s16]` — f32 when the format is omitted.
                if words.len() != 2 && words.len() != 3 {
                    return Err(format!(
                        "line {at}: export takes 1 or 2 operand(s), got {}",
                        words.len() - 1
                    ));
                }
                let path = PathBuf::from(word(&words, 1, at)?);
                let format = match words.get(2) {
                    None => ExportFormat::F32,
                    Some(&"f32") => ExportFormat::F32,
                    Some(&"s16") => ExportFormat::S16,
                    Some(other) => {
                        return Err(format!(
                            "line {at}: unknown export format '{other}' (use f32 or s16)"
                        ));
                    }
                };
                commands.push(HostCommand::Export { path, format });
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
        "rename_clip" => {
            // `rename_clip <track> <clip> <name>` sets the label; the 3-word form
            // (`rename_clip <track> <clip>`) **clears** it, which is the only way the
            // empty name can be spelled in a whitespace-separated format.
            if words.len() != 3 && words.len() != 4 {
                return Err(format!(
                    "line {at}: arrange rename_clip takes 2 or 3 operand(s) (a name to set, or                      none to clear), got {}",
                    words.len() - 1
                ));
            }
            Ok(media::ArrangeOp::RenameClip {
                track: s(1)?,
                clip: s(2)?,
                name: if words.len() == 4 {
                    s(3)?
                } else {
                    String::new()
                },
            })
        }
        "set_marker" => {
            arity(3)?;
            Ok(media::ArrangeOp::SetMarker {
                at_frame: u(1)?,
                name: s(2)?,
            })
        }
        "remove_marker" => {
            arity(2)?;
            Ok(media::ArrangeOp::RemoveMarker { at_frame: u(1)? })
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
            // add_clip track c0 source src_start src_len at_frame fade_in fade_out gain
            //           [loop_len] [name]
            // operands (words[0]=op): track(1) id(2) source(3) src_start(4) src_len(5)
            //                       at_frame(6) fade_in(7) fade_out(8) gain(9) loop_len(10)
            //                       name(11)
            if !(10..=12).contains(&words.len()) {
                return Err(format!(
                    "line {at}: arrange add_clip expects 10 to 12 words (op + 9 operands + optional loop_len and name), got {}",
                    words.len()
                ));
            }
            let name = words.get(11).copied().unwrap_or("");
            if !name.is_empty() && !media::valid_name(name) {
                return Err(format!("line {at}: '{name}' is not usable as a clip name"));
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
                name: (!name.is_empty()).then(|| name.to_string()),
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

/// A `key=<value>` operand: the prefix is part of the line's grammar, so a
/// missing one is a parse refusal, not a positional guess.
fn keyed<'a>(key: &str, token: &'a str, at: usize) -> Result<&'a str, String> {
    token
        .strip_prefix(&format!("{key}="))
        .ok_or_else(|| format!("line {at}: expected {key}=<value>, got '{token}'"))
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
#[path = "tests/tests.rs"]
mod tests;

// ------------------------------------------------- clock-out wiring (slice B)

#[cfg(test)]
#[path = "tests/lib_clock_out_wiring.rs"]
mod clock_out_wiring;
