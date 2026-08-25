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
//! - the render loop never allocates (enforced by a counting-allocator test).

use std::collections::HashMap;

use crate::clock::{Clock, Scheduler};
use crate::graph::{Graph, NodeId, Port, RenderBlock, SignalKind, BLOCK};
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
    Unmount { plugin: &'static str },
    Patch {
        from_plugin: &'static str,
        from_port: &'static str,
        to_plugin: &'static str,
        to_port: &'static str,
    },
    SetTempo { bpm: f64, beats_per_bar: u32 },
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
    /// registered runtime parameter surfaces (validated by `set_param`).
    params_table: HashMap<&'static str, &'static [ParamDef]>,
    /// plugin name → its mounted primary node.
    node_of: HashMap<&'static str, NodeId>,
    disposers: HashMap<&'static str, Disposer>,
    /// plugins whose mount is queued but not yet applied.
    scheduled: std::collections::HashSet<&'static str>,
    /// registered plugin-message handlers, keyed by op (closed-core dispatch).
    op_handlers: HashMap<&'static str, OpHandler>,
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
            node_of: HashMap::new(),
            disposers: HashMap::new(),
            scheduled: std::collections::HashSet::new(),
            op_handlers: HashMap::new(),
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

    /// Mount a plugin at the current frame: validated synchronously (fail-loud),
    /// logged with its frame, then applied by the render loop at that frame.
    pub fn mount(&mut self, name: &'static str, params: &[(&'static str, f32)]) -> Result<(), String> {
        self.validate_mount(name, params)?;
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
        Ok(())
    }

    /// Synchronous, side-effect-free validation: known plugin, declared services
    /// all provided (core or ctx), one instance per name.
    fn validate_mount(&self, name: &'static str, params: &[(&'static str, f32)]) -> Result<(), String> {
        if self.disposers.contains_key(name) || self.scheduled.contains(name) {
            return Err(format!("plugin '{name}' is already mounted (one instance per name in spike A.5)"));
        }
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
        debug_assert_eq!(plugin.ports(), self.port_table[name], "registered ports must match the plugin");
        Ok(())
    }

    fn apply_mount(&mut self, name: &'static str, params: &[(&'static str, f32)]) -> Result<(), String> {
        let factory = *self
            .factories
            .get(name)
            .ok_or_else(|| format!("unknown plugin '{name}'"))?;
        let mut plugin = factory(params)?;
        let id = plugin.id();
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

    fn validate_patch(&self, from: (&'static str, &'static str), to: (&'static str, &'static str)) -> Result<(), String> {
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
        let from_port = self
            .port_table
            .get(fp)
            .and_then(|ports| ports.iter().find(|p| p.name == from.1))
            .ok_or_else(|| format!("no port '{}' on plugin '{fp}'", from.1))?;
        let to_port = self
            .port_table
            .get(tp)
            .and_then(|ports| ports.iter().find(|p| p.name == to.1))
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
        Ok(())
    }

    /// Apply a patch at its scheduled frame. Never panics: if an endpoint is
    /// not mounted (a log-order error) or the graph refuses the cord (forward
    /// order, single-driver control), the intent stays in the log and the
    /// refusal is asserted in debug — no audio-thread crash, no silent
    /// divergence (replay reproduces the same refused state).
    fn apply_patch(&mut self, from: (&'static str, &'static str), to: (&'static str, &'static str)) {
        let Some(&from_node) = self.node_of.get(from.0) else {
            debug_assert!(false, "patch endpoint '{}' not mounted at apply (log-order error)", from.0);
            return;
        };
        let Some(&to_node) = self.node_of.get(to.0) else {
            debug_assert!(false, "patch endpoint '{}' not mounted at apply (log-order error)", to.0);
            return;
        };
        if let Err(e) = self.graph.connect(from_node, from.1, to_node, to.1) {
            debug_assert!(false, "scheduled patch refused at apply: {e}");
        }
    }

    /// Schedule an unmount at an absolute frame — the scheduling queue driving
    /// lifecycle, sample-accurately.
    pub fn schedule_unmount(&mut self, name: &'static str, at_frame: u64) {
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

    /// Apply an unmount: run the disposer (reversible effects). Idempotent.
    fn apply_unmount(&mut self, name: &'static str) {
        self.scheduled.remove(name);
        if let Some(disposer) = self.disposers.remove(name) {
            self.node_of.remove(name);
            let Engine { ctx, scheduler, graph, .. } = self;
            disposer(&mut DisposerCtx { ctx, scheduler, graph });
        }
    }

    /// A tempo change at the current frame (sample-accurate: applied by the
    /// render loop at that frame). Fail-loud: a non-finite or non-positive
    /// tempo is refused and never logged (kimi review finding 10).
    pub fn set_tempo(&mut self, bpm: f64, beats_per_bar: u32) -> Result<(), String> {
        if !bpm.is_finite() || bpm <= 0.0 {
            return Err(format!("tempo must be finite and positive, got {bpm}"));
        }
        let at_frame = self.clock.frame();
        self.log.push(Event::SetTempo {
            bpm,
            beats_per_bar,
            at_frame,
        });
        self.scheduler
            .schedule(at_frame, SchedEvent::SetTempo { bpm, beats_per_bar });
        Ok(())
    }

    /// Set a discrete parameter on a plugin at the current frame: validated
    /// fail-loud (plugin registered and mounted-or-scheduled, name declared in
    /// the plugin's parameter surface, value finite and in range), logged, and
    /// applied by the render loop at its frame — sample-accurate and
    /// replayable (Phase 1: the generic control path for the mixer's
    /// gain/mute/solo/fader). A refused call is never logged; automation
    /// curves are a later event type.
    pub fn set_param(&mut self, plugin: &'static str, param: &'static str, value: f32) -> Result<(), String> {
        if !self.factories.contains_key(plugin) {
            return Err(format!("unknown plugin '{plugin}'"));
        }
        if !(self.node_of.contains_key(plugin) || self.scheduled.contains(plugin)) {
            return Err(format!("plugin '{plugin}' is neither scheduled nor mounted"));
        }
        let declared = self.params_table.get(plugin).copied().unwrap_or(&[]);
        let Some(def) = declared.iter().find(|d| d.name == param) else {
            return Err(format!("plugin '{plugin}' has no parameter '{param}'"));
        };
        if !value.is_finite() {
            return Err(format!("parameter '{param}' must be finite, got {value}"));
        }
        if value < def.min || value > def.max {
            return Err(format!("parameter '{param}' out of range [{}, {}]: {value}", def.min, def.max));
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
    pub fn register_op_handler(&mut self, op: &'static str, handler: OpHandler) -> Result<(), String> {
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
    fn validate_op(&self, op: &'static str, fields: &[(&'static str, Value)]) -> Result<(), String> {
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
        self.log.push(Event::Arrangement { op, fields, at_frame });
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
    pub fn arrange(&mut self, op: &'static str, fields: Vec<(&'static str, Value)>) -> Result<(), String> {
        self.validate_op(op, &fields)?;
        let at_frame = self.log_arrangement(op, fields.clone());
        self.scheduler.schedule(at_frame, SchedEvent::Arrangement { op, fields });
        Ok(())
    }

    /// Like [`arrange`], but only **logs** the op — it is not scheduled. Used for
    /// value-level arrangement ops whose live value is applied eagerly by the
    /// clip editor (so they never touch the scheduler and cannot flush the mixer
    /// early); `replay_from` re-schedules them to reconstruct the value.
    pub fn arrange_logged(&mut self, op: &'static str, fields: Vec<(&'static str, Value)>) -> Result<(), String> {
        self.validate_op(op, &fields)?;
        self.log_arrangement(op, fields);
        Ok(())
    }

    /// Replay a log onto this engine. Must be a *fresh* engine: every event is
    /// scheduled at its recorded frame and applied by the render loop — nothing
    /// is applied eagerly, so the timeline reproduces exactly.
    pub fn replay_from(&mut self, log: &SessionLog) -> Result<(), String> {
        for event in log.events() {
            match event {
                Event::Mount {
                    plugin,
                    params,
                    at_frame,
                } => {
                    self.validate_mount(plugin, params)?;
                    self.scheduler.schedule(
                        *at_frame,
                        SchedEvent::Mount {
                            plugin,
                            params: params.clone(),
                        },
                    );
                    self.scheduled.insert(plugin);
                }
                Event::ScheduleUnmount { plugin, at_frame } => {
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
                Event::Arrangement { op, fields, at_frame } => {
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
                debug_assert!(
                    self.apply_mount(plugin, &params).is_ok(),
                    "scheduled mount must apply (log was validated)"
                );
            }
            SchedEvent::Patch {
                from_plugin,
                from_port,
                to_plugin,
                to_port,
            } => self.apply_patch((from_plugin, from_port), (to_plugin, to_port)),
            SchedEvent::SetTempo { bpm, beats_per_bar } => {
                self.clock.push_tempo(bpm, beats_per_bar);
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
                    None => debug_assert!(false, "set_param endpoint '{plugin}' not mounted at apply"),
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
                    None => debug_assert!(false, "arrangement op '{op}' has no registered handler at apply"),
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

    /// Render `frames` samples into a fresh buffer.
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; frames];
        self.render_into(&mut out);
        out
    }

    /// Render into a caller-provided buffer, in fixed-size blocks.
    pub fn render_into(&mut self, out: &mut [f32]) {
        for chunk in out.chunks_mut(BLOCK) {
            self.render_block(chunk);
        }
    }

    /// Apply every scheduled event due at the current frame **without
    /// rendering audio** — the control→render handoff's seed (the reference
    /// host materializes scheduled mounts before wiring cords; kimi review
    /// finding 5: no discarded block).
    pub fn flush_scheduled(&mut self) {
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
    fn render_block(&mut self, out: &mut [f32]) {
        let f1 = self.clock.frame() + out.len() as u64;
        let mut pos = self.clock.frame();
        let mut written = 0usize;
        loop {
            let next = self.scheduler.peek_frame().filter(|f| *f < f1).unwrap_or(f1);
            if next > pos {
                let len = (next - pos) as usize;
                self.render_chunk(&mut out[written..written + len], pos);
                written += len;
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
                            // I/O), which must not run on the audio thread. A
                            // host that reaches one here failed to flush before
                            // rendering — fail loud in debug (the op stays
                            // logged; replay reproduces the same skip).
                            debug_assert!(false, "an arrangement op reached the render stack; flush_scheduled before rendering");
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

    /// Render one contiguous chunk. The render path allocates nothing.
    fn render_chunk(&mut self, out: &mut [f32], f0: u64) {
        let Engine { clock, graph, .. } = self;
        let block = RenderBlock {
            frame: f0,
            sample_rate: clock.sample_rate,
            tempo: &clock.tempo_map,
        };
        graph.render(out, block);
        clock.advance(out.len() as u64);
    }
}
