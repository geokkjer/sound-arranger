//! The engine: assembles the four core pieces and drives the render loop.
//!
//! The public mutation API — `mount`, `schedule_unmount`, `unmount`, `set_tempo`,
//! `replay_from` — is the seed of the core plugin contract (the future IPC
//! command list, per the composition-seams note).
//!
//! Invariants:
//! - every mutation is **logged at call time with its absolute frame**, then
//!   scheduled; the render loop applies it when the clock reaches that frame
//!   (sample-accurate lifecycle; a refused mutation is never logged);
//! - rendering is a pure function of the log: `same log ⇒ byte-identical bounce`
//!   (determinism is tested, including mid-session changes);
//! - the render loop never allocates: fixed block size, preallocated scratch,
//!   lazy scheduler drains, generators that emit via callback (enforced by a
//!   counting-allocator test).

use std::collections::HashMap;

use crate::clock::{Clock, Scheduler};
use crate::graph::{Graph, RenderBlock, BLOCK};
use crate::log::{Event, SessionLog};
use crate::plugins::{Disposer, DisposerCtx, GeneratorMount, Plugin, PluginApi};

/// One-shot events the scheduling queue delivers at an exact absolute frame.
#[derive(Debug, Clone, PartialEq)]
pub enum SchedEvent {
    Mount {
        plugin: &'static str,
        params: Vec<(&'static str, f32)>,
    },
    Unmount { plugin: &'static str },
    SetTempo { bpm: f64, beats_per_bar: u32 },
}

/// Builds a plugin from a flat parameter list (the `Event::Mount` payload).
pub type PluginFactory = fn(&[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String>;

/// The assembled minimal core.
pub struct Engine {
    pub clock: Clock,
    pub scheduler: Scheduler<SchedEvent>,
    pub graph: Graph,
    pub ctx: crate::ctx::Context,
    pub log: SessionLog,
    factories: HashMap<&'static str, PluginFactory>,
    generators: Vec<GeneratorMount>,
    disposers: HashMap<&'static str, Disposer>,
    /// plugins whose mount is queued but not yet applied.
    scheduled: std::collections::HashSet<&'static str>,
}

impl Engine {
    /// Core services the engine itself satisfies — the clock is core, never a
    /// ctx-provided service (a plugin may not own time, and a clone would go
    /// stale). Plugins access it read-only via `PluginApi.clock`.
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
            generators: Vec::new(),
            disposers: HashMap::new(),
            scheduled: std::collections::HashSet::new(),
        }
    }

    pub fn register_factory(&mut self, name: &'static str, factory: PluginFactory) {
        self.factories.insert(name, factory);
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
            return Err(format!("plugin '{name}' is already mounted (one instance per name in spike A)"));
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
        Ok(())
    }

    fn apply_mount(&mut self, name: &'static str, params: &[(&'static str, f32)]) -> Result<(), String> {
        let factory = *self
            .factories
            .get(name)
            .ok_or_else(|| format!("unknown plugin '{name}'"))?;
        let mut plugin = factory(params)?;
        let id = plugin.id();
        let disposer = {
            let Engine {
                ctx,
                scheduler,
                graph,
                clock,
                generators,
                ..
            } = self;
            let mut api = PluginApi {
                ctx,
                scheduler,
                graph,
                clock,
                generators,
            };
            plugin.apply(&mut api)?
        };
        self.scheduled.remove(name);
        self.disposers.insert(id, disposer);
        Ok(())
    }

    /// Schedule an unmount at an absolute frame — the scheduling queue driving
    /// lifecycle, sample-accurately (the render loop applies it at that frame).
    pub fn schedule_unmount(&mut self, name: &'static str, at_frame: u64) {
        self.log.push(Event::ScheduleUnmount {
            plugin: name,
            at_frame,
        });
        self.scheduler
            .schedule(at_frame, SchedEvent::Unmount { plugin: name });
    }

    /// Unmount at the current frame.
    pub fn unmount(&mut self, name: &'static str) {
        let at_frame = self.clock.frame();
        self.schedule_unmount(name, at_frame);
    }

    /// Apply an unmount: run the disposer (reversible effects). Idempotent.
    fn apply_unmount(&mut self, name: &'static str) {
        self.scheduled.remove(name);
        if let Some(disposer) = self.disposers.remove(name) {
            let Engine {
                ctx,
                scheduler,
                graph,
                generators,
                ..
            } = self;
            disposer(&mut DisposerCtx {
                ctx,
                scheduler,
                graph,
                generators,
            });
        }
    }

    /// A tempo change at the current frame (logged with its frame, then applied
    /// by the render loop at that frame — sample-accurate tempo changes).
    pub fn set_tempo(&mut self, bpm: f64, beats_per_bar: u32) {
        let at_frame = self.clock.frame();
        self.log.push(Event::SetTempo {
            bpm,
            beats_per_bar,
            at_frame,
        });
        self.scheduler
            .schedule(at_frame, SchedEvent::SetTempo { bpm, beats_per_bar });
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
            }
        }
        Ok(())
    }

    fn apply_event(&mut self, event: SchedEvent) {
        match event {
            SchedEvent::Unmount { plugin } => self.apply_unmount(plugin),
            SchedEvent::Mount { plugin, params } => {
                // The log was validated at schedule time; a failure here is a
                // bug in the log, not a runtime error.
                debug_assert!(
                    self.apply_mount(plugin, &params).is_ok(),
                    "scheduled mount must apply (log was validated)"
                );
            }
            SchedEvent::SetTempo { bpm, beats_per_bar } => {
                self.clock.push_tempo(bpm, beats_per_bar);
            }
        }
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

    /// Render one block, interleaving the scheduling queue: events are applied
    /// at their exact absolute frame by splitting the block around them, so a
    /// lifecycle change mid-block takes effect at the right sample.
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
            // Apply every event scheduled at this frame (FIFO). Pops, so no
            // allocation; the iterator form exists for tests.
            loop {
                let frame = self.scheduler.peek_frame();
                match frame {
                    Some(f) if f <= next => {
                        let event = self.scheduler.pop().expect("peeked");
                        self.apply_event(event);
                    }
                    _ => break,
                }
            }
        }
        debug_assert_eq!(written, out.len());
    }

    /// Render one contiguous chunk: pull generator triggers, render the graph,
    /// advance the clock. The render path allocates nothing.
    fn render_chunk(&mut self, out: &mut [f32], f0: u64) {
        let f1 = f0 + out.len() as u64;
        let Engine {
            clock,
            graph,
            generators,
            ..
        } = self;
        let range = f0..f1;
        for mount in generators.iter() {
            let tempo = &clock.tempo_map;
            mount.generator.for_each_trigger(range.clone(), tempo, &mut |frame| {
                let offset = (frame - f0) as u32;
                graph.trigger(mount.node, offset);
            });
        }
        graph.render(out, RenderBlock {
            frame: f0,
            sample_rate: clock.sample_rate,
        });
        clock.advance(out.len() as u64);
    }
}
