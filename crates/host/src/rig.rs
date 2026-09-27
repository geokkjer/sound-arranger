//! The rig: the sources a session *declares* it will capture.
//!
//! A rig declaration is **data, not hardware** — the same purity rule the takes
//! slice follows: a session names the sources it intends to capture, the log
//! carries those names, and a replay rebuilds the rig **without opening,
//! binding or probing any device**. Binding a declared source to a real device
//! is a device-bound side effect a later slice adds; nothing here touches one.
//!
//! Identity is deliberately **not** a device index: indices renumber across
//! reboots and replugs, so a declaration is a stable name plus a matching rule.

/// The seam family a source belongs to. A closed list, like the plugin
/// registry: an unknown kind is a loud parse error, not a silently inert
/// declaration.
pub const SOURCE_KINDS: &[&str] = &["alsa", "jack", "osc"];

/// Look up a source kind, mirroring the host's registry lookup (`in_list`):
/// an unknown kind names the registry rather than guessing.
pub fn source_kind(s: &str) -> Result<&'static str, String> {
    SOURCE_KINDS
        .iter()
        .find(|k| **k == s)
        .copied()
        .ok_or_else(|| {
            format!(
                "unknown source kind '{s}' (registry: {})",
                SOURCE_KINDS.join(", ")
            )
        })
}

/// A source's place in the clock scheme. This is where the later clock-out
/// slice finds its destination: every `Master` defines the time, and every
/// `Follower` is one the clock is sent to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockRole {
    /// Defines the time: the session follows this source's clock.
    Master,
    /// Receives clock from the session (the clock-out slice's destination).
    Follower,
    /// No clock relationship: recorded and aligned afterwards.
    Free,
}

impl ClockRole {
    /// The word the `host v1` form uses (`clock=<role>`).
    pub fn as_str(self) -> &'static str {
        match self {
            ClockRole::Master => "master",
            ClockRole::Follower => "follower",
            ClockRole::Free => "free",
        }
    }

    /// Parse the word back. No default, no guessing: anything else is refused,
    /// because a mis-parsed role would silently change what the clock slice
    /// drives.
    pub fn parse(s: &str) -> Result<ClockRole, String> {
        match s {
            "master" => Ok(ClockRole::Master),
            "follower" => Ok(ClockRole::Follower),
            "free" => Ok(ClockRole::Free),
            other => Err(format!(
                "unknown clock role '{other}' (use master, follower or free)"
            )),
        }
    }
}

/// One declared source. Identity is a stable `name` plus a matching *rule*
/// (`matcher`), never a device index — indices renumber across reboots and
/// replugs, so a session that named an index would replay against the wrong
/// device or none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDecl {
    /// The stable name the session addresses the source by.
    pub name: String,
    /// The closed seam family ([`SOURCE_KINDS`]) the binding will live in.
    pub kind: &'static str,
    /// The one-token rule that resolves `name` to a device at run time (when
    /// the later binding slice adds one).
    pub matcher: String,
    /// The declared channel width.
    pub channels: usize,
    /// The source's clock role.
    pub clock: ClockRole,
}

/// The rig: the set of sources the recorder intends to capture. Pure session
/// state — a rebuilt session carries the same declarations with no device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rig {
    pub sources: Vec<SourceDecl>,
}

impl Rig {
    /// The declaration a name addresses, if the rig declares it.
    pub fn source(&self, name: &str) -> Option<&SourceDecl> {
        self.sources.iter().find(|s| s.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_role_round_trips() {
        for role in [ClockRole::Master, ClockRole::Follower, ClockRole::Free] {
            assert_eq!(ClockRole::parse(role.as_str()), Ok(role));
        }
    }

    #[test]
    fn clock_role_refuses_the_unknown() {
        assert!(ClockRole::parse("Master").is_err(), "case-sensitive words");
        assert!(ClockRole::parse("slave").is_err(), "no guessing");
    }

    #[test]
    fn unknown_kind_names_the_registry() {
        assert!(source_kind("firewire").is_err());
        assert!(source_kind("alsa").is_ok());
    }

    #[test]
    fn rig_looks_up_by_name() {
        let mut rig = Rig::default();
        rig.sources.push(SourceDecl {
            name: "synth".into(),
            kind: "alsa",
            matcher: "hw:USB".into(),
            channels: 2,
            clock: ClockRole::Follower,
        });
        assert!(rig.source("synth").is_some());
        assert!(rig.source("drum").is_none());
    }
}
