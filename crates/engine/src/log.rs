//! Core piece 3 — the session event log (append-only; *model-visible means
//! logged*, minimal-core note §3).
//!
//! A composition *is* a log: the engine mutates itself only by applying events,
//! so rendering is a pure function of the log. Events reference audio by id and
//! carry absolute frames; nothing audio-rate is ever logged.

/// Absolute-frame events of the Spike A session log. Every event carries the
/// frame at which it takes effect — positions are stored in absolute sample
/// frames, so replay reproduces the exact timeline (the log's time-basis rule).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A plugin was mounted with its configuration at an absolute frame (replay
    /// reconstructs the plugin from its factory + params and applies it when the
    /// clock reaches the frame).
    Mount {
        plugin: &'static str,
        params: Vec<(&'static str, f32)>,
        at_frame: u64,
    },
    /// A plugin is scheduled to unmount at an absolute frame (the scheduling
    /// queue drives lifecycle).
    ScheduleUnmount { plugin: &'static str, at_frame: u64 },
    /// A patch cord between two plugins' ports, at an absolute frame.
    Patch {
        from_plugin: &'static str,
        from_port: &'static str,
        to_plugin: &'static str,
        to_port: &'static str,
        at_frame: u64,
    },
    /// A tempo change at an absolute frame.
    SetTempo {
        bpm: f64,
        beats_per_bar: u32,
        at_frame: u64,
    },
}

/// The append-only session log.
#[derive(Debug, Clone, Default)]
pub struct SessionLog {
    events: Vec<Event>,
}

impl SessionLog {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
        }
    }

    pub fn push(&mut self, event: Event) {
        self.events.push(event);
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}
