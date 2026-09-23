//! The shell **workflow**, defined once (P1.x, `workflow`): the modal, key-driven
//! editing model the shells implement.
//!
//! The decision this crate exists to enforce: **one workflow, two shells.** The modal
//! model — modes, keys, and the `host v1` vocabulary behind them — is what the product
//! *is*; a shell is a rendering of it. So the model lives here, once, and the shells
//! (`spikes/tui-shell`, `spikes/iced-shell`) translate their toolkit's key events into
//! [`Key`] and dispatch the [`Action`] they get back. A shell that grows a key this
//! table does not have is a bug, not a feature — see
//! `.agents/notes/proposed/architecture/2026-09-21-modal-editing-model.md` (*One
//! workflow, two shells*) and the
//! [shells note](../../.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md).
//!
//! What is **not** here, deliberately: geometry and arguments. A shell owns the
//! playhead, the active track, the selected clip, and the text it builds — the crate
//! names the *commands* ([`Action`], and [`Action::command_name`] for the ones that
//! are log ops), not their arguments. That is the seam: the vocabulary is shared, the
//! document is the shell's.
//!
//! Two things are per-*panel* rather than per-key, and stay that way: `+`/`-`/`0` are
//! [`Action::Zoom`]/[`Action::Fit`], which the focused panel reads as "zoom / fit" or
//! "ride the fader / fader to unity"; and `j`/`k` are [`Action::Vertical`], a channel
//! or a track. The key is shared, the meaning is the focus's.
//!
//! The crate is UI-toolkit-free on purpose (no `crossterm`, no `iced`), and its one
//! dependency is `host`, whose `host v1` vocabulary it is a keyboard face of.

/// Which editing mode the shell is in. Always on screen: a modal UI that hides its
/// mode is a trap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Selection-first: an anchor and a head, extended by motions, consumed by an
    /// action.
    Visual,
    /// The `:` command line is open (text goes to the line, not to the document).
    Command,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Visual => "VISUAL",
            Mode::Command => "COMMAND",
        }
    }
}

/// A toolkit-neutral key press. Each shell translates its own event type into this;
/// nothing else about the toolkit crosses the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    /// Ctrl + a character.
    Ctrl(char),
    Space,
    Enter,
    Esc,
    Backspace,
    Tab,
    /// Shift-Tab (most terminals report it as its own code).
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Home,
}

/// What a key means. Context-free by design: the shell applies focus and mode when it
/// interprets the action, and **the direction is part of the action** — never
/// something a shell re-derives from the key, which is how two shells drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // transport
    PlayToggle,
    Stop,
    Rewind,
    SeekSeconds(i64),
    /// Jump the playhead to the next (`true`) / previous (`false`) clip start.
    SeekClip(bool),
    // panels and viewport
    CycleFocus(i32),
    /// Move down (+1) / up (-1) inside the focused panel: mixer channel, active track.
    Vertical(i32),
    /// Timeline: scroll (-1/+1) in normal mode, extend the selection in visual mode.
    Timeline(i64),
    /// The focused panel's "more / less": zoom in (+1) / out (-1), or ride the fader.
    Zoom(i32),
    /// The focused panel's neutral default: fit the timeline, or the fader to unity.
    Fit,
    MixerMute,
    MixerSolo,
    // the document
    Visual,
    Split,
    Delete,
    TrimStart,
    TrimEnd,
    TrimToSelection,
    /// Move the clip a beat earlier (-1) / later (+1).
    Nudge(i32),
    /// Move the clip to the track below (+1) / above (-1).
    MoveTrack(i32),
    /// Step the clip's gain in dB.
    Gain(i32),
    /// Put the fade-in (`true`) / fade-out (`false`) at the playhead.
    Fade(bool),
    Undo,
    Redo,
    // modes and the shell itself
    Prompt,
    Help,
    /// Terminal mouse capture — a *platform* capability, not a workflow one: a shell
    /// whose platform captures beyond its window (iced) has nothing to toggle and may
    /// treat this as a no-op.
    ToggleMouseCapture,
    Quit,
    /// Esc: leave whatever state is on top. Never quits.
    Cancel,
}

impl Action {
    /// The `host v1` operation this action dispatches, when it is a log op. The
    /// strings are the parser's own op names (`arrange <name> …`), so a shell builds a
    /// line and the test below proves the vocabulary still has it.
    pub fn command_name(self) -> Option<&'static str> {
        match self {
            Action::Split => Some("razor_split"),
            Action::Delete => Some("delete"),
            Action::TrimStart | Action::TrimEnd | Action::TrimToSelection => Some("trim"),
            Action::Nudge(_) => Some("move_clip"),
            Action::MoveTrack(_) => Some("move_clip_to_track"),
            Action::Gain(_) => Some("set_clip_gain"),
            Action::Fade(_) => Some("set_clip_fade"),
            _ => None,
        }
    }
}

/// One row of the keymap: a help line, the keys it names, and what each key means.
/// The help is **generated from this table**, so it cannot drift from the behaviour.
pub struct Binding {
    /// The keys to print, e.g. `"j  k  ↑  ↓"`. Every key in `bindings` must be spelled
    /// out here — the help names what a user is expected to press.
    pub keys: &'static str,
    /// The help-named keys, each with the action it means.
    pub bindings: &'static [(Key, Action)],
    /// Keys that also work but are not worth printing (vim's aliases: `=` for `+`).
    /// They are documented here rather than in the overlay, and the lookup honours
    /// them.
    pub aliases: &'static [(Key, Action)],
    pub help: &'static str,
}

const fn bind(
    keys: &'static str,
    bindings: &'static [(Key, Action)],
    help: &'static str,
) -> Binding {
    Binding {
        keys,
        bindings,
        aliases: &[],
        help,
    }
}

const fn bind_aliased(
    keys: &'static str,
    bindings: &'static [(Key, Action)],
    aliases: &'static [(Key, Action)],
    help: &'static str,
) -> Binding {
    Binding {
        keys,
        bindings,
        aliases,
        help,
    }
}

/// The keymap — the single source of truth for what a key does and what the help
/// says. Order is the help's order: transport, panels, viewport, console, document,
/// then the shell itself.
pub static KEYMAP: &[Binding] = &[
    bind("space", &[(Key::Space, Action::PlayToggle)], "play / stop"),
    bind("s", &[(Key::Char('s'), Action::Stop)], "stop"),
    bind(
        "r  Home",
        &[
            (Key::Char('r'), Action::Rewind),
            (Key::Home, Action::Rewind),
        ],
        "rewind to 0",
    ),
    bind(
        ",  .",
        &[
            (Key::Char(','), Action::SeekSeconds(-1)),
            (Key::Char('.'), Action::SeekSeconds(1)),
        ],
        "seek −1 s / +1 s (stops first)",
    ),
    bind(
        "n  N",
        &[
            (Key::Char('n'), Action::SeekClip(true)),
            (Key::Char('N'), Action::SeekClip(false)),
        ],
        "timeline: jump to the next / previous clip",
    ),
    bind(
        "Tab  Shift-Tab",
        &[
            (Key::Tab, Action::CycleFocus(1)),
            (Key::BackTab, Action::CycleFocus(-1)),
        ],
        "move between panels (mixer ⇄ timeline)",
    ),
    bind(
        "j  k  ↑  ↓",
        &[
            (Key::Char('j'), Action::Vertical(1)),
            (Key::Down, Action::Vertical(1)),
            (Key::Char('k'), Action::Vertical(-1)),
            (Key::Up, Action::Vertical(-1)),
        ],
        "the focused panel: mixer channel / active track",
    ),
    bind(
        "h  l  ←  →",
        &[
            (Key::Char('h'), Action::Timeline(-1)),
            (Key::Left, Action::Timeline(-1)),
            (Key::Char('l'), Action::Timeline(1)),
            (Key::Right, Action::Timeline(1)),
        ],
        "timeline: scroll (visual mode: extend the selection)",
    ),
    bind_aliased(
        "+  -",
        &[
            (Key::Char('+'), Action::Zoom(1)),
            (Key::Char('-'), Action::Zoom(-1)),
        ],
        &[
            (Key::Char('='), Action::Zoom(1)),
            (Key::Char('Z'), Action::Zoom(1)),
            (Key::Char('z'), Action::Zoom(-1)),
        ],
        "timeline: zoom in / out · mixer: ride the selected fader",
    ),
    bind(
        "0",
        &[(Key::Char('0'), Action::Fit)],
        "timeline: fit · mixer: fader to unity",
    ),
    bind(
        "M  S",
        &[
            (Key::Char('M'), Action::MixerMute),
            (Key::Char('S'), Action::MixerSolo),
        ],
        "mixer: mute / solo the selected channel",
    ),
    bind(
        "v",
        &[(Key::Char('v'), Action::Visual)],
        "visual mode: select from the playhead",
    ),
    bind(
        "x",
        &[(Key::Char('x'), Action::Split)],
        "timeline: split the clip under the playhead",
    ),
    bind(
        "d",
        &[(Key::Char('d'), Action::Delete)],
        "timeline: delete the clip under the playhead",
    ),
    bind(
        "<  >",
        &[
            (Key::Char('<'), Action::TrimStart),
            (Key::Char('>'), Action::TrimEnd),
        ],
        "timeline: trim the clip's start / end to the playhead",
    ),
    bind(
        "t",
        &[(Key::Char('t'), Action::TrimToSelection)],
        "visual: trim the clip to the selection (then leave visual)",
    ),
    bind(
        "H  L",
        &[
            (Key::Char('H'), Action::Nudge(-1)),
            (Key::Char('L'), Action::Nudge(1)),
        ],
        "timeline: move the clip one beat earlier / later",
    ),
    bind(
        "J  K",
        &[
            (Key::Char('J'), Action::MoveTrack(1)),
            (Key::Char('K'), Action::MoveTrack(-1)),
        ],
        "timeline: move the clip to the track below / above",
    ),
    bind(
        "g  G",
        &[
            (Key::Char('g'), Action::Gain(-1)),
            (Key::Char('G'), Action::Gain(1)),
        ],
        "timeline: clip gain −1 dB / +1 dB (1 dB steps)",
    ),
    bind(
        "f  F",
        &[
            (Key::Char('f'), Action::Fade(true)),
            (Key::Char('F'), Action::Fade(false)),
        ],
        "timeline: fade in / fade out **to the playhead**",
    ),
    bind(
        "u  Ctrl+r",
        &[
            (Key::Char('u'), Action::Undo),
            (Key::Ctrl('r'), Action::Redo),
        ],
        "undo / redo the last arrangement edit (a log replay)",
    ),
    bind(
        ":",
        &[(Key::Char(':'), Action::Prompt)],
        "the command line — type any `host v1` line (Esc cancels, ↑/↓ history)",
    ),
    bind(
        "Esc",
        &[(Key::Esc, Action::Cancel)],
        "leave visual mode / close this overlay (never quits)",
    ),
    bind(
        "m",
        &[(Key::Char('m'), Action::ToggleMouseCapture)],
        "toggle mouse capture (a terminal capability; a GUI has nothing to toggle)",
    ),
    bind("?", &[(Key::Char('?'), Action::Help)], "this keymap"),
    bind(
        "q  Ctrl+c",
        &[
            (Key::Char('q'), Action::Quit),
            (Key::Ctrl('c'), Action::Quit),
        ],
        "quit",
    ),
];

/// The action a key press means, or `None` for a key the workflow does not use. The
/// first row wins, and [`tests`] proves no key is bound twice.
pub fn action(key: Key) -> Option<Action> {
    KEYMAP.iter().find_map(|row| {
        row.bindings
            .iter()
            .chain(row.aliases)
            .find(|(bound, _)| *bound == key)
            .map(|(_, action)| *action)
    })
}

/// The help, generated from the keymap: `(keys, meaning)` in table order.
pub fn help() -> impl Iterator<Item = (&'static str, &'static str)> {
    KEYMAP.iter().map(|row| (row.keys, row.help))
}

/// The number of rows the `?` overlay renders.
pub fn help_len() -> usize {
    KEYMAP.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_keys(row: &Binding) -> impl Iterator<Item = (&Key, &Action)> {
        row.bindings
            .iter()
            .chain(row.aliases)
            .map(|(key, action)| (key, action))
    }

    #[test]
    fn every_listed_key_resolves_to_its_action() {
        for row in KEYMAP {
            for (key, action_) in all_keys(row) {
                assert_eq!(
                    action(*key),
                    Some(*action_),
                    "`{}` ({key:?}) must resolve to {action_:?}",
                    row.keys
                );
            }
        }
    }

    #[test]
    fn no_key_is_bound_twice() {
        // A duplicate binding is the classic way a keymap rots: the second row is dead
        // code that the help still advertises.
        let mut seen: Vec<Key> = Vec::new();
        for row in KEYMAP {
            for (key, _) in all_keys(row) {
                assert!(
                    !seen.contains(key),
                    "{key:?} is bound twice (the later row, `{}`, is unreachable)",
                    row.keys
                );
                seen.push(*key);
            }
        }
    }

    /// A paired row binds opposite directions — the property that makes `j`/`k` and
    /// `H`/`L` mean something without the shell re-deriving it from the key.
    #[test]
    fn paired_rows_bind_both_directions() {
        for row in KEYMAP {
            let directions: Vec<i32> = all_keys(row)
                .filter_map(|(_, action)| match action {
                    Action::Vertical(d)
                    | Action::Nudge(d)
                    | Action::MoveTrack(d)
                    | Action::Gain(d)
                    | Action::Zoom(d)
                    | Action::CycleFocus(d) => Some(*d),
                    Action::Timeline(d) => Some(*d as i32),
                    _ => None,
                })
                .collect();
            if directions.len() > 1 && row.keys.contains("  ") {
                assert!(
                    directions.contains(&1) && directions.contains(&-1),
                    "`{}` binds only one direction: {directions:?}",
                    row.keys
                );
            }
        }
    }

    #[test]
    fn paired_rows_that_should_oppose_do() {
        // SeekSeconds is the one paired row whose actions are not `*d` variants.
        let seek: Vec<i64> = KEYMAP
            .iter()
            .flat_map(all_keys)
            .filter_map(|(_, action)| match action {
                Action::SeekSeconds(s) => Some(*s),
                _ => None,
            })
            .collect();
        assert_eq!(seek, vec![-1, 1]);
    }

    /// The vocabulary test the modal note asks for: every action that claims to be a
    /// log op names an **op the host's parser accepts** — so a binding cannot drift
    /// from the log.
    #[test]
    fn every_log_op_names_a_real_host_operation() {
        // A syntactically complete line per op the workflow dispatches, in the host's
        // own text format.
        let samples: &[(&str, &str)] = &[
            ("razor_split", "razor_split t0 c0 L R 48000"),
            ("delete", "delete t0 c0"),
            ("trim", "trim t0 c0 start 4800"),
            ("move_clip", "move_clip t0 c0 96000"),
            ("move_clip_to_track", "move_clip_to_track t0 c0 t1 96000"),
            ("set_clip_gain", "set_clip_gain t0 c0 0.75"),
            ("set_clip_fade", "set_clip_fade t0 c0 4800 4800"),
        ];

        let mut claimed: Vec<&str> = Vec::new();
        for row in KEYMAP {
            for (_, action) in all_keys(row) {
                if let Some(op) = action.command_name() {
                    if !claimed.contains(&op) {
                        claimed.push(op);
                    }
                    let (_, line) = samples
                        .iter()
                        .find(|(name, _)| *name == op)
                        .unwrap_or_else(|| panic!("no sample line for op `{op}`"));
                    host::parse_arrange_line(line)
                        .unwrap_or_else(|e| panic!("the host refuses `{line}`: {e}"));
                }
            }
        }
        assert!(!claimed.is_empty());

        // …and the samples do not name ops the workflow forgot to bind.
        for (name, _) in samples {
            assert!(
                claimed.contains(name),
                "the workflow has no action for the op `{name}`"
            );
        }
    }

    #[test]
    fn the_help_is_generated_from_the_table() {
        let rows: Vec<_> = help().collect();
        assert_eq!(rows.len(), KEYMAP.len());
        assert_eq!(rows.len(), help_len());
        for (keys, meaning) in rows {
            assert!(!keys.trim().is_empty(), "a binding has no keys");
            assert!(!meaning.trim().is_empty(), "`{keys}` has no help text");
        }
    }

    /// The help's spelling must contain every key it *names*, or the overlay lies
    /// about what to press. Aliases are exempt by design (they are documented in the
    /// table, not printed).
    #[test]
    fn the_help_spelling_contains_the_keys_it_names() {
        for row in KEYMAP {
            for (key, _) in row.bindings {
                let c = match key {
                    Key::Char(c) | Key::Ctrl(c) => *c,
                    _ => continue,
                };
                let c = c.to_ascii_lowercase();
                assert!(
                    row.keys.to_ascii_lowercase().contains(c),
                    "{key:?} is named by `{}` but not spelled out in its help",
                    row.keys
                );
            }
        }
    }
}
