//! The engine: assembles the four core pieces and drives the render loop.
//!
//! The public mutation API — `mount`, `patch`, `schedule_unmount`, `unmount`,
//! `set_tempo`, `replay_from`, `providers_of` — is the seed of the core plugin
//! contract (the future IPC command list, per the composition-seams note).
//!
//! Invariants:
//! - every mutation is **logged at call time with its absolute frame**, then
//!   scheduled; the render loop applies it when the clock reaches that frame
//!   (sample-accurate lifecycle; a refused mutation is never logged);
//! - rendering is a pure function of the log: `same log ⇒ byte-identical bounce`
//!   (determinism is tested, including mid-session changes and patches);
//! - the render loop never allocates on contract-abiding paths (enforced by a
//!   counting-allocator test); the only allocation is the misuse-only `parked`
//!   push when an arrangement op reaches the render stack;
//! - **nothing logged is ever silently dropped** — an arrangement op that
//!   reaches the render stack is parked for the control side (`flush_scheduled`
//!   drains it), never discarded; the live run and a replay of the same log
//!   therefore cannot diverge on skipped work (see the control→render handoff
//!   decision note, 2026-08-27).

use std::collections::{HashMap, HashSet};

use crate::clock::{Clock, Scheduler};
use crate::graph::{BLOCK, Graph, NodeId, Port, RenderBlock, RenderMode, SignalKind};
use crate::log::{Event, SessionLog};
use crate::plugins::{Disposer, DisposerCtx, ParamDef, Plugin, PluginApi};
use crate::value::Value;

/// One-shot events the scheduling queue delivers at an exact absolute frame.
#[derive(Debug, Clone, PartialEq)]
pub enum SchedEvent {
    Mount {
        plugin: &'static str,
        params: Vec<(&'static str, f32)>,
    },
    Unmount {
        plugin: &'static str,
    },
    Patch {
        from_plugin: &'static str,
        from_port: &'static str,
        to_plugin: &'static str,
        to_port: &'static str,
    },
    SetTempo {
        bpm: f64,
        beats_per_bar: u32,
        /// The frame the change belongs to. Carried so the **segment is stamped at its
        /// own frame even when the event is delivered later** — the render loop normally
        /// delivers it exactly at `at_frame`, but a seek that places the clock *past* a
        /// scheduled change (a warm-up run-in) delivers it late, and stamping it at the
        /// clock's current position would move the change's effect (the beat count, the
        /// ruler) without moving the audio.
        at_frame: u64,
    },
    SetParam {
        plugin: &'static str,
        param: &'static str,
        value: f32,
    },
    Arrangement {
        op: &'static str,
        fields: Vec<(&'static str, Value)>,
    },
}

/// Builds a plugin from a flat parameter list (the `Event::Mount` payload).
pub type PluginFactory = fn(&[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String>;

/// A registered plugin-message handler: interprets an op's fields (the media
/// side decodes them into a profile-level op and applies it to its value). The
/// engine logs and schedules the op; this is what actually applies it.
///
/// **Contract:** the handler is invoked on the control side (`flush_scheduled`),
/// never on the render stack. It must be **atomic** — validate before mutating,
/// so an `Err` means "nothing changed" — and a **deterministic function of the
/// op stream** (plus its own logged state), so replay reproduces the same value.
/// If it reconciles readers, referenced pool files must be immutable by id.
pub type OpHandler = Box<dyn FnMut(&[(&'static str, Value)]) -> Result<(), String>>;

/// Whether a render drains stateful nodes' buffered tails after the timeline
/// ends, or hard-cuts at the last frame. The policy belongs to the bounce /
/// `OfflineProcess` event; recording it in the session log rides the
/// media-command logging step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainPolicy {
    /// Stop at the timeline's last frame — the pre-drain behaviour.
    HardCut,
    /// Render drain blocks until no node holds a tail (bounded).
    Tails,
}

/// What a drain phase emitted after the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DrainOutcome {
    /// Frames of buffered tail emitted after the timeline.
    pub tail_frames: u64,
    /// True when the bound was hit with output still pending — an explicit,
    /// fail-loud outcome, never a silent truncation.
    pub capped: bool,
}

/// The default bound on a drain phase: ~60 s at 48 kHz. A feedback structure can
/// ring indefinitely, so the cap makes drain terminating (and hitting it is
/// reported, not hidden).
pub const MAX_DRAIN_FRAMES: usize = 48_000 * 60;

/// The slowest tempo [`Engine::set_tempo`] accepts: 1e-3 bpm, where a quarter
/// note lasts 60 000 s — 16.7 hours. Nothing musical is that slow (a whole note
/// over a minute is ~50× above the floor), so the floor excludes only nonsense.
///
/// The lower end of the range is not a matter of taste but of what the tempo map
/// can express. [`crate::clock::TempoMap::frame_at`] covers the final
/// open-ended segment with `u64::MAX` frames at `bpm`, so a segment spans
/// `u64::MAX × bpm / 60 / sample_rate` beats — at 48 kHz that falls under one
/// 24-PPQN tick (1/24 beat) below ~6.5e-15 bpm, and every tick after the first
/// then resolves to the segment's start frame instead of its own. The floor sits
/// some 1.5e11× above that point, so a `u64` frame range is never the binding
/// constraint: `u64::MAX` frames at 48 kHz is ~1.2e7 years, and the timeline
/// itself is never the thing that runs out.
///
/// Refusing below the floor is the honest answer — a tick the map cannot place is
/// a tick MIDI clock has nothing to say about — and the render path's own bounds
/// ([`crate::clock::TempoMap::frame_at`] saturating, the clock-out node's capped
/// walk) hold for a log that carries one anyway.
pub const MIN_TEMPO_BPM: f64 = 1e-3;

/// **Which plugin names are live** — mounted, or queued to mount — as far as some
/// document says.
///
/// This is the one-instance-per-name bookkeeping, and *who owns it* is the whole
/// question. A **live command** answers it from the engine's scheduling state
/// (`disposers` + `scheduled`), because a render applies what it queues and so the
/// state is current. A **document walk** — [`Self::enter_walk`]: the recorded state
/// of a session, re-issued onto an engine — cannot: nothing renders during the walk,
/// so the apply queue never drains and a name whose unmount the document schedules
/// would still read as mounted. The lifecycle belongs to the document while it is
/// being applied, exactly as it belongs to the engine between renders.
///
/// A walk is seeded from what the engine already holds ([`Self::of`]), so it never
/// loses the target's own state: a walk that mounts a name the engine already has
/// applied is refused like any other second instance.
struct Live {
    names: HashSet<&'static str>,
}

impl Live {
    /// The names this engine holds live **right now**: applied, or queued to apply.
    fn of(engine: &Engine) -> Self {
        Live {
            names: engine
                .disposers
                .keys()
                .chain(engine.scheduled.iter())
                .copied()
                .collect(),
        }
    }

    /// Whether one instance of `name` is already in force.
    fn holds(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// A mount takes the name. The caller has already refused a second instance
    /// ([`Engine::validate_mount`]), so this never has to answer.
    fn mount(&mut self, name: &'static str) {
        self.names.insert(name);
    }

    /// An unmount gives the name back — the document says the plugin goes away, so a
    /// later mount of the same name in the same document is a **re-mount**, not a
    /// second instance. This mirrors `apply_unmount`, which is where the live path
    /// releases the name.
    fn unmount(&mut self, name: &str) {
        self.names.remove(name);
    }
}

/// The assembled minimal core.
pub struct Engine {
    pub clock: Clock,
    pub scheduler: Scheduler<SchedEvent>,
    pub graph: Graph,
    pub ctx: crate::ctx::Context,
    pub log: SessionLog,
    factories: HashMap<&'static str, PluginFactory>,
    /// registered port surfaces (validated against the plugin at apply).
    port_table: HashMap<&'static str, &'static [Port]>,
    /// The **mounted** port surface per plugin, captured from the instance at apply
    /// time. It can be narrower than the catalog — the mixer mounts some of the
    /// channels it declares — and validation prefers it, so a patch to an unmounted
    /// channel fails loudly instead of being accepted and ignored downstream.
    mounted_ports: HashMap<&'static str, Vec<Port>>,
    /// registered runtime parameter surfaces (validated by `set_param`).
    params_table: HashMap<&'static str, &'static [ParamDef]>,
    /// The **mounted** parameter surface per plugin; see [`Self::mounted_ports`].
    mounted_params: HashMap<&'static str, Vec<ParamDef>>,
    /// Mount params of plugins whose mount is queued but not yet applied. A `Patch` or
    /// `SetParam` arriving in that window has no instance to ask, so the surface is
    /// derived by asking the **factory** with these params — the same call
    /// `validate_mount` already makes.
    scheduled_params: HashMap<&'static str, Vec<(&'static str, f32)>>,
    /// plugin name → its mounted primary node.
    node_of: HashMap<&'static str, NodeId>,
    disposers: HashMap<&'static str, Disposer>,
    /// plugins whose mount is queued but not yet applied.
    scheduled: HashSet<&'static str>,
    /// The **document walks** in force, innermost last (see [`Self::enter_walk`]).
    /// Non-empty exactly while a caller re-issues a recorded document — a session
    /// log, the host's command history — onto this engine.
    walks: Vec<Live>,
    /// registered plugin-message handlers, keyed by op (closed-core dispatch).
    op_handlers: HashMap<&'static str, OpHandler>,
    /// Arrangement ops that reached the render stack (a host that rendered
    /// without flushing) — parked for the control side; `flush_scheduled`
    /// applies them FIFO before the due queue. Nothing logged is dropped.
    parked: Vec<SchedEvent>,
}

impl Engine {
    /// Core services the engine itself satisfies — the clock is core, never a
    /// ctx-provided service (a plugin may not own time, and a clone would go
    /// stale). Plugins access it read-only via the block's tempo map.
    fn core_services() -> &'static [&'static str] {
        &["clock"]
    }

    pub fn new(sample_rate: u32, bpm: f64, beats_per_bar: u32) -> Self {
        let clock = Clock::new(sample_rate, bpm, beats_per_bar);
        Engine {
            clock,
            scheduler: Scheduler::new(),
            graph: Graph::new(),
            ctx: crate::ctx::Context::new(),
            log: SessionLog::new(),
            factories: HashMap::new(),
            port_table: HashMap::new(),
            params_table: HashMap::new(),
            mounted_ports: HashMap::new(),
            mounted_params: HashMap::new(),
            scheduled_params: HashMap::new(),
            node_of: HashMap::new(),
            disposers: HashMap::new(),
            scheduled: HashSet::new(),
            walks: Vec::new(),
            op_handlers: HashMap::new(),
            parked: Vec::new(),
        }
    }

    /// Register a plugin factory and its declared port + parameter surfaces
    /// (the patch bay's registry; `providers_of` reads it from mounted nodes,
    /// and `set_param` validates names against the declared params).
    pub fn register_factory(
        &mut self,
        name: &'static str,
        factory: PluginFactory,
        ports: &'static [Port],
        params: &'static [ParamDef],
    ) {
        self.factories.insert(name, factory);
        self.port_table.insert(name, ports);
        self.params_table.insert(name, params);
    }

    /// Enter a **document walk**: from here until [`Self::leave_walk`], the
    /// one-instance-per-name rule is answered by the walk's own `Live` set rather
    /// than by the engine's scheduling state.
    ///
    /// A walk is for re-issuing a *recorded* document onto this engine — a session
    /// log ([`Self::replay_from`]) or a host's command history. Nothing renders
    /// during one, so the apply queue never drains, and the engine's own view would
    /// refuse a `mount … unmount … mount` sequence the live path accepts (it releases
    /// the name when the unmount *applies*). The document is the authority while it
    /// is being applied.
    ///
    /// Walks stack: the new one starts from the walk it is nested in, or from what
    /// the engine holds, so no name in force is ever forgotten. **Pair every
    /// `enter_walk` with a `leave_walk`** — a `?` between them would leave the
    /// engine answering from a half-applied document.
    pub fn enter_walk(&mut self) {
        let live = match self.walks.last() {
            Some(outer) => Live {
                names: outer.names.clone(),
            },
            None => Live::of(self),
        };
        self.walks.push(live);
    }

    /// Leave a document walk, restoring the walk it was nested in (or none).
    pub fn leave_walk(&mut self) {
        self.walks.pop();
    }

    /// Mount a plugin at the current frame: validated synchronously (fail-loud),
    /// logged with its frame, then applied by the render loop at that frame.
    pub fn mount(
        &mut self,
        name: &'static str,
        params: &[(&'static str, f32)],
    ) -> Result<(), String> {
        self.validate_mount(name, params)?;
        if let Some(live) = self.walks.last_mut() {
            // The document takes the name; its own unmount gives it back.
            live.mount(name);
        }
        let at_frame = self.clock.frame();
        self.log.push(Event::Mount {
            plugin: name,
            params: params.to_vec(),
            at_frame,
        });
        self.scheduler.schedule(
            at_frame,
            SchedEvent::Mount {
                plugin: name,
                params: params.to_vec(),
            },
        );
        self.scheduled.insert(name);
        self.scheduled_params.insert(name, params.to_vec());
        Ok(())
    }

    /// Synchronous, side-effect-free validation: known plugin, declared services
    /// all provided (core or ctx), one instance per name.
    fn validate_mount(
        &self,
        name: &'static str,
        params: &[(&'static str, f32)],
    ) -> Result<(), String> {
        if self.holds_instance_of(name) {
            return Err(Self::already_mounted(name));
        }
        self.validate_mount_declaration(name, params)
    }

    /// "Is one instance of `name` already in force?" — the one-instance-per-name
    /// question, asked of whoever owns the lifecycle: a **document walk** answers
    /// from the document ([`Self::enter_walk`]), the live path from the engine's
    /// scheduling state (applied, or queued to apply).
    fn holds_instance_of(&self, name: &str) -> bool {
        match self.walks.last() {
            Some(live) => live.holds(name),
            None => self.disposers.contains_key(name) || self.scheduled.contains(name),
        }
    }

    /// The lifecycle-free half of [`Self::validate_mount`]. [`Self::replay_from`]
    /// calls this directly, because there "one instance per name" is a question
    /// about the **log's** lifecycle, not about this engine's scheduling state.
    fn validate_mount_declaration(
        &self,
        name: &'static str,
        params: &[(&'static str, f32)],
    ) -> Result<(), String> {
        // Mount params get a finiteness check (GLM-5.3 #9): `set_param` has one,
        // but a `mount ... NaN` would otherwise reach the plugin's apply silently.
        for (pname, v) in params {
            if !v.is_finite() {
                return Err(format!("mount param '{pname}' must be finite, got {v}"));
            }
        }
        let factory = *self
            .factories
            .get(name)
            .ok_or_else(|| format!("unknown plugin '{name}'"))?;
        let plugin = factory(params)?;
        for key in plugin.inject() {
            if !Self::core_services().contains(key) && !self.ctx.has(key) {
                return Err(format!("plugin '{name}' requires missing service '{key}'"));
            }
        }
        debug_assert_eq!(
            plugin.ports(),
            self.port_table[name],
            "registered ports must match the plugin"
        );
        Ok(())
    }

    /// The refusal the "one instance per name" rule speaks in, from the live
    /// engine, from a document walk and from a replay alike.
    fn already_mounted(name: &str) -> String {
        format!("plugin '{name}' is already mounted (one instance per name in spike A.5)")
    }

    /// The refusal a **replay onto a non-fresh engine** speaks in. `replay_from`'s
    /// contract is a fresh engine, and the reason is this: replaying a log that
    /// mounts a name the target already holds would apply a *second* instance —
    /// `node_of`/`disposers` overwrite the first, whose node stays in the graph and
    /// whose disposer is dropped with it. A loud `Err` beats a silent leak.
    fn replay_needs_fresh_engine(name: &str) -> String {
        format!(
            "replay: plugin '{name}' is already mounted on this engine — replay_from needs a \
             fresh engine (the replayed log owns every mount; a second instance would overwrite \
             the applied node and drop its disposer)"
        )
    }

    fn apply_mount(
        &mut self,
        name: &'static str,
        params: &[(&'static str, f32)],
    ) -> Result<(), String> {
        let factory = *self
            .factories
            .get(name)
            .ok_or_else(|| format!("unknown plugin '{name}'"))?;
        let mut plugin = factory(params)?;
        let id = plugin.id();
        // Capture the **mounted** surface before applying: it is the instance's answer
        // to "what did you actually mount", and every later patch and parameter
        // validation reads it in preference to the registered catalog.
        self.mounted_ports.insert(name, plugin.mounted_ports());
        self.mounted_params.insert(name, plugin.mounted_params());
        let (node, disposer) = {
            let Engine {
                ctx,
                scheduler,
                graph,
                clock,
                ..
            } = self;
            let mut api = PluginApi {
                ctx,
                scheduler,
                graph,
                clock,
            };
            plugin.apply(&mut api)?
        };
        self.scheduled.remove(name);
        self.scheduled_params.remove(name);
        self.node_of.insert(id, node);
        self.disposers.insert(id, disposer);
        Ok(())
    }

    /// Patch two plugins' ports at the current frame. Validated synchronously
    /// against the *declared* port surfaces (kinds and directions), logged, and
    /// applied by the render loop once both plugins are mounted.
    pub fn patch(
        &mut self,
        from: (&'static str, &'static str),
        to: (&'static str, &'static str),
    ) -> Result<(), String> {
        self.validate_patch(from, to)?;
        let at_frame = self.clock.frame();
        self.log.push(Event::Patch {
            from_plugin: from.0,
            from_port: from.1,
            to_plugin: to.0,
            to_port: to.1,
            at_frame,
        });
        self.scheduler.schedule(
            at_frame,
            SchedEvent::Patch {
                from_plugin: from.0,
                from_port: from.1,
                to_plugin: to.0,
                to_port: to.1,
            },
        );
        Ok(())
    }

    fn validate_patch(
        &self,
        from: (&'static str, &'static str),
        to: (&'static str, &'static str),
    ) -> Result<(), String> {
        let (fp, tp) = (from.0, to.0);
        if !self.factories.contains_key(fp) {
            return Err(format!("unknown plugin '{fp}'"));
        }
        if !self.factories.contains_key(tp) {
            return Err(format!("unknown plugin '{tp}'"));
        }
        // Both endpoints must be scheduled or mounted, or the patch would have
        // nothing to apply to at its frame (fail-loud, never logged).
        if !(self.scheduled.contains(fp) || self.node_of.contains_key(fp)) {
            return Err(format!("plugin '{fp}' is neither scheduled nor mounted"));
        }
        if !(self.scheduled.contains(tp) || self.node_of.contains_key(tp)) {
            return Err(format!("plugin '{tp}' is neither scheduled nor mounted"));
        }
        // The surface the plugin currently offers — the applied instance's, or the one
        // a queued mount's factory derives from its params. The catalog is nominal.
        let from_ports = self.ports_of(fp);
        let to_ports = self.ports_of(tp);
        let from_port = from_ports
            .iter()
            .find(|p| p.name == from.1)
            .ok_or_else(|| format!("no port '{}' on plugin '{fp}'", from.1))?;
        let to_port = to_ports
            .iter()
            .find(|p| p.name == to.1)
            .ok_or_else(|| format!("no port '{}' on plugin '{tp}'", to.1))?;
        if from_port.direction != crate::graph::Direction::Out
            || to_port.direction != crate::graph::Direction::In
        {
            return Err(format!(
                "patch: '{}' must be an Out port and '{}' an In port",
                from.1, to.1
            ));
        }
        if from_port.kind != to_port.kind {
            return Err(format!(
                "patch: signal kind mismatch — '{}' is {:?}, '{}' is {:?}",
                from.1, from_port.kind, to.1, to_port.kind
            ));
        }
        if from_port.kind == crate::graph::SignalKind::Audio
            && from_port.channels() != to_port.channels()
        {
            // A cord carries one channel count: the destination port's buffer is
            // `channels * frames`, so a mismatch would silently misread the interleave
            // (a stereo bus into a mono port would arrive as garbage). Fail loud, and
            // never log it — validation happens before the patch enters the log.
            return Err(format!(
                "patch: channel mismatch — '{}' is {} channel(s), '{}' is {}",
                from.1,
                from_port.channels(),
                to.1,
                to_port.channels()
            ));
        }
        Ok(())
    }

    /// Apply a patch at its scheduled frame. Never panics: if an endpoint is
    /// not mounted (a log-order error) or the graph refuses the cord (forward
    /// order, single-driver control), the intent stays in the log and the
    /// refusal is asserted in debug — no audio-thread crash, no silent
    /// divergence (replay reproduces the same refused state).
    fn apply_patch(
        &mut self,
        from: (&'static str, &'static str),
        to: (&'static str, &'static str),
    ) {
        let Some(&from_node) = self.node_of.get(from.0) else {
            debug_assert!(
                false,
                "patch endpoint '{}' not mounted at apply (log-order error)",
                from.0
            );
            return;
        };
        let Some(&to_node) = self.node_of.get(to.0) else {
            debug_assert!(
                false,
                "patch endpoint '{}' not mounted at apply (log-order error)",
                to.0
            );
            return;
        };
        if let Err(e) = self.graph.connect(from_node, from.1, to_node, to.1) {
            debug_assert!(false, "scheduled patch refused at apply: {e}");
        }
    }

    /// Schedule an unmount at an absolute frame — the scheduling queue driving
    /// lifecycle, sample-accurately.
    pub fn schedule_unmount(&mut self, name: &'static str, at_frame: u64) {
        if let Some(live) = self.walks.last_mut() {
            // In a **document walk** the name is free from here: the document says
            // the plugin goes away, so a later mount in the same document is a
            // re-mount. On the live path the release still happens at apply
            // (`apply_unmount`), which is why the same-tick `mount → unmount →
            // mount` window stays refused there.
            live.unmount(name);
        }
        self.log.push(Event::ScheduleUnmount {
            plugin: name,
            at_frame,
        });
        self.scheduler
            .schedule(at_frame, SchedEvent::Unmount { plugin: name });
    }

    /// Unmount at the current frame. Fail-loud: an unknown plugin is refused
    /// and never logged (kimi review finding 10 — the log must not record
    /// things that never happened).
    pub fn unmount(&mut self, name: &'static str) -> Result<(), String> {
        if !self.node_of.contains_key(name) && !self.scheduled.contains(name) {
            return Err(format!("plugin '{name}' is neither scheduled nor mounted"));
        }
        let at_frame = self.clock.frame();
        self.schedule_unmount(name, at_frame);
        Ok(())
    }

    /// The ports a plugin offers **now**, in preference order: the applied instance's
    /// mounted surface, then the surface a queued mount will have — the factory is
    /// asked with the params it will be mounted with, the same call `validate_mount`
    /// already makes — then the registered catalog as the nominal fallback.
    ///
    /// The middle rung is what lets a `mount` followed by a `patch` in one script work
    /// for a plugin whose surface depends on its mount params (the mixer's channels).
    fn ports_of(&self, name: &'static str) -> Vec<Port> {
        if let Some(mounted) = self.mounted_ports.get(name) {
            return mounted.clone();
        }
        if let Some(params) = self.scheduled_params.get(name)
            && let Some(factory) = self.factories.get(name)
            && let Ok(plugin) = factory(params)
        {
            return plugin.mounted_ports();
        }
        self.port_table.get(name).copied().unwrap_or(&[]).to_vec()
    }

    /// The parameters a plugin offers now; see [`Self::ports_of`].
    fn params_of(&self, name: &'static str) -> Vec<ParamDef> {
        if let Some(mounted) = self.mounted_params.get(name) {
            return mounted.clone();
        }
        if let Some(params) = self.scheduled_params.get(name)
            && let Some(factory) = self.factories.get(name)
            && let Ok(plugin) = factory(params)
        {
            return plugin.mounted_params();
        }
        self.params_table.get(name).copied().unwrap_or(&[]).to_vec()
    }

    /// Apply an unmount: run the disposer (reversible effects). Idempotent.
    fn apply_unmount(&mut self, name: &'static str) {
        self.scheduled.remove(name);
        self.mounted_ports.remove(name);
        self.mounted_params.remove(name);
        if let Some(disposer) = self.disposers.remove(name) {
            self.node_of.remove(name);
            let Engine {
                ctx,
                scheduler,
                graph,
                ..
            } = self;
            disposer(&mut DisposerCtx {
                ctx,
                scheduler,
                graph,
            });
        }
    }

    /// A tempo change at the current frame (sample-accurate: applied by the
    /// render loop at that frame). Fail-loud: a non-finite, non-positive, or
    /// sub-floor tempo is refused and never logged (kimi review finding 10;
    /// the floor is [`MIN_TEMPO_BPM`]).
    pub fn set_tempo(&mut self, bpm: f64, beats_per_bar: u32) -> Result<(), String> {
        if !bpm.is_finite() || bpm <= 0.0 {
            return Err(format!("tempo must be finite and positive, got {bpm}"));
        }
        if bpm < MIN_TEMPO_BPM {
            return Err(format!(
                "tempo must be at least {MIN_TEMPO_BPM} bpm \
                 (a quarter note taking 16.7 hours), got {bpm}"
            ));
        }
        let at_frame = self.clock.frame();
        self.log.push(Event::SetTempo {
            bpm,
            beats_per_bar,
            at_frame,
        });
        self.scheduler.schedule(
            at_frame,
            SchedEvent::SetTempo {
                bpm,
                beats_per_bar,
                at_frame,
            },
        );
        Ok(())
    }

    /// Set a discrete parameter on a plugin at the current frame: validated
    /// fail-loud (plugin registered and mounted-or-scheduled, name declared in
    /// the plugin's parameter surface, value finite and in range), logged, and
    /// applied by the render loop at its frame — sample-accurate and
    /// replayable (Phase 1: the generic control path for the mixer's
    /// gain/mute/solo/fader). A refused call is never logged; automation
    /// curves are a later event type.
    pub fn set_param(
        &mut self,
        plugin: &'static str,
        param: &'static str,
        value: f32,
    ) -> Result<(), String> {
        if !self.factories.contains_key(plugin) {
            return Err(format!("unknown plugin '{plugin}'"));
        }
        if !(self.node_of.contains_key(plugin) || self.scheduled.contains(plugin)) {
            return Err(format!(
                "plugin '{plugin}' is neither scheduled nor mounted"
            ));
        }
        // The parameters the plugin currently offers: the applied instance's, the
        // queued mount's derived surface, or the nominal catalog.
        let (min, max) = {
            let declared = self.params_of(plugin);
            let Some(def) = declared.iter().find(|d| d.name == param) else {
                return Err(format!("plugin '{plugin}' has no parameter '{param}'"));
            };
            (def.min, def.max)
        };
        if !value.is_finite() {
            return Err(format!("parameter '{param}' must be finite, got {value}"));
        }
        if value < min || value > max {
            return Err(format!(
                "parameter '{param}' out of range [{min}, {max}]: {value}"
            ));
        }
        let at_frame = self.clock.frame();
        self.log.push(Event::SetParam {
            plugin,
            param,
            value,
            at_frame,
        });
        self.scheduler.schedule(
            at_frame,
            SchedEvent::SetParam {
                plugin,
                param,
                value,
            },
        );
        Ok(())
    }

    /// Register a plugin-message handler for an op name. Called by a plugin on
    /// mount (its `apply`); the engine logs `arrange` ops and dispatches them
    /// here. Fail-loud when the op is already claimed.
    pub fn register_op_handler(
        &mut self,
        op: &'static str,
        handler: OpHandler,
    ) -> Result<(), String> {
        if self.op_handlers.contains_key(op) {
            return Err(format!("op '{op}' already has a handler"));
        }
        self.op_handlers.insert(op, handler);
        Ok(())
    }

    /// Unregister an op handler (a plugin's disposer on unmount). Idempotent.
    pub fn unregister_op_handler(&mut self, op: &'static str) {
        self.op_handlers.remove(op);
    }

    /// Validate a plugin-message op: a handler registered and every `F32` field
    /// finite (fail-loud; a refused op is never logged).
    fn validate_op(
        &self,
        op: &'static str,
        fields: &[(&'static str, Value)],
    ) -> Result<(), String> {
        if !self.op_handlers.contains_key(op) {
            return Err(format!("no handler registered for op '{op}'"));
        }
        for (name, v) in fields {
            if let Value::F32(x) = v
                && !x.is_finite()
            {
                return Err(format!("field '{name}' must be finite, got {x}"));
            }
        }
        Ok(())
    }

    /// Push an `Arrangement` event to the log at the current frame; returns the
    /// frame. The caller decides whether to also schedule it (value-level ops are
    /// logged but not scheduled — the live value is updated eagerly by the
    /// clip editor; `replay_from` re-schedules the logged op to reconstruct it).
    fn log_arrangement(&mut self, op: &'static str, fields: Vec<(&'static str, Value)>) -> u64 {
        let at_frame = self.clock.frame();
        self.log.push(Event::Arrangement {
            op,
            fields,
            at_frame,
        });
        at_frame
    }

    /// A plugin message at the current frame: validated (a handler must be
    /// registered for the op, and every `F32` field finite — fail-loud), logged
    /// with its frame, and applied **on the control side** by `flush_scheduled`
    /// (a media handler reconciles readers — threads/file I/O — so it must not
    /// run on the render stack; see `apply_event`). A refused op is never logged.
    /// Note: a handler is registered when the plugin's mount is *applied*, so
    /// call `flush_scheduled()` between mounting the plugin and the first
    /// `arrange` of its ops (or the op is refused as unregistered).
    pub fn arrange(
        &mut self,
        op: &'static str,
        fields: Vec<(&'static str, Value)>,
    ) -> Result<(), String> {
        self.validate_op(op, &fields)?;
        let at_frame = self.log_arrangement(op, fields.clone());
        self.scheduler
            .schedule(at_frame, SchedEvent::Arrangement { op, fields });
        Ok(())
    }

    /// Like [`arrange`], but only **logs** the op — it is not scheduled. Used for
    /// value-level arrangement ops whose live value is applied eagerly by the
    /// clip editor (so they never touch the scheduler and cannot flush the mixer
    /// early); `replay_from` re-schedules them to reconstruct the value.
    pub fn arrange_logged(
        &mut self,
        op: &'static str,
        fields: Vec<(&'static str, Value)>,
    ) -> Result<(), String> {
        self.validate_op(op, &fields)?;
        self.log_arrangement(op, fields);
        Ok(())
    }

    /// Replay a log onto this engine. Must be a *fresh* engine — enforced, and
    /// loudly refused otherwise: every event is scheduled at its recorded frame and
    /// applied by the render loop, so nothing is applied eagerly and the timeline
    /// reproduces exactly. A log's mounts are the *only* mounts the engine may have
    /// after a replay; a name the target already holds would be applied twice, and
    /// the second `apply_mount` would overwrite the first instance's `node_of` entry
    /// and drop its disposer, leaving its node in the graph forever.
    ///
    /// The one-instance-per-name rule itself is a fact about the **log's** lifecycle,
    /// so it is tracked here in a `Live` set rather than read off `self.scheduled`:
    /// a replay applies nothing (nothing renders yet), so a name whose unmount the
    /// log schedules stays in `scheduled` for the whole walk — and the
    /// `mount … unmount … mount` shape the live engine produces (it releases the
    /// name when the unmount *applies*) would be refused as a second instance. The
    /// engine writes such logs; it must be able to read them.
    pub fn replay_from(&mut self, log: &SessionLog) -> Result<(), String> {
        // The fresh-engine precondition, spoken by the engine rather than assumed: an
        // applied plugin *or* a queued one (nothing has rendered, so both are
        // pre-existing state) means the target is not a clean slate.
        let applied = self.disposers.keys().next().copied();
        let queued = self.scheduled.iter().next().copied();
        if let Some(name) = applied.or(queued) {
            return Err(Self::replay_needs_fresh_engine(name));
        }
        // A walk, entered and left around the loop below: the log's lifecycle owns
        // "is this name live?" for the duration, exactly as the host's command
        // history does when a session is rebuilt. Nothing may return between the
        // two calls, or the engine would answer from a half-walked document.
        self.enter_walk();
        let result = self.replay_events(log);
        self.leave_walk();
        result
    }

    /// The walk of [`Self::replay_from`]: every event scheduled at its recorded
    /// frame, the lifecycle owned by the log, the engine's own log repopulated.
    fn replay_events(&mut self, log: &SessionLog) -> Result<(), String> {
        for event in log.events() {
            match event {
                Event::Mount {
                    plugin,
                    params,
                    at_frame,
                } => {
                    if self.holds_instance_of(plugin) {
                        return Err(Self::already_mounted(plugin));
                    }
                    self.validate_mount_declaration(plugin, params)?;
                    self.scheduler.schedule(
                        *at_frame,
                        SchedEvent::Mount {
                            plugin,
                            params: params.clone(),
                        },
                    );
                    // Both halves of the apply queue's view, as the live `mount`
                    // records them: a patch or a parameter change arriving before
                    // this mount applies must be validated against the surface the
                    // queued instance *will* have (`ports_of`/`params_of` ask
                    // `scheduled_params`), not against the nominal catalog.
                    self.scheduled.insert(plugin);
                    self.scheduled_params.insert(plugin, params.clone());
                    if let Some(live) = self.walks.last_mut() {
                        live.mount(plugin);
                    }
                }
                Event::ScheduleUnmount { plugin, at_frame } => {
                    self.scheduler
                        .schedule(*at_frame, SchedEvent::Unmount { plugin });
                    if let Some(live) = self.walks.last_mut() {
                        live.unmount(plugin);
                    }
                }
                Event::Patch {
                    from_plugin,
                    from_port,
                    to_plugin,
                    to_port,
                    at_frame,
                } => {
                    self.validate_patch((*from_plugin, *from_port), (*to_plugin, *to_port))?;
                    self.scheduler.schedule(
                        *at_frame,
                        SchedEvent::Patch {
                            from_plugin,
                            from_port,
                            to_plugin,
                            to_port,
                        },
                    );
                }
                Event::SetTempo {
                    bpm,
                    beats_per_bar,
                    at_frame,
                } => {
                    self.scheduler.schedule(
                        *at_frame,
                        SchedEvent::SetTempo {
                            bpm: *bpm,
                            beats_per_bar: *beats_per_bar,
                            at_frame: *at_frame,
                        },
                    );
                }
                Event::SetParam {
                    plugin,
                    param,
                    value,
                    at_frame,
                } => {
                    self.scheduler.schedule(
                        *at_frame,
                        SchedEvent::SetParam {
                            plugin,
                            param,
                            value: *value,
                        },
                    );
                }
                Event::Arrangement {
                    op,
                    fields,
                    at_frame,
                } => {
                    // Fail loud: replaying into an engine without the op's handler
                    // would otherwise schedule an op that is silently dropped at
                    // apply (release: `debug_assert` only), yielding a session
                    // with no media/arrangement state and no error.
                    if !self.op_handlers.contains_key(op) {
                        return Err(format!("replay: no handler registered for op '{op}'"));
                    }
                    self.scheduler.schedule(
                        *at_frame,
                        SchedEvent::Arrangement {
                            op,
                            fields: fields.clone(),
                        },
                    );
                }
            }
            // Repopulate the engine's own log with each replayed event, so a
            // *continued* session (load a log, keep editing, save) has a log that
            // still describes the audio actually produced — model-visible means
            // logged, even across a replay. A Mount/Patch validation failure
            // returns `Err` above before this runs, so a refused event is never
            // logged (the replayed log was invalid).
            self.log.push(event.clone());
        }
        Ok(())
    }

    fn apply_event(&mut self, event: SchedEvent) {
        match event {
            SchedEvent::Unmount { plugin } => self.apply_unmount(plugin),
            SchedEvent::Mount { plugin, params } => {
                // `apply_mount` must run in BOTH builds. The previous
                // `debug_assert!(self.apply_mount(...).is_ok(), ...)` form is a
                // *no-op in release* (debug_assert! never evaluates its
                // argument), so no scheduled mount was ever applied there and
                // the engine rendered silence on every mounted path — a
                // release-only bug. Now always applied. Like the sibling arms,
                // an `apply` failure is a log-order error (the mount was
                // validated against a registered plugin at schedule time) that
                // is debug-asserted and skipped in release, where replay
                // reproduces the same state. `apply` is the one step
                // `validate_mount` can't dry-run (it calls `inject()`, not
                // `apply`), so "a scheduled mount must apply" is enforced by
                // tests, not the type system.
                if let Err(e) = self.apply_mount(plugin, &params) {
                    debug_assert!(false, "scheduled mount must apply (log was validated): {e}");
                }
            }
            SchedEvent::Patch {
                from_plugin,
                from_port,
                to_plugin,
                to_port,
            } => self.apply_patch((from_plugin, from_port), (to_plugin, to_port)),
            SchedEvent::SetTempo {
                bpm,
                beats_per_bar,
                at_frame,
            } => {
                // Stamp the segment at the frame the change belongs to, not at wherever the
                // clock happens to be when the event is delivered (the two differ only when
                // something places the clock past a scheduled change — a warm-up seek).
                self.clock.tempo_map.push(at_frame, bpm, beats_per_bar);
            }
            SchedEvent::SetParam {
                plugin,
                param,
                value,
            } => {
                // Never panics: an unmounted plugin at apply is a log-order
                // error (the set_param was validated against a mount) —
                // debug-asserted, skipped in release, replay reproduces it.
                match self.node_of.get(plugin) {
                    Some(&node) => self.graph.set_param(node, param, value),
                    None => {
                        debug_assert!(false, "set_param endpoint '{plugin}' not mounted at apply")
                    }
                }
            }
            SchedEvent::Arrangement { op, fields } => {
                // Never panics: a missing handler or a refused op at apply is a
                // log-order error (the op was validated when logged) — debug-
                // asserted, skipped in release, replay reproduces the same state.
                match self.op_handlers.get_mut(op) {
                    Some(handler) => {
                        if let Err(e) = handler(&fields) {
                            debug_assert!(false, "arrangement op '{op}' refused at apply: {e}");
                        }
                    }
                    None => debug_assert!(
                        false,
                        "arrangement op '{op}' has no registered handler at apply"
                    ),
                }
            }
        }
    }

    /// The patch-bay registry view: every mounted `Out` port of the given kind —
    /// the data source for the "dropdown of available inputs".
    pub fn providers_of(&self, kind: SignalKind) -> Vec<(NodeId, &'static str)> {
        self.graph
            .nodes()
            .iter()
            .filter_map(|node| {
                node.ports
                    .iter()
                    .find(|p| p.direction == crate::graph::Direction::Out && p.kind == kind)
                    .map(|p| (node.id, p.name))
            })
            .collect()
    }

    /// The primary node a mounted plugin owns (its `apply` registered it). A
    /// host resolves a plugin by name rather than assuming the master-bus node
    /// is the mixer — the bus only points at the mixer while the mixer is mounted.
    pub fn node_of(&self, plugin: &'static str) -> Option<NodeId> {
        self.node_of.get(plugin).copied()
    }

    /// Plugin names providing an `Out` port of the given kind — the dropdown
    /// wants names, not node ids.
    pub fn provider_names_of(&self, kind: SignalKind) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .node_of
            .iter()
            .filter(|(_, node)| {
                self.graph
                    .nodes()
                    .iter()
                    .find(|n| n.id == **node)
                    .is_some_and(|n| {
                        n.ports
                            .iter()
                            .any(|p| p.direction == crate::graph::Direction::Out && p.kind == kind)
                    })
            })
            .map(|(name, _)| *name)
            .collect();
        names.sort();
        names
    }

    /// Flush scheduling events due at the current frame so the master's channel
    /// count is stable for the whole render call. Without this, a scheduled
    /// mixer mount (frame 0) would only take effect mid-`render`, so the frame
    /// count would be silently halved by the stereo master.
    fn channels_for_render(&mut self) -> usize {
        self.flush_scheduled();
        self.graph.out_channels().max(1)
    }

    /// Whether applying `event` mid-render could change the master bus **width** —
    /// the channel count the output buffer of this call was sized for.
    ///
    /// Two structural cases, no plugin names: **unmounting the current bus owner**
    /// (the bus passes to whoever precedes it, which this cannot know) and **mounting
    /// a plugin whose declared audio Out is a different width from the bus** (it may
    /// claim the bus, or be handed it by the graph's "first audio provider owns it"
    /// fallback). The original matched `"mixer"` by name and silently missed the
    /// second bus plugin — found by the slice's gate.
    ///
    /// Parking keeps the width constant across a call, so `render` stays a pure
    /// function of (log, call boundaries). The cost is that such a mount/unmount is
    /// deferred by at most one call boundary; both are control-side events.
    fn changes_master_width(&self, event: &SchedEvent) -> bool {
        let plugin = match event {
            SchedEvent::Mount { plugin, .. } | SchedEvent::Unmount { plugin } => *plugin,
            _ => return false,
        };
        if matches!(event, SchedEvent::Unmount { .. })
            && self
                .node_of
                .get(plugin)
                .is_some_and(|id| self.graph.out_node == Some(*id))
        {
            return true;
        }
        let declared = self.port_table.get(plugin).and_then(|ports| {
            ports
                .iter()
                .find(|p| {
                    p.direction == crate::graph::Direction::Out
                        && p.kind == crate::graph::SignalKind::Audio
                })
                .map(|p| p.channels())
        });
        match (event, declared) {
            (SchedEvent::Mount { .. }, Some(ch)) => ch != self.graph.out_channels().max(1),
            // A plugin with no audio Out cannot become the bus.
            _ => false,
        }
    }

    /// **Place the clock at `frame` without rendering 0→frame.** The caller must then
    /// render a warm-up long enough for every stateful node to reach the state a full
    /// replay would have ([`Clock::seek_to`] explains the contract); a node whose reads
    /// are a pure function of the block frame needs nothing. The host's
    /// `SEEK_WARMUP_FRAMES` is that run-in, and its tests prove the equality.
    pub fn seek(&mut self, frame: u64) {
        self.clock.seek_to(frame);
    }

    /// Render `frames` samples (frames * channels, interleaved) into a fresh
    /// buffer. The channel count is the graph's master out (1 mono, 2 stereo).
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let ch = self.channels_for_render();
        let mut out = vec![0.0f32; frames * ch];
        self.render_into(&mut out);
        out
    }

    /// Render `frames` of timeline, then drain buffered tails per `policy` — the
    /// offline bounce shape. With [`DrainPolicy::Tails`] the returned buffer is
    /// timeline ++ drain; a graph with no tail returns the timeline unchanged
    /// (`DrainOutcome::default()`).
    pub fn render_with_drain(
        &mut self,
        frames: usize,
        policy: DrainPolicy,
        max_tail_frames: usize,
    ) -> (Vec<f32>, DrainOutcome) {
        let mut out = self.render(frames);
        match policy {
            DrainPolicy::HardCut => (out, DrainOutcome::default()),
            DrainPolicy::Tails => {
                let (tail, outcome) = self.drain(max_tail_frames);
                out.extend_from_slice(&tail);
                (out, outcome)
            }
        }
    }

    /// Render `frames` and drain buffered tails — **latency-aligned**: the render
    /// runs `frames + flush_frames()` long and the leading `flush_frames()` frames
    /// (the graph's transit) are dropped, so the returned audio starts at the
    /// timeline's frame 0 even when a node carries processing latency (the master
    /// bus's lookahead). Without the trim an offline render would begin with that
    /// much silence and end that much short; the drain still supplies everything in
    /// flight, so the result is the piece (plus its tails), not the piece plus
    /// latency.
    ///
    /// The latency is *processing*, not content: the ballistics still see the first
    /// frames of the piece, exactly as live playback from frame 0 would. The graph
    /// must be fully wired before this is called (the host's `render_with_drain`
    /// wires the arrangement first), because the transit is a property of the
    /// wiring — `flush_frames` computes it from the nodes and cords, not from a
    /// cached value, so it is exact before the first block.
    pub fn render_with_drain_aligned(
        &mut self,
        frames: usize,
        policy: DrainPolicy,
        max_tail_frames: usize,
    ) -> (Vec<f32>, DrainOutcome) {
        let latency = self.graph.flush_frames() as usize;
        let channels = self.graph.out_channels().max(1);
        if latency == 0 {
            return self.render_with_drain(frames, policy, max_tail_frames);
        }
        let (mut out, outcome) =
            self.render_with_drain(frames.saturating_add(latency), policy, max_tail_frames);
        // Drop the head: `latency` frames * channels samples. A render shorter than
        // the latency is all head (nothing of the piece was produced).
        let head = (latency * channels).min(out.len());
        out.drain(..head);
        (out, outcome)
    }

    /// Drain stateful nodes' buffered tails: render extra [`RenderMode::Drain`]
    /// blocks after the timeline until every node reports no tail, then flush the
    /// PDC transit still in flight (so a compensated path is not cut short), or
    /// `max_frames` is reached (bounded by [`MAX_DRAIN_FRAMES`]). Advances the
    /// clock. Deterministic — a pure function of graph state (no wall clock, no
    /// randomness) — so a drained render is byte-identical.
    pub fn drain(&mut self, max_frames: usize) -> (Vec<f32>, DrainOutcome) {
        // Bound the drain: a feedback structure can ring indefinitely.
        let max_frames = max_frames.min(MAX_DRAIN_FRAMES);
        let ch = self.graph.out_channels().max(1);
        let mut tail: Vec<f32> = Vec::new();

        // The PDC transit still in flight, independent of tails. When a tail is
        // live the flush is *re-armed* at the tail→false transition, so a tail
        // longer than the transit is still followed by its flush; when no tail is
        // live it arms immediately, so in-flight samples are never dropped.
        let mut flush: Option<usize> = None;
        let mut capped = false;
        loop {
            let has_tail = self.graph.has_tail();
            if !has_tail && flush.is_none() {
                flush = Some(self.graph.flush_frames() as usize);
            }
            if !has_tail && flush == Some(0) {
                break;
            }
            let frames_done = tail.len() / ch;
            if frames_done >= max_frames {
                capped = true;
                break;
            }
            let frames = BLOCK.min(max_frames - frames_done);
            let f0 = self.clock.frame();
            let mut block = vec![0.0f32; frames * ch];
            {
                let Engine { clock, graph, .. } = self;
                let rb = RenderBlock {
                    frame: f0,
                    sample_rate: clock.sample_rate,
                    tempo: &clock.tempo_map,
                    mode: RenderMode::Drain,
                };
                graph.render_drain(&mut block, rb);
            }
            self.clock.advance(frames as u64);
            tail.extend_from_slice(&block);
            if let Some(f) = flush.as_mut() {
                *f = f.saturating_sub(frames);
            }
        }
        let tail_frames = (tail.len() / ch) as u64;
        (
            tail,
            DrainOutcome {
                tail_frames,
                capped,
            },
        )
    }

    /// Render into a caller-provided buffer, in fixed-size blocks.
    pub fn render_into(&mut self, out: &mut [f32]) {
        let ch = self.channels_for_render();
        let block_samples = BLOCK * ch;
        for chunk in out.chunks_mut(block_samples) {
            self.render_block(chunk);
        }
    }

    /// Apply every scheduled event due at the current frame **without
    /// rendering audio** — the control→render handoff's current mechanism.
    /// Parked arrangement ops (they reached the render stack because a host
    /// rendered without flushing) apply FIRST, in scheduler order, then the due
    /// queue. Both phases run here, on the control side, never on the render
    /// stack (the reference host materializes scheduled mounts before wiring
    /// cords; kimi review finding 5: no discarded block).
    pub fn flush_scheduled(&mut self) {
        for event in std::mem::take(&mut self.parked) {
            self.apply_event(event);
        }
        while let Some(frame) = self.scheduler.peek_frame() {
            if frame > self.clock.frame() {
                break;
            }
            let event = self.scheduler.pop().expect("peeked");
            self.apply_event(event);
        }
    }

    /// Render one block, interleaving the scheduling queue: events are applied
    /// at their exact absolute frame by splitting the block around them.
    /// `out` holds `channels * frames` interleaved samples (one BLOCK of frames
    /// per call from `render_into`).
    fn render_block(&mut self, out: &mut [f32]) {
        let ch = self.graph.out_channels().max(1);
        let frames = out.len() / ch;
        let f1 = self.clock.frame() + frames as u64;
        let mut pos = self.clock.frame();
        let mut written = 0usize;
        loop {
            let next = self
                .scheduler
                .peek_frame()
                .filter(|f| *f < f1)
                .unwrap_or(f1);
            if next > pos {
                let flen = (next - pos) as usize;
                let slen = flen * ch;
                self.render_chunk(&mut out[written..written + slen], pos);
                written += slen;
                pos = next;
            }
            if next == f1 {
                break;
            }
            loop {
                let frame = self.scheduler.peek_frame();
                match frame {
                    Some(f) if f <= next => {
                        let event = self.scheduler.pop().expect("peeked");
                        if matches!(&event, SchedEvent::Arrangement { .. }) {
                            // Arrangement ops apply on the CONTROL side
                            // (flush_scheduled), never on the render stack: a
                            // media handler reconciles readers (threads, file
                            // I/O), which must not run on the audio thread.
                            // A host that reaches one here failed to flush
                            // before rendering — PARK the op (never drop it):
                            // it stays pending for the next flush_scheduled,
                            // so nothing logged is ever lost and live/replay
                            // cannot diverge (previously this path dropped
                            // the op in release while replay would still
                            // apply it). The debug assert keeps the contract
                            // violation loud in development.
                            self.parked.push(event);
                            debug_assert!(
                                false,
                                "an arrangement op reached the render stack; flush_scheduled before rendering"
                            );
                            continue;
                        }
                        if self.changes_master_width(&event) {
                            // A master-*width* change (the mixer mounts/unmounts
                            // and thus the bus owner's channel count changes)
                            // cannot apply mid-call: the output buffer was sized
                            // for the width at the call's start, so a mid-call
                            // change would silently split the frame count
                            // (replay would advance the clock wrong). PARK it —
                            // it takes effect at the next flush (render-call
                            // boundary), keeping the width constant per call and
                            // render a pure function of (log, call boundaries).
                            self.parked.push(event);
                            continue;
                        }
                        self.apply_event(event);
                    }
                    _ => break,
                }
            }
        }
        debug_assert_eq!(written, out.len());
    }

    /// Render one contiguous chunk. The render path allocates nothing. The
    /// clock advances by FRAMES (not samples) — `out` is interleaved.
    fn render_chunk(&mut self, out: &mut [f32], f0: u64) {
        let Engine { clock, graph, .. } = self;
        let block = RenderBlock {
            frame: f0,
            sample_rate: clock.sample_rate,
            tempo: &clock.tempo_map,
            mode: RenderMode::Timeline,
        };
        graph.render(out, block);
        clock.advance((out.len() as u64) / graph.out_channels().max(1) as u64);
    }
}
