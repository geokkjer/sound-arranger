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
//!   decision note, 2026-08-27);
//! - **a failed apply changes nothing, and is reported in every build** — a
//!   plugin's `apply` is fallible and the one step [`Engine::validate_mount`]
//!   cannot dry-run, so [`Engine::apply_mount`] is a transaction: on `Err` the
//!   engine's own bookkeeping is never written, the graph is put back, and the
//!   refusal is **recorded** ([`Engine::apply_faults`], bounded) rather than
//!   asserted, so a host can see it and mark the session degraded;
//! - **a logged patch is a patch the graph will make** — [`Engine::validate_patch`]
//!   answers the graph's forward-order rule (the source's node must precede the
//!   destination's) from the graph itself, or from the mount queue's order while an
//!   endpoint is still queued, so a cord `Graph::connect` would refuse is refused
//!   at call time and never logged; a cord that *is* refused at apply is recorded
//!   as a fault, so neither build reports a backward cord by asserting and neither
//!   renders a channel nothing feeds in silence.

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

/// The default bound on [`Engine::apply_faults`]: how many apply-time refusals
/// the engine keeps before it counts the rest.
///
/// A bound rather than a silent cap, like [`MAX_DRAIN_FRAMES`] and
/// [`DrainOutcome::capped`]: the refusals past the bound are *counted* in
/// [`Engine::apply_faults_dropped`], so a session that refuses a thousand mounts
/// shows a thousand refusals without holding a thousand strings. 64 is far more
/// than a session should ever accumulate — one is already a fault — and small
/// enough that keeping them costs nothing.
pub const MAX_APPLY_FAULTS: usize = 64;

/// A scheduled mutation the engine could not apply: the log says this plugin mounts
/// (or that this cord is patched) at this frame, and the plugin's `apply` — or the
/// graph — refused.
///
/// **A fault, not a diagnostic.** Two things that ought to agree — the log and the
/// plugin or the graph — do not, so the engine records it in *every* build (the
/// previous report was a `debug_assert!`, which release compiles away) and the
/// session is degraded. The log event is **not** withdrawn: the log is the document,
/// so a refusal is reported beside it rather than erased from it, and a replay of
/// the same log refuses at the same frame and records the same fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyFault {
    /// The plugin the log asked to mount, or — for a refused cord — the cord's
    /// source plugin, the one a user has to move; the destination is named in
    /// [`Self::reason`].
    pub plugin: &'static str,
    /// The frame the **log** stamped the event with — not the frame the refusal
    /// was noticed at, which a late flush or a warm-up seek can move.
    pub at_frame: u64,
    /// The plugin's own refusal, or the graph's for a cord, verbatim — behind the
    /// mutation the log spells out, so the message names what to change.
    pub reason: String,
}

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

/// What one **document** has said about a plugin's name — the one-instance-per-name
/// bookkeeping, and *who owns it* is the whole question.
///
/// A **live command** asks the engine's scheduling state (`disposers` + `scheduled`),
/// because a render applies what it queues and so that state is current. A
/// **document walk** — [`Engine::enter_walk`], the recorded state of a session
/// re-issued onto an engine — cannot: nothing renders during the walk, so the apply
/// queue never drains and a name whose unmount the document schedules would still
/// read as mounted. The lifecycle belongs to the document while it is being applied,
/// exactly as it belongs to the engine between renders.
///
/// **A walk records the names the document has spoken for, and nothing else.** A name
/// it has not mentioned is still the engine's business
/// ([`Engine::holds_instance_of`]), so the walk is a view of the document rather than
/// a snapshot of the target taken at entry — a snapshot goes stale the moment the
/// target's own state moves, and a stale view is how a walk loses state it was
/// supposed to protect. Two consequences, both load-bearing:
///
/// - a document that unmounts a name the target holds **replaces** that instance.
///   That is the point: the document says the plugin goes away and comes back, and
///   the frame rule below is what guarantees the teardown applies first.
/// - a document that mounts a name the target holds, without unmounting it first, is
///   refused like any other second instance — the target's own state answers for a
///   name the document has not spoken for.
///
/// The frame each event carries is the frame the scheduler will apply it at, and a
/// document's lifecycle for one name must be in **frame** order: `Mount p@a,
/// Unmount p@b, Mount p@c` is only re-mountable if `a ≤ b ≤ c`. Frame-inverted
/// otherwise (the scheduler sorts by frame, so the second mount would apply *before*
/// the unmount: two `apply_mount`s for one name, the first node orphaned and its
/// disposer dropped unrun). [`Engine::frame_inverted`] is that refusal — loud, at the
/// walk, so replay, load and rebuild all inherit it.
struct Live {
    spoken: HashMap<&'static str, Instance>,
}

/// One name's document-side lifecycle: in force, and the frame of the last thing the
/// document said about it.
#[derive(Clone)]
struct Instance {
    /// Is the name in force? An unmount sets this false, a mount true.
    live: bool,
    /// The frame of this document's most recent lifecycle event for the name.
    at_frame: u64,
}

impl Live {
    /// The **document's** answer for `name`, or `None` when the document has not
    /// spoken for it — in which case the name is the engine's business, not the
    /// document's (see [`Engine::holds_instance_of`]).
    fn in_force(&self, name: &str) -> Option<bool> {
        self.spoken.get(name).map(|instance| instance.live)
    }

    /// A mount takes the name at `at_frame`. The caller has already refused a second
    /// instance ([`Engine::validate_mount`]), so the only thing left to answer is
    /// whether the document is going *backwards* in time.
    fn mount(&mut self, name: &'static str, at_frame: u64) -> Result<(), String> {
        self.record(name, at_frame, true)
    }

    /// An unmount gives the name back — the document says the plugin goes away, so a
    /// later mount of the same name in the same document is a **re-mount**, not a
    /// second instance. This mirrors `apply_unmount`, which is where the live path
    /// releases the name. It records even for a name the document never mounted: the
    /// *target* may hold that name, and the document is the one saying it goes away.
    fn unmount(&mut self, name: &'static str, at_frame: u64) -> Result<(), String> {
        self.record(name, at_frame, false)
    }

    /// The one write path: refuse a document that goes back in time, else record.
    fn record(&mut self, name: &'static str, at_frame: u64, live: bool) -> Result<(), String> {
        if let Some(prev) = self.spoken.get(name)
            && prev.at_frame > at_frame
        {
            return Err(Engine::frame_inverted(name, prev.at_frame, at_frame));
        }
        self.spoken.insert(name, Instance { live, at_frame });
        Ok(())
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
    /// plugin name → the sequence number its live mount was **scheduled** with,
    /// one per `mount` call (a re-mount takes a fresh, higher one). A queued mount
    /// has no node to ask about its place in the graph, and `validate_patch` needs
    /// one to answer the forward-order question before the log takes the patch —
    /// see [`Self::graph_rank`].
    mount_seq: HashMap<&'static str, usize>,
    /// The next sequence number [`Self::mount_seq`] hands out.
    next_mount_seq: usize,
    /// The **document walks** in force, innermost last (see [`Self::enter_walk`]).
    /// Non-empty exactly while a caller re-issues a recorded document — a session
    /// log, a host's command history, a session script — onto this engine.
    walks: Vec<Live>,
    /// registered plugin-message handlers, keyed by op (closed-core dispatch).
    op_handlers: HashMap<&'static str, OpHandler>,
    /// Arrangement ops that reached the render stack (a host that rendered
    /// without flushing) — parked for the control side; `flush_scheduled`
    /// applies them FIFO before the due queue. Nothing logged is dropped. The
    /// frame each one was scheduled at rides with it, because a parked mount
    /// applies at the *next* flush and its fault must still name the frame the log
    /// stamped, not the frame the flush happened on.
    parked: Vec<(u64, SchedEvent)>,
    /// Scheduled mounts a plugin's `apply` refused, oldest first (see
    /// [`Self::apply_faults`]). Bounded by [`MAX_APPLY_FAULTS`]; the rest are
    /// counted in `apply_faults_dropped`. Never cleared: a fault is a standing
    /// fact about the session, not work to be done later (the opposite of
    /// `parked`), and it is written from the apply path — the misuse path, which
    /// is allowed to allocate.
    apply_faults: Vec<ApplyFault>,
    /// How many refusals the bound in [`MAX_APPLY_FAULTS`] could not hold.
    apply_faults_dropped: usize,
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
            mount_seq: HashMap::new(),
            next_mount_seq: 0,
            walks: Vec::new(),
            op_handlers: HashMap::new(),
            parked: Vec::new(),
            apply_faults: Vec::new(),
            apply_faults_dropped: 0,
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
    /// one-instance-per-name rule for every name **the document has spoken for** is
    /// answered by the document's own lifecycle rather than by the engine's
    /// scheduling state.
    ///
    /// A walk is for re-issuing a *recorded* document onto this engine — a session
    /// log ([`Self::replay_from`]), a host's command history, a session script. Nothing
    /// renders during one, so the apply queue never drains, and the engine's own view
    /// would refuse a `mount … unmount … mount` sequence the live path accepts (it
    /// releases the name when the unmount *applies*). The document is the authority
    /// over the names it speaks for, exactly as the engine is between renders.
    ///
    /// **What a walk guarantees** (all of it enforced, all of it here):
    ///
    /// - every mount, unmount and re-mount of a name the document issued is
    ///   accounted for, so `mount … unmount … mount` is a re-mount;
    /// - a document that mounts a name the engine already holds, without unmounting
    ///   it first, is refused — the walk does not speak for a name the document never
    ///   mentioned, so the engine's own state answers and a second instance cannot be
    ///   applied over a live one;
    /// - a document whose lifecycle frames for one name are not in order is refused
    ///   with [`Self::frame_inverted`], so a walk can never leave an orphaned node or
    ///   a dropped disposer behind.
    ///
    /// **What a caller must guarantee.** A walk is not a sandbox: the engine cannot
    /// tell *which* caller issued a command inside one, so entering a walk is the
    /// assertion that **everything issued until [`Self::leave_walk`] is one document**.
    /// Two ways to break that, both the caller's to avoid:
    ///
    /// - issuing state of your own while the walk is in force. It joins the
    ///   document's lifecycle; if it contradicts what the document says, the frame
    ///   rule refuses the whole walk — which is the safe outcome, but the message
    ///   names a document the caller did not write.
    /// - **placing the clock backwards** ([`Self::seek`]) inside a walk, so a later
    ///   event is stamped before an earlier one. The walk refuses the document that
    ///   results; the caller that caused it is the one to hear about it.
    ///
    /// Walks stack, and a nested walk starts from the document state it is nested in.
    /// **Pair every `enter_walk` with a `leave_walk`**, and put no `?` between them: a
    /// path that returns early leaves the engine answering from a half-applied
    /// document. (There is no guard type for this — a guard would have to borrow the
    /// engine, which is the very thing being mutated inside the walk — so it is a
    /// discipline, and every in-tree caller scopes it deliberately.)
    pub fn enter_walk(&mut self) {
        let live = match self.walks.last() {
            Some(outer) => Live {
                spoken: outer.spoken.clone(),
            },
            None => Live {
                spoken: HashMap::new(),
            },
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
        let at_frame = self.clock.frame();
        if let Some(live) = self.walks.last_mut() {
            // The document takes the name at the frame the scheduler will apply it
            // at; its own unmount gives it back. The frame order is checked here, so
            // a document that goes back in time is refused rather than walked.
            live.mount(name, at_frame)?;
        }
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
        // The place this mount will take in the graph's order, recorded with the
        // rest of the reservation: a patch that arrives before it applies asks
        // `graph_rank`, and the queue is frame-ordered and FIFO within a frame, so
        // schedule order is apply order.
        self.take_mount_seq(name);
        Ok(())
    }

    /// Give a queued mount its place in the graph's order — one write path for the
    /// live [`Self::mount`] and for a replayed document, so a replayed cord cannot
    /// be checked for forward order against a different order than a live one.
    /// A re-mount takes a **fresh** number: the name is appended again, behind
    /// whatever has applied since.
    fn take_mount_seq(&mut self, name: &'static str) {
        let seq = self.next_mount_seq;
        self.next_mount_seq += 1;
        self.mount_seq.insert(name, seq);
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
    /// question, asked of whoever owns the lifecycle. In a **document walk** the
    /// document owns the names it has spoken for (an unmount releases the name, so a
    /// later mount is a re-mount); for every other name the live path's own view
    /// answers — applied, or queued to apply. A walk never answers for a name the
    /// document has not mentioned, which is what keeps a walk from *losing* the
    /// target's state as well as from contradicting it.
    fn holds_instance_of(&self, name: &str) -> bool {
        if let Some(live) = self.walks.last()
            && let Some(in_force) = live.in_force(name)
        {
            return in_force;
        }
        self.disposers.contains_key(name) || self.scheduled.contains(name)
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

    /// The refusal a **frame-inverted document** speaks in, from every walk alike.
    ///
    /// The apply queue is frame-ordered, the log is not: `Mount p@a, Unmount p@b,
    /// Mount p@c` with `c ≤ b` or `b < a` delivers the second mount *before* the
    /// unmount that frees the name, so `apply_mount` runs twice for one name — the
    /// first instance's node stays in the graph and its disposer is dropped with its
    /// entry. The walk is the one place that sees the whole document, so it refuses
    /// here rather than repairing the log behind the caller's back: a loud `Err`
    /// names the document, where a self-healing apply would silently render
    /// something else than the log says.
    fn frame_inverted(name: &str, previous: u64, next: u64) -> String {
        format!(
            "plugin '{name}' goes back to frame {next} after frame {previous} — a document's \
             mounts and unmounts must be in frame order (the scheduler applies in frame order, so \
             an inverted triple mounts '{name}' twice and orphans the first instance)"
        )
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

    /// Apply a mount: build the instance, let it register, and **commit** — or,
    /// on a refusal, leave the engine exactly as it was.
    ///
    /// `apply` is fallible and this is the one step [`Self::validate_mount`]
    /// cannot dry-run (it calls `inject()`, never `apply`), so a mount that the
    /// log accepted can still be refused here. The euclidean plugin's `apply` is
    /// the first that can: its `steps` bound is re-checked at apply because its
    /// fields are public and an instance can be hand-assembled without the
    /// factory's door. When that happens the engine must not be left holding a
    /// name it never mounted — `scheduled` kept, the name refused as a second
    /// instance for the rest of the session, and `replay_from` refusing the log.
    ///
    /// So it is a **transaction**. Nothing the mount *adds* is written until the
    /// apply has succeeded: the mounted surfaces are captured first (they are the
    /// instance's answer to "what did you actually mount", which a refused
    /// instance has no answer to), and the node, its disposer and those surfaces
    /// are committed together below. On `Err` the engine keeps only what it had —
    /// the reservation `mount` took is released, so the name reads as free again
    /// rather than wedged for the rest of the session — and
    /// [`Self::undo_a_refused_apply`] puts the **graph** back too, since an
    /// `apply` that added a node and *then* refused would otherwise orphan it with
    /// no disposer able to remove it (the euclidean's refusal precedes its
    /// `add_node`, so this is the belt to that suspenders).
    ///
    /// A *successful* apply whose name is already mounted still overwrites the
    /// first instance's `node_of`/`disposers` — a log that says so is refused by
    /// the walk ([`Self::frame_inverted`]) before it can be applied.
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
        // Captured, **not** recorded: an instance that refuses has mounted nothing,
        // so its surface must not answer a patch or a parameter change.
        let mounted_ports = plugin.mounted_ports();
        let mounted_params = plugin.mounted_params();
        // The graph as it was, for the same reason. `NodeId`s come from a
        // monotonic counter, so "what this apply added" is exactly "the ids at or
        // above the watermark" — wherever `insert_before` put them.
        let watermark = self.graph.next_id();
        let bus = self.graph.out_node;
        let applied = {
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
            plugin.apply(&mut api)
        };
        let (node, disposer) = match applied {
            Ok(mounted) => mounted,
            Err(refusal) => {
                // The reservation goes back with the graph. This is the wedge: a
                // name left in `scheduled` reads as mounted to every later
                // `validate_mount`, so the session could never mount it again and
                // `replay_from` refused the log that carries it.
                self.scheduled.remove(name);
                self.scheduled_params.remove(name);
                self.mount_seq.remove(name);
                self.undo_a_refused_apply(watermark, bus);
                return Err(refusal);
            }
        };
        // Commit. Every write the mount makes is below this line.
        self.mounted_ports.insert(name, mounted_ports);
        self.mounted_params.insert(name, mounted_params);
        self.scheduled.remove(name);
        self.scheduled_params.remove(name);
        self.node_of.insert(id, node);
        self.disposers.insert(id, disposer);
        Ok(())
    }

    /// Put the **graph** back the way a refused `apply` found it: every node the
    /// failed apply added is removed, and the master-bus claim goes back to
    /// whoever held it (`remove_node` only clears the claim, it does not hand it
    /// on). The engine's own maps need no counterpart — `apply_mount` writes none
    /// of them until the apply has succeeded.
    ///
    /// **Additions and the claim, not everything**: a plugin that *removed* a node,
    /// or provided a context service, before it refused has already broken the
    /// contract below, and neither is restorable from here — the graph would need
    /// its whole node list rebuilt, and `Context` holds `Box<dyn Any>`. Both are
    /// [`Plugin::apply`]'s half of the deal: **an `Err` means nothing changed**,
    /// stated there the way [`OpHandler`] states its own.
    fn undo_a_refused_apply(&mut self, watermark: u64, bus: Option<NodeId>) {
        let added: Vec<NodeId> = self
            .graph
            .nodes()
            .iter()
            .map(|n| n.id)
            .filter(|id| id.0 >= watermark)
            .collect();
        for id in added {
            self.graph.remove_node(id);
        }
        self.graph.out_node = match bus {
            Some(id) if self.graph.nodes().iter().any(|n| n.id == id) => Some(id),
            _ => None,
        };
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
        // The graph's forward-order rule, asked **before** the log takes the patch
        // rather than at apply: `graph.connect` refuses a cord whose source does not
        // precede its destination, and a cord it will refuse must never be logged
        // (a logged mutation that never happens is a silent one — the destination
        // channel is never fed and nothing says so).
        if let (Some(from_rank), Some(to_rank)) = (self.graph_rank(fp), self.graph_rank(tp))
            && from_rank >= to_rank
        {
            return Err(format!(
                "patch: patch cords must go forward in node order — '{tp}' is mounted before \
                 '{fp}' (a queued mount lands behind every node already in the graph), so mount \
                 '{tp}' first"
            ));
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

    /// Where a plugin's node sits — or, for a queued mount, where it will sit — in
    /// the graph's forward order. `None` for a name the engine holds no order for
    /// (neither mounted nor carrying a scheduled mount's sequence), which the
    /// caller reads as "no answer", never as a refusal.
    ///
    /// A **mounted** plugin's answer is exact: its node's own index, asked of the
    /// graph. A **queued** mount has no node yet, and every plugin in the tree
    /// appends one in its `apply` — `Graph::insert_before` is the tool for a node
    /// that must *precede* an existing one, and it is not reachable from
    /// [`Self::patch`], whose endpoints are always plugins the engine itself
    /// mounted. So a queued mount lands after every applied node, and two queued
    /// mounts land in the order their mounts were scheduled (which is the order
    /// they apply in: the queue is frame-ordered, FIFO within a frame).
    ///
    /// Both readings share one scale — applied nodes rank `0..nodes.len()`, a
    /// queued mount ranks `nodes.len() + seq` — so the forward-order question is a
    /// single `>=` and needs no case analysis.
    fn graph_rank(&self, name: &str) -> Option<usize> {
        if let Some(&node) = self.node_of.get(name) {
            return self.graph.nodes().iter().position(|n| n.id == node);
        }
        let seq = *self.mount_seq.get(name)?;
        Some(self.graph.nodes().len().saturating_add(seq))
    }

    /// Apply a patch at its scheduled frame. Never panics: if an endpoint is not
    /// mounted (a log-order error) or the graph refuses the cord (forward order,
    /// single-driver control), the intent stays in the log and the refusal is
    /// **recorded** as an [`ApplyFault`] — loud in every build, so a session whose
    /// audio is not what its log says is one a host can mark degraded, rather than
    /// a destination channel that is silently never fed. Replay reproduces the
    /// same refusal, so a loaded session says the same thing a played one does.
    fn apply_patch(
        &mut self,
        from: (&'static str, &'static str),
        to: (&'static str, &'static str),
        at_frame: u64,
    ) {
        let Some(&from_node) = self.node_of.get(from.0) else {
            self.record_apply_fault(Self::patch_fault(
                from,
                to,
                at_frame,
                "its source is not mounted",
            ));
            return;
        };
        let Some(&to_node) = self.node_of.get(to.0) else {
            self.record_apply_fault(Self::patch_fault(
                from,
                to,
                at_frame,
                "its destination is not mounted",
            ));
            return;
        };
        if let Err(e) = self.graph.connect(from_node, from.1, to_node, to.1) {
            self.record_apply_fault(Self::patch_fault(from, to, at_frame, &e));
        }
    }

    /// The fault a scheduled patch's apply leaves behind. The cord is named the way
    /// the log spells it (the graph's own message speaks of nodes, which a host
    /// cannot resolve back to a plugin), and the reason is the refusal verbatim.
    fn patch_fault(
        from: (&'static str, &'static str),
        to: (&'static str, &'static str),
        at_frame: u64,
        reason: &str,
    ) -> ApplyFault {
        ApplyFault {
            // The source plugin: the cord's origin is what a user has to move.
            plugin: from.0,
            at_frame,
            reason: format!(
                "patch {}.{} → {}.{} refused: {reason}",
                from.0, from.1, to.0, to.1
            ),
        }
    }

    /// Schedule an unmount at an absolute frame — the scheduling queue driving
    /// lifecycle, sample-accurately.
    ///
    /// Fallible for one reason: in a **document walk** the unmount is half of a
    /// lifecycle, and a document whose unmount lands *before* the mount it ends
    /// cannot be read back (it would apply two instances — see
    /// [`Self::frame_inverted`]). Off the walk it cannot fail: the live path has no
    /// document to contradict, which is why `schedule_unmount` was infallible until
    /// the walk needed it to speak.
    pub fn schedule_unmount(&mut self, name: &'static str, at_frame: u64) -> Result<(), String> {
        if let Some(live) = self.walks.last_mut() {
            // In a **document walk** the name is free from here: the document says
            // the plugin goes away, so a later mount in the same document is a
            // re-mount. On the live path the release still happens at apply
            // (`apply_unmount`), which is why the same-tick `mount → unmount →
            // mount` window stays refused there.
            live.unmount(name, at_frame)?;
        }
        self.log.push(Event::ScheduleUnmount {
            plugin: name,
            at_frame,
        });
        self.scheduler
            .schedule(at_frame, SchedEvent::Unmount { plugin: name });
        Ok(())
    }

    /// Unmount at the current frame. Fail-loud: an unknown plugin is refused
    /// and never logged (kimi review finding 10 — the log must not record
    /// things that never happened).
    pub fn unmount(&mut self, name: &'static str) -> Result<(), String> {
        if !self.node_of.contains_key(name) && !self.scheduled.contains(name) {
            return Err(format!("plugin '{name}' is neither scheduled nor mounted"));
        }
        let at_frame = self.clock.frame();
        self.schedule_unmount(name, at_frame)
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

    /// Apply an unmount: run the disposer (reversible effects). Idempotent, and
    /// **total**: the name leaves every lifecycle map whether or not a disposer
    /// was found, so the maps describe what is mounted and never what was. A
    /// disposer-less entry cannot be built by the apply path any more (a mount
    /// that fails records neither `node_of` nor `disposers`), so the clears are
    /// unconditional rather than a courtesy of the disposer being present.
    fn apply_unmount(&mut self, name: &'static str) {
        self.scheduled.remove(name);
        self.scheduled_params.remove(name);
        self.mount_seq.remove(name);
        self.mounted_ports.remove(name);
        self.mounted_params.remove(name);
        self.node_of.remove(name);
        if let Some(disposer) = self.disposers.remove(name) {
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
    /// so it is tracked here as a document walk rather than read off `self.scheduled`:
    /// a replay applies nothing (nothing renders yet), so a name whose unmount the
    /// log schedules stays in `scheduled` for the whole walk — and the
    /// `mount … unmount … mount` shape the live engine produces (it releases the
    /// name when the unmount *applies*) would be refused as a second instance. The
    /// engine writes such logs; it must be able to read them.
    ///
    /// The walk also **refuses a frame-inverted log**
    /// ([`Self::frame_inverted`]): a log whose mount/unmount frames for one name are
    /// not in order would apply that name twice and orphan an instance, so it is a
    /// loud `Err` naming the frames rather than a silent leak. A refused replay
    /// leaves the target with the events it had already scheduled and nothing
    /// applied — the contract is a fresh engine, which the caller then discards.
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
                    // Before anything is scheduled or logged: the frame the
                    // scheduler will apply this mount at, checked against the
                    // document's own order, so a refused event leaves no trace.
                    if let Some(live) = self.walks.last_mut() {
                        live.mount(plugin, *at_frame)?;
                    }
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
                    // `scheduled_params`), not against the nominal catalog — and the
                    // queued mount's place in the graph's order (`graph_rank` asks
                    // `mount_seq`, written by the same `take_mount_seq` the live
                    // path writes), so a replayed cord is refused for a backward
                    // order exactly as a live one is.
                    self.scheduled.insert(plugin);
                    self.scheduled_params.insert(plugin, params.clone());
                    self.take_mount_seq(plugin);
                }
                Event::ScheduleUnmount { plugin, at_frame } => {
                    if let Some(live) = self.walks.last_mut() {
                        live.unmount(plugin, *at_frame)?;
                    }
                    self.scheduler
                        .schedule(*at_frame, SchedEvent::Unmount { plugin });
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

    /// Apply one scheduled event. `at_frame` is the frame the **log** stamped the
    /// event with, carried in from the call site rather than read off the clock:
    /// the queue is frame-ordered, but an event can be delivered late (a warm-up
    /// seek, a flush after the fact), and a fault report has to name the frame the
    /// document says, not the frame the engine noticed.
    fn apply_event(&mut self, event: SchedEvent, at_frame: u64) {
        match event {
            SchedEvent::Unmount { plugin } => self.apply_unmount(plugin),
            SchedEvent::Mount { plugin, params } => {
                // `apply_mount` must run in BOTH builds. The previous
                // `debug_assert!(self.apply_mount(...).is_ok(), ...)` form is a
                // *no-op in release* (debug_assert! never evaluates its
                // argument), so no scheduled mount was ever applied there and
                // the engine rendered silence on every mounted path — a
                // release-only bug (the 2026-08-30 note).
                //
                // A refusal is **recorded, not asserted**: `apply` is fallible by
                // design (the euclidean plugin's `apply` re-checks its `steps`
                // bound, because its fields are public and an instance can be
                // hand-assembled without the factory's door), so "the log and the
                // plugin disagree" is a fault a user must be able to see — and a
                // `debug_assert!` is invisible in release, the build that ships.
                // `apply_mount` has already undone what the refused apply touched,
                // so the session is not wedged: the name is free again and the
                // next mount of it succeeds.
                if let Err(reason) = self.apply_mount(plugin, &params) {
                    self.record_apply_fault(ApplyFault {
                        plugin,
                        at_frame,
                        reason,
                    });
                }
            }
            SchedEvent::Patch {
                from_plugin,
                from_port,
                to_plugin,
                to_port,
            } => self.apply_patch((from_plugin, from_port), (to_plugin, to_port), at_frame),
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

    /// Record a scheduled mutation the engine could not apply — a mount a plugin's
    /// `apply` refused, or a cord the graph refused. Bounded by
    /// [`MAX_APPLY_FAULTS`]: past it the fault is **counted** rather than kept
    /// ([`Self::apply_faults_dropped`]), the same "the bound is reported, not
    /// hidden" discipline as [`DrainOutcome::capped`] and the euclidean's
    /// `euclidean.drops`.
    fn record_apply_fault(&mut self, fault: ApplyFault) {
        if self.apply_faults.len() < MAX_APPLY_FAULTS {
            self.apply_faults.push(fault);
        } else {
            self.apply_faults_dropped += 1;
        }
    }

    /// Every apply the engine could not perform, oldest first — a mount the log
    /// asked for and the plugin refused, or a cord the graph refused, with the frame
    /// the log stamped and the refusal verbatim.
    ///
    /// **Never drained, and never cleared.** A fault is a standing fact about the
    /// session — the audio is not what the log says — so a shell that polls this
    /// cannot consume the evidence by looking at it (the opposite of
    /// [`Self::flush_scheduled`], which drains `parked` because a parked op is work
    /// still to be done). Bounded by [`MAX_APPLY_FAULTS`]; what did not fit is
    /// [`Self::apply_faults_dropped`].
    pub fn apply_faults(&self) -> &[ApplyFault] {
        &self.apply_faults
    }

    /// How many refusals the bound in [`MAX_APPLY_FAULTS`] could not hold. Zero on
    /// any healthy session; non-zero means the fault list is a *sample*, so a host
    /// must say "N refused applies" rather than "these are all of them".
    pub fn apply_faults_dropped(&self) -> usize {
        self.apply_faults_dropped
    }

    /// Whether the session is **degraded**: something the log scheduled could not
    /// be applied, so the audio is not what the document says. A shell shows this
    /// rather than rendering a playhead over a session that is missing a plugin.
    ///
    /// Sticky for the life of the engine, like the fault list itself: a re-render
    /// cannot undo a mount that never happened, and a poll that drained the flag
    /// would let a shell clear it by looking.
    pub fn is_degraded(&self) -> bool {
        !self.apply_faults.is_empty() || self.apply_faults_dropped > 0
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
    ///
    /// **A footgun, stated because it is a footgun:** moving the clock *backwards*
    /// across a scheduled lifecycle event lets a later event be stamped before an
    /// earlier one — `mount p; render; schedule_unmount p @f1; render; seek(0);
    /// mount p` writes `Mount p@0, Unmount p@f1, Mount p@0`, and the scheduler (which
    /// is frame-ordered) then applies two `Mount`s for one name and orphans the first
    /// instance. A document walk **refuses** such a log when it is read back
    /// ([`Self::frame_inverted`]) — loudly, which is the right outcome, but the caller
    /// that placed the clock backwards is the one to hear about it. Every in-tree
    /// caller places the clock forward on a session whose queue it is about to
    /// rebuild; a caller that seeks an already-running engine backwards owns the
    /// lifecycle it breaks. (`Clock` is a public field, so this is a stated obligation
    /// rather than an enforced one — there is nothing to enforce it *with*.)
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
        for (at_frame, event) in std::mem::take(&mut self.parked) {
            self.apply_event(event, at_frame);
        }
        while let Some(frame) = self.scheduler.peek_frame() {
            if frame > self.clock.frame() {
                break;
            }
            let event = self.scheduler.pop().expect("peeked");
            self.apply_event(event, frame);
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
            while let Some(f) = self.scheduler.peek_frame() {
                if f > next {
                    break;
                }
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
                    self.parked.push((f, event));
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
                    self.parked.push((f, event));
                    continue;
                }
                self.apply_event(event, f);
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
