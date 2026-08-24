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
    "ch0.gain", "ch0.mute", "ch0.solo", "ch1.gain", "ch1.mute", "ch1.solo",
    "ch2.gain", "ch2.mute", "ch2.solo", "ch3.gain", "ch3.mute", "ch3.solo",
    "ch4.gain", "ch4.mute", "ch4.solo", "ch5.gain", "ch5.mute", "ch5.solo",
    "ch6.gain", "ch6.mute", "ch6.solo", "ch7.gain", "ch7.mute", "ch7.solo",
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
    /// Record the master into the pool (declared for the device path; the
    /// reference host without a device refuses with a clear error).
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
    /// the mixer's mounted channel count (validated at Play apply).
    mixer_channels: Option<usize>,
    /// media commands applied (the log covers engine commands only until
    /// P1.3; the host reports both — kimi review finding 9).
    media_commands: usize,
    /// the clip editor (lazy): owns the arrangement value + op handlers.
    editor: Option<media::ClipEditor>,
    /// resolves a pool source id (stem) to its `.wav` path, set by `set_pool`.
    pool_resolver: Option<media::PoolResolver>,
}

impl HostSession {
    fn new() -> Self {
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
            mixer_channels: None,
            media_commands: 0,
            editor: None,
            pool_resolver: None,
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

    /// Test scaffolding (a real host speaks `HostCommand`, not `&mut Engine` —
    /// the boundary the note draws against in-process shells; kimi nit 16).
    pub fn engine(&mut self) -> &mut Engine {
        &mut self.engine
    }

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
            HostCommand::Unmount { plugin, .. } => self.engine.unmount(plugin),
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
                let id = self.engine.graph.add_node(
                    NodeKind::Opaque(Box::new(node)),
                    vec![Port { name: "audio", direction: Direction::Out, kind: SignalKind::Audio }],
                );
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
                Err("recording the input requires a device — the reference host bounces the master instead (see Bounce)".into())
            }
            HostCommand::Arrange { op, .. } => {
                self.ensure_editor()?;
                if self.pool_resolver.is_none() {
                    return Err("arrange requires set_pool first (clips need pool-source paths)".into());
                }
                let editor = self.editor.as_mut().expect("ensured");
                editor.apply(&mut self.engine, op)?;
                self.media_commands += 1;
                Ok(())
            }
            HostCommand::Pool { dir } => {
                self.set_pool(dir.clone())?;
                self.media_commands += 1;
                Ok(())
            }
            HostCommand::Bounce { frames, path } => {
                self.wire_pending()?;
                let out = self.engine.render(*frames);
                let mut w = media::WavWriter::create(path, self.engine.clock.sample_rate, 1)?;
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

    /// Wire one `ArrangerNode` per track (in the timeline value) into the mixer,
    /// Render `frames` from the current position (wiring pending cords first).
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        self.wire_pending().expect("wiring is validated at Play apply");
        self.engine.render(frames)
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

    /// The clip editor's arrangement value (read-only snapshot). Empty when no
    /// arrangement has been built. The value is a pure reconstruction of the
    /// logged `Arrange` commands (byte-identically replayable).
    pub fn arrangement(&self) -> media::Timeline {
        self.editor
            .as_ref()
            .map(|e| e.snapshot())
            .unwrap_or_default()
    }

    pub fn underruns(&self) -> u64 {
        self.player_underruns
            .as_ref()
            .map(|u| u.load(Ordering::Relaxed))
            .unwrap_or(0)
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
        if let HostCommand::Mount { plugin: "mixer", params, .. } = cmd {
            // The mixer's channel count is the contract's own state (validated
            // at Play apply).
            let channels = params
                .iter()
                .find(|(name, _)| *name == "channels")
                .map(|(_, v)| *v as usize)
                .unwrap_or(4);
            session.mixer_channels = Some(channels);
        }
        if let Some(frame) = cmd.at_frame() {
            let now = session.engine.clock.frame();
            if frame > now {
                session.render((frame - now) as usize);
            }
        }
        session.apply(cmd)?;
    }
    Ok(session)
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
/// bounce 6000 /abs/out.wav
/// ```
pub fn parse_script(text: &str) -> Result<Vec<HostCommand>, String> {
    let mut lines = text.lines();
    let mut commands = Vec::new();

    // version line (the first non-empty, non-comment line)
    let version_line = loop {
        let raw = lines.next().ok_or("empty script (want 'host v1' first)")?;
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
        let at = lineno + 2; // account for the version line
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
                let plugin = in_list(HOST_PLUGINS, words[1], "plugin")?;
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
                let plugin = in_list(HOST_PLUGINS, words[1], "plugin")?;
                let param = in_list(HOST_PARAMS, words[2], "param")?;
                let value = words[3].parse::<f32>().map_err(|_| format!("line {at}: bad value"))?;
                commands.push(HostCommand::SetParam { plugin, param, value, at_frame });
            }
            "set_tempo" => {
                let bpm = words[1].parse().map_err(|_| format!("line {at}: bad bpm"))?;
                let beats = words[2].parse().map_err(|_| format!("line {at}: bad beats"))?;
                commands.push(HostCommand::SetTempo { bpm, beats_per_bar: beats, at_frame });
            }
            "unmount" => {
                let plugin = in_list(HOST_PLUGINS, words[1], "plugin")?;
                commands.push(HostCommand::Unmount { plugin, at_frame });
            }
            "play" => {
                let clip = ClipRef { path: PathBuf::from(words[1]), start: 0, len: 0 };
                let channel = channel_of(words[2], at)?;
                commands.push(HostCommand::Play { clip, channel, at_frame });
            }
            "splice" => {
                let at_frame = words[1].parse().map_err(|_| format!("line {at}: bad frame"))?;
                let clip = ClipRef { path: PathBuf::from(words[2]), start: 0, len: 0 };
                let crossfade = words[3].parse().map_err(|_| format!("line {at}: bad crossfade"))?;
                commands.push(HostCommand::Splice { at_frame, clip, crossfade });
            }
            "record" => {
                commands.push(HostCommand::Record { take_id: words[1].to_string() });
            }
            "bounce" => {
                let frames = words[1].parse().map_err(|_| format!("line {at}: bad frames"))?;
                let path = PathBuf::from(words[2]);
                commands.push(HostCommand::Bounce { frames, path });
            }
            other => return Err(format!("line {at}: unknown command '{other}'")),
        }
    }
    Ok(commands)
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
