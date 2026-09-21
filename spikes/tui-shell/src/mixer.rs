//! The mixer panel — channel strips with a fader, a meter, a name and a value,
//! arranged vertically like a hardware console.
//!
//! ratatui has no slider widget (the `vertical_slider` I first reached for is
//! iced's), so the strips are drawn straight into the buffer. That is also what
//! makes them a *real* mixer rather than a row of bars: a fader has a position
//! you can see and set, and a meter sits next to it showing what the engine is
//! actually doing.
//!
//! One thing the strips deliberately do **not** own: the truth about gain. The
//! host owns audio state; the shell holds the values it has asked for and sends
//! each change as a logged `set_param` — a fader ride is automation in the
//! session log, which is the same thing a real console's moving fader records.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Strip widths, in terminal columns: a fader, a meter, and room for the name
/// and the value. The narrow variant drops the value row.
const STRIP_WIDE: u16 = 5;
const STRIP_NARROW: u16 = 3;

/// One channel's fader position over `[0, 1]`, plus its mute/solo state. The
/// master is the strip after the channels.
pub struct Mixer {
    pub gains: Vec<f32>,
    pub mutes: Vec<bool>,
    pub solos: Vec<bool>,
    pub master: f32,
    channels: usize,
}

impl Mixer {
    pub fn new(channels: usize) -> Self {
        Mixer {
            gains: vec![1.0; channels],
            mutes: vec![false; channels],
            solos: vec![false; channels],
            master: 1.0,
            channels,
        }
    }

    /// Grow/shrink to the mounted channel count, keeping what was set.
    pub fn resize(&mut self, channels: usize) {
        if channels == self.channels {
            return;
        }
        self.gains.resize(channels, 1.0);
        self.mutes.resize(channels, false);
        self.solos.resize(channels, false);
        self.channels = channels;
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Strips including the master.
    pub fn strips(&self) -> usize {
        self.channels + 1
    }

    /// `strip` is a channel index, or `channels` for the master.
    pub fn value(&self, strip: usize) -> f32 {
        if strip >= self.channels {
            self.master
        } else {
            self.gains[strip]
        }
    }

    pub fn set_value(&mut self, strip: usize, value: f32) {
        let value = value.clamp(0.0, 1.0);
        if strip >= self.channels {
            self.master = value;
        } else {
            self.gains[strip] = value;
        }
    }

    /// Nudge a strip by `delta` (keys), saturating at the ends.
    pub fn nudge(&mut self, strip: usize, delta: f32) {
        self.set_value(strip, self.value(strip) + delta);
    }

    pub fn muted(&self, strip: usize) -> bool {
        self.mutes.get(strip).copied().unwrap_or(false)
    }

    pub fn soloed(&self, strip: usize) -> bool {
        self.solos.get(strip).copied().unwrap_or(false)
    }

    /// Toggle mute. The master has none.
    pub fn toggle_mute(&mut self, strip: usize) {
        if strip < self.channels {
            self.mutes[strip] = !self.mutes[strip];
        }
    }

    /// Set mute outright — what the host's folded params say, rather than a toggle.
    pub fn set_muted(&mut self, strip: usize, muted: bool) {
        if strip < self.channels {
            self.mutes[strip] = muted;
        }
    }

    pub fn set_soloed(&mut self, strip: usize, soloed: bool) {
        if strip < self.channels {
            self.solos[strip] = soloed;
        }
    }

    pub fn toggle_solo(&mut self, strip: usize) {
        if strip < self.channels {
            self.solos[strip] = !self.solos[strip];
        }
    }

    pub fn name(&self, strip: usize) -> String {
        if strip >= self.channels {
            "mst".to_string()
        } else {
            format!("ch{strip}")
        }
    }

    /// The `set_param` target for a strip: `ch<n>.gain` or `master.gain`.
    pub fn gain_param(&self, strip: usize) -> String {
        if strip >= self.channels {
            "master.gain".to_string()
        } else {
            format!("ch{strip}.gain")
        }
    }

    pub fn mute_param(&self, strip: usize) -> String {
        format!("ch{strip}.mute")
    }

    pub fn solo_param(&self, strip: usize) -> String {
        format!("ch{strip}.solo")
    }
}

/// What the mixer drew: each strip's rect and where its fader column is, so the
/// mouse can select and drag.
#[derive(Debug, Clone, Default)]
pub struct Strips {
    pub outer: Rect,
    pub strips: Vec<(usize, Rect, Rect)>,
}

impl Strips {
    /// The strip under a position, plus the value its fader column implies
    /// (`ratio` 0 at the bottom, 1 at the top).
    pub fn at(&self, position: ratatui::layout::Position) -> Option<(usize, f32)> {
        for (strip, area, fader) in &self.strips {
            if area.contains(position) {
                let top = fader.y;
                let bottom = fader.y + fader.height.saturating_sub(1);
                let ratio = if bottom <= top {
                    1.0
                } else {
                    (bottom.saturating_sub(position.y)) as f32 / (bottom - top) as f32
                };
                return Some((*strip, ratio.clamp(0.0, 1.0)));
            }
        }
        None
    }

    pub fn contains(&self, position: ratatui::layout::Position) -> bool {
        self.outer.contains(position)
    }
}

/// Draw the console. `levels` are the per-channel meter peaks and `master` the
/// master peak (both from the host's snapshot, post-gain).
pub fn draw(
    frame: &mut ratatui::Frame,
    area: Rect,
    mixer: &Mixer,
    levels: &[f32],
    master_level: f32,
    selected: usize,
    focused: bool,
) -> Strips {
    let border = if focused {
        Color::LightBlue
    } else {
        Color::DarkGray
    };
    let block = ratatui::widgets::Block::bordered()
        .title(" mixer ")
        .border_style(Style::new().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut drawn = Strips {
        outer: area,
        ..Strips::default()
    };

    if inner.width == 0 || inner.height < 3 {
        return drawn;
    }

    let strips = mixer.strips();
    let width = if inner.width as usize >= strips * STRIP_WIDE as usize {
        STRIP_WIDE
    } else {
        STRIP_NARROW
    };
    // Only the strips that fit are drawn; the rest are reported in the title of
    // the state line rather than silently dropped.
    let visible = ((inner.width + 1) / (width + 1)).min(strips as u16) as usize;

    for strip in 0..visible {
        let x = inner.x + strip as u16 * (width + 1);
        let strip_area = Rect::new(x, inner.y, width, inner.height);
        let fader = Rect::new(x, inner.y + 1, 1, inner.height.saturating_sub(2));
        drawn.strips.push((strip, strip_area, fader));

        let color = if strip >= mixer.channels() {
            Color::White
        } else {
            LANE_COLORS[strip % LANE_COLORS.len()]
        };
        let muted = mixer.muted(strip);
        let soloed = mixer.soloed(strip);
        let selected_here = strip == selected;

        // Name row (with mute/solo flag), highlighted when this strip is selected.
        let flag = if muted {
            'M'
        } else if soloed {
            'S'
        } else {
            ' '
        };
        let name_style = if selected_here {
            Style::new()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD)
        } else if muted {
            Style::new().fg(Color::DarkGray)
        } else if soloed {
            Style::new().fg(Color::LightYellow)
        } else {
            Style::new().fg(color)
        };
        frame.render_widget(
            ratatui::widgets::Paragraph::new(format!("{}{flag}", mixer.name(strip)))
                .style(name_style),
            Rect::new(x, inner.y, width, 1),
        );

        // Fader: a track, a bright handle at the value, and the set part below.
        let value = mixer.value(strip);
        let track = fader.height;
        let handle_row = if track <= 1 {
            fader.y
        } else {
            fader.y + ((1.0 - value) * (track - 1) as f32).round() as u16
        };
        let fader_style = Style::new().fg(if muted { Color::DarkGray } else { color });
        for row in 0..track {
            let y = fader.y + row;
            let cell = &mut frame.buffer_mut()[(x, y)];
            if y == handle_row {
                cell.set_char('█');
                cell.set_style(
                    Style::new()
                        .fg(if selected_here { Color::White } else { color })
                        .add_modifier(Modifier::BOLD),
                );
            } else if y < handle_row {
                cell.set_char('░');
                cell.set_style(Style::new().fg(Color::DarkGray));
            } else {
                cell.set_char('│');
                cell.set_style(fader_style);
            }
        }

        // Meter: the engine's level, from the bottom up, coloured by height.
        let level = if strip >= mixer.channels() {
            master_level
        } else {
            levels.get(strip).copied().unwrap_or(0.0)
        };
        let filled = ((level.clamp(0.0, 1.0) * track as f32).round() as u16).min(track);
        for row in 0..filled {
            let y = fader.y + track - 1 - row;
            let cell = &mut frame.buffer_mut()[(x + 1, y)];
            cell.set_char('█');
            let color = if level > 0.9 {
                Color::LightRed
            } else if level > 0.7 {
                Color::LightYellow
            } else {
                Color::LightGreen
            };
            cell.set_style(Style::new().fg(if muted { Color::DarkGray } else { color }));
        }

        // Value row (wide strips only).
        if width == STRIP_WIDE {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(format!("{value:.2}")).style(Style::new().fg(
                    if selected_here {
                        Color::White
                    } else {
                        Color::Gray
                    },
                )),
                Rect::new(x, inner.y + inner.height - 1, width, 1),
            );
        }
    }

    drawn
}

/// Same palette as the timeline's lanes, so a channel and its track read alike.
const LANE_COLORS: [Color; 6] = [
    Color::LightGreen,
    Color::LightBlue,
    Color::LightYellow,
    Color::LightMagenta,
    Color::LightCyan,
    Color::LightRed,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_map_rows_to_values_top_down() {
        let strips = Strips {
            outer: Rect::new(0, 0, 20, 10),
            strips: vec![
                (0, Rect::new(0, 0, 5, 10), Rect::new(0, 1, 1, 8)),
                (1, Rect::new(6, 0, 5, 10), Rect::new(6, 1, 1, 8)),
            ],
        };

        // The top of the fader column is 1.0, the bottom 0.0.
        let (strip, top) = strips
            .at(ratatui::layout::Position::new(0, 1))
            .expect("the fader column");
        assert_eq!(strip, 0);
        assert!((top - 1.0).abs() < 1e-6, "top = {top}");

        let (_, bottom) = strips
            .at(ratatui::layout::Position::new(0, 8))
            .expect("the fader column");
        assert!(bottom.abs() < 1e-6, "bottom = {bottom}");

        // The second strip is its own hit region.
        let (strip, _) = strips
            .at(ratatui::layout::Position::new(6, 4))
            .expect("the second strip");
        assert_eq!(strip, 1);
    }

    #[test]
    fn values_clamp_and_the_master_is_its_own_strip() {
        let mut mixer = Mixer::new(2);
        mixer.nudge(0, 5.0);
        assert_eq!(mixer.value(0), 1.0, "clamped at the top");
        mixer.nudge(0, -5.0);
        assert_eq!(mixer.value(0), 0.0, "clamped at the bottom");

        mixer.set_value(2, 0.5); // the master strip
        assert_eq!(mixer.master, 0.5);
        assert_eq!(mixer.gain_param(2), "master.gain");
        assert_eq!(mixer.gain_param(1), "ch1.gain");

        // Mute/solo exist for channels only.
        mixer.toggle_mute(1);
        assert!(mixer.muted(1));
        mixer.toggle_mute(2);
        assert!(!mixer.muted(2), "the master has no mute");
    }

    #[test]
    fn resize_keeps_what_was_set() {
        let mut mixer = Mixer::new(2);
        mixer.set_value(0, 0.25);
        mixer.toggle_mute(1);
        mixer.resize(4);

        assert_eq!(mixer.channels(), 4);
        assert_eq!(mixer.value(0), 0.25, "existing gains survive");
        assert!(mixer.muted(1), "existing mutes survive");
        assert_eq!(mixer.value(3), 1.0, "new channels start at unity");
    }
}
