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
//! The crate is UI-toolkit-free on purpose (no `crossterm`, no `iced`). It depends on
//! `host` (whose `host v1` vocabulary it is a keyboard face of) and on `media`, whose
//! [`Grid`](media::Grid) — the musical divisions an edit snaps to — is **shell state**:
//! the model names the divisions, the shell remembers which one is armed. The grid is
//! never logged (a snapped edit is an edit whose frame was quantized before the command
//! was issued), so this is vocabulary, not document.

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

/// The shell's **snap grid**: which musical division an edit is quantized to, or
/// `None` for no grid. This is UI state and is deliberately *not* logged: a snapped
/// edit is an edit whose frame was quantized before the command was issued, so the log
/// keeps absolute frames and replay stays deterministic.
///
/// The cycling order is the workflow's, not a shell's — two shells must not disagree
/// about what `b` means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Grid {
    division: Option<media::Division>,
}

impl Grid {
    /// No grid (the default: snapping changes an existing workflow, so it is armed
    /// explicitly).
    pub fn off() -> Self {
        Grid { division: None }
    }

    /// A grid already armed at `division` (a shell restoring a preference).
    pub fn new(division: media::Division) -> Self {
        Grid {
            division: Some(division),
        }
    }

    pub fn division(self) -> Option<media::Division> {
        self.division
    }

    pub fn is_on(self) -> bool {
        self.division.is_some()
    }

    /// The next step in the cycle: off → bar → beat → 1/2 → 1/4 → off. (Coarser
    /// first, so the first press from off gives the most useful grid.)
    pub fn cycle(&mut self) {
        self.division = match self.division {
            None => Some(media::Division::Bar),
            Some(media::Division::Bar) => Some(media::Division::Beat),
            Some(media::Division::Beat) => Some(media::Division::Half),
            Some(media::Division::Half) => Some(media::Division::Quarter),
            Some(media::Division::Quarter) => None,
        };
    }

    /// The step in beats under a meter of `beats_per_bar`, or `None` when off.
    pub fn grid(self, beats_per_bar: u32) -> Option<media::Grid> {
        self.division.map(|d| media::Grid::new(d, beats_per_bar))
    }

    /// A short label for a status line (`off`, `bar`, `beat`, `1/2`, `1/4`).
    pub fn label(self) -> &'static str {
        match self.division {
            None => "off",
            Some(d) => d.label(),
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
    /// Move the clip one grid step earlier (-1) / later (+1) — a beat when no grid
    /// is armed.
    Nudge(i32),
    /// Move the playhead to the previous (-1) / next (+1) grid line (a beat when no
    /// grid is armed).
    SeekGrid(i32),
    /// Cycle the snap grid: off → bar → beat → 1/2 → 1/4 → off.
    GridCycle,
    /// Copy the selection (or the clip under the playhead) to the shell's
    /// clipboard — a value, never logged: copying changes nothing.
    Yank,
    /// Copy **and** delete in one gesture (the clipboard plus a logged delete).
    Cut,
    /// Paste the clipboard at the playhead on the active track, as one logged
    /// gesture with minted ids.
    Paste,
    /// Paste **appended** after the active track's last clip (the arrangement's end
    /// when the track is empty) — no new op, just a different target frame.
    PasteAppend,
    /// Add a track (the shell mints its id) and make it active.
    TrackAdd,
    /// Rename the active track: the shell opens the command line prefilled with the
    /// `rename_track` line, so the name is typed through the host's own format.
    TrackRename,
    /// Delete the active track **and its clips** as one gesture.
    TrackDelete,
    /// Reorder the active track down (+1) / up (-1), keeping it active. (Not
    /// `MoveTrack` — that one moves a *clip* between tracks.)
    ReorderTrack(i32),
    /// Place the **selected pool source** on the active track at the playhead — the
    /// pool panel's "load this clip" (the clipboard's `p` is the same idea with the
    /// clipboard as its source).
    PoolPlace,
    /// Play the clip backwards / forwards again (a clip property, one log line).
    Reverse,
    /// Scale the clip's gain so its loudest sample hits full scale (measured from the
    /// peak pyramid — the shell reads it, the log gets one `set_clip_gain`).
    Normalize,
    /// Flip the clip's polarity (a gain of −1).
    Invert,
    /// Set the clip's gain to zero (silence, keeping its span and fades).
    Silence,
    /// Trim the clip to its audible content (a compound `trim` over the silence at
    /// its edges, measured from the peak pyramid).
    TrimToContent,
    /// **Tempo match**: time-stretch the clip so its material plays at the session's
    /// tempo. The ratio comes from the source's logged tempo (`source_tempo`), and the
    /// host renders the material into the pool — an offline transform, so it is a
    /// command rather than an arrange op (`undo` still restores the old reference).
    StretchToTempo,
    /// **Export the whole arrangement**: opens the command line prefilled with an
    /// `export <path> f32` line, so the path and format are visible and editable
    /// before anything is written (the render-out gesture, not a hidden write).
    ExportMix,
    /// **Name a point on the timeline** (`set_marker`), through the command line so the
    /// name is typed rather than guessed. Markers are what make a 30-minute piece
    /// navigable.
    MarkerSet,
    /// Jump the playhead to the next (+1) / previous (-1) marker.
    MarkerSeek(i32),
    /// **Name the clip under the playhead** (`rename_clip`), through the command line:
    /// a label a shell shows beside the id, so a long arrangement is recognisable.
    ClipRename,
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
    /// A short human name, for status lines and (later) the command palette.
    pub fn name(self) -> &'static str {
        match self {
            Action::PlayToggle => "play / stop",
            Action::Stop => "stop",
            Action::Rewind => "rewind",
            Action::SeekSeconds(_) => "seek by seconds",
            Action::SeekClip(_) => "next / previous clip",
            Action::CycleFocus(_) => "move the focus",
            Action::Vertical(_) => "move the selection",
            Action::Timeline(_) => "scroll / extend the selection",
            Action::Zoom(_) => "zoom or ride the fader",
            Action::Fit => "fit or unity",
            Action::MixerMute => "mute",
            Action::MixerSolo => "solo",
            Action::Visual => "visual mode",
            Action::Split => "split the clip",
            Action::Delete => "delete the clip",
            Action::TrimStart | Action::TrimEnd => "trim the clip",
            Action::TrimToSelection => "trim to the selection",
            Action::Nudge(_) => "move the clip a grid step",
            Action::SeekGrid(_) => "step the playhead a grid line",
            Action::GridCycle => "the snap grid",
            Action::Yank => "copy to the clipboard",
            Action::Cut => "cut to the clipboard",
            Action::Paste => "paste at the playhead",
            Action::PasteAppend => "paste appended to the track",
            Action::TrackAdd => "add a track",
            Action::TrackRename => "rename the track",
            Action::TrackDelete => "delete the track",
            Action::ReorderTrack(_) => "reorder the track",
            Action::PoolPlace => "place the pool source",
            Action::Reverse => "reverse the clip",
            Action::Normalize => "normalize the clip",
            Action::Invert => "invert the clip's polarity",
            Action::Silence => "silence the clip",
            Action::TrimToContent => "trim the clip to its content",
            Action::StretchToTempo => "stretch the clip to the session tempo",
            Action::ExportMix => "export the whole arrangement",
            Action::MarkerSet => "name a marker at the playhead",
            Action::MarkerSeek(_) => "jump to the next / previous marker",
            Action::ClipRename => "name the clip under the playhead",
            Action::MoveTrack(_) => "move the clip to another track",
            Action::Gain(_) => "clip gain",
            Action::Fade(_) => "fade",
            Action::Undo => "undo",
            Action::Redo => "redo",
            Action::Prompt => "the command line",
            Action::Help => "the keymap",
            Action::ToggleMouseCapture => "toggle mouse capture",
            Action::Quit => "quit",
            Action::Cancel => "cancel",
        }
    }

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
            // A cut is a logged `delete` (the copy half is a value); a paste is a
            // group of `add_clip`s, which is why it needs no vocabulary of its own.
            Action::Cut => Some("delete"),
            Action::Paste | Action::PasteAppend => Some("add_clip"),
            Action::TrackAdd => Some("add_track"),
            Action::TrackRename => Some("rename_track"),
            Action::TrackDelete => Some("remove_track"),
            Action::ReorderTrack(_) => Some("move_track"),
            Action::PoolPlace => Some("add_clip"),
            Action::Reverse => Some("reverse"),
            Action::Normalize | Action::Invert | Action::Silence => Some("set_clip_gain"),
            Action::TrimToContent => Some("trim"),
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
        "[  ]",
        &[
            (Key::Char('['), Action::SeekGrid(-1)),
            (Key::Char(']'), Action::SeekGrid(1)),
        ],
        "timeline: move the playhead to the previous / next grid line",
    ),
    bind(
        "b",
        &[(Key::Char('b'), Action::GridCycle)],
        "cycle the snap grid: off → bar → beat → 1/2 → 1/4 (the grid is never logged)",
    ),
    bind(
        "y  c",
        &[
            (Key::Char('y'), Action::Yank),
            (Key::Char('c'), Action::Cut),
        ],
        "copy / cut the selection (or the clip under the playhead) — a shell value, not a log entry",
    ),
    bind(
        "p  P",
        &[
            (Key::Char('p'), Action::Paste),
            (Key::Char('P'), Action::PasteAppend),
        ],
        "paste at the playhead / appended after the track's last clip (one undo step)",
    ),
    bind(
        "a  R  D",
        &[
            (Key::Char('a'), Action::TrackAdd),
            (Key::Char('R'), Action::TrackRename),
            (Key::Char('D'), Action::TrackDelete),
        ],
        "timeline: add a track / rename the active track / delete it and its clips",
    ),
    bind(
        "{  }",
        &[
            (Key::Char('{'), Action::ReorderTrack(-1)),
            (Key::Char('}'), Action::ReorderTrack(1)),
        ],
        "timeline: move the active track up / down (the mixer channel follows)",
    ),
    bind(
        "V  U  i  E  T",
        &[
            (Key::Char('V'), Action::Reverse),
            (Key::Char('U'), Action::Normalize),
            (Key::Char('i'), Action::Invert),
            (Key::Char('E'), Action::Silence),
            (Key::Char('T'), Action::TrimToContent),
        ],
        "timeline: reverse / normalize / invert / silence / trim to content (each one log line, one undo)",
    ),
    bind(
        "W",
        &[(Key::Char('W'), Action::StretchToTempo)],
        "timeline: warp — stretch the clip to the session tempo (needs `: source_tempo <id> <bpm>`)",
    ),
    bind(
        "'",
        &[(Key::Char('\''), Action::MarkerSet)],
        "timeline: name a marker at the playhead (`set_marker`) — the section list of a long piece",
    ),
    bind(
        "; / \"",
        &[
            (Key::Char(';'), Action::MarkerSeek(1)),
            (Key::Char('"'), Action::MarkerSeek(-1)),
        ],
        "timeline: jump to the next / previous marker (`;` forward, `\"` back)",
    ),
    bind(
        "C",
        &[(Key::Char('C'), Action::ClipRename)],
        "timeline: name the clip under the playhead (`rename_clip`) — a label beside the id",
    ),
    bind(
        "X",
        &[(Key::Char('X'), Action::ExportMix)],
        "anywhere: export the whole arrangement — opens the command line prefilled with `export <path> f32` (add `s16` for a dithered 16-bit file)",
    ),
    bind(
        "Enter",
        &[(Key::Enter, Action::PoolPlace)],
        "pool: place the selected source on the active track at the playhead \
         (creates the track when the pool is all there is)",
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
        "timeline: fade in / fade out to the playhead",
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
        "the command line — type any host v1 line (Esc cancels, ↑/↓ history)",
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
#[path = "tests/tests.rs"]
mod tests;
