//! Core piece 4 — context plumbing: a repository of services, `inject`
//! dependency checks, and reversible registration (minimal-core note §4).
//!
//! This is the control-plane side of the composition discipline: a plugin
//! declares the services it requires and the host refuses to mount it when a
//! requirement is missing (fail-loud). The Rust side gets the check at *compile
//! time* for its own seams (traits); this runtime registry is the seed of the
//! TS-side context.

use std::any::Any;
use std::collections::HashMap;

/// A repository of services keyed by name (`ctx.clock`, `ctx.sessions`, …).
/// Consumers find services by key, never by importing an implementation.
#[derive(Default)]
pub struct Context {
    services: HashMap<&'static str, Box<dyn Any>>,
}

impl Context {
    pub fn new() -> Self {
        Context {
            services: HashMap::new(),
        }
    }

    /// Provide (or replace) a service under a key.
    pub fn provide(&mut self, key: &'static str, value: impl Any) {
        self.services.insert(key, Box::new(value));
    }

    /// Withdraw a service (used by disposers).
    pub fn remove(&mut self, key: &'static str) {
        self.services.remove(key);
    }

    pub fn has(&self, key: &'static str) -> bool {
        self.services.contains_key(key)
    }

    pub fn get<T: Any>(&self, key: &'static str) -> Option<&T> {
        self.services
            .get(key)
            .and_then(|boxed| boxed.downcast_ref())
    }
}
