//! A closed-core value for plugin messages (P1.3.2, `value`).
//!
//! The engine is std-only and must not know plugin vocabulary, so a plugin log
//! event carries an **op name** plus a field list of these values. The plugin
//! registered a handler that interprets the fields. Keeping this in the core
//! (not the plugin) is what lets the log be replayed and the op applied at a
//! frame without the core growing one variant per plugin op.

/// A scalar value carried on a plugin message. `&'static str` is the interned
/// id form (target/clip/track/source ids are interned to `&'static str`); `f32`
/// is canonicalised bit-exactly by the sender, so the log round-trips.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// An interned id / name (`&'static str`).
    Str(&'static str),
    /// A frame count or large unsigned position.
    U64(u64),
    /// A signed delta (trim by, loop times as i64 when needed).
    I64(i64),
    /// A small unsigned quantity (channel, times, etc.).
    U32(u32),
    /// A scalar (gain, fade length) — sender uses bit-exact `to_bits` semantics.
    F32(f32),
}

/// A plugin message: an op name (interned) plus its fields, in declaration order.
#[derive(Debug, Clone, PartialEq)]
pub struct OpMsg {
    pub op: &'static str,
    pub fields: Vec<(&'static str, Value)>,
}

impl OpMsg {
    /// Get a field by name; `None` when absent.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    /// Get a field as `u64`; `None` when absent or the wrong type.
    pub fn u64(&self, name: &str) -> Option<u64> {
        match self.get(name) {
            Some(Value::U64(v)) => Some(*v),
            Some(Value::U32(v)) => Some(*v as u64),
            Some(Value::I64(v)) => u64::try_from(*v).ok(),
            _ => None,
        }
    }

    /// Get a field as `f32`; `None` when absent or the wrong type.
    pub fn f32(&self, name: &str) -> Option<f32> {
        match self.get(name) {
            Some(Value::F32(v)) => Some(*v),
            _ => None,
        }
    }
}
