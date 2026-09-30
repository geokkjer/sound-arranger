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
        ("add_clip", "add_clip t0 c0 s1 0 48000 0 64 64 1.0"),
        ("add_track", "add_track t1"),
        ("rename_track", "rename_track t0 lead"),
        ("remove_track", "remove_track t1"),
        ("move_track", "move_track t0 1"),
        ("reverse", "reverse t0 c0"),
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
fn every_action_has_a_name() {
    // The match in `name` is exhaustive by the compiler; this checks the strings
    // are real (a status line printing "" is a bug you only see at runtime).
    for row in KEYMAP {
        for (_, action) in all_keys(row) {
            assert!(!action.name().trim().is_empty(), "{action:?} has no name");
        }
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
/// The grid cycle is the workflow's, coarser to finer and back to off, so two
/// shells cannot disagree about what `b` does.
#[test]
fn the_grid_cycles_off_bar_beat_half_quarter() {
    let mut grid = Grid::default();
    assert!(!grid.is_on());
    assert_eq!(grid.label(), "off");
    assert_eq!(grid.grid(4), None);

    let mut seen = Vec::new();
    for _ in 0..5 {
        grid.cycle();
        seen.push(grid.label());
    }
    assert_eq!(seen, vec!["bar", "beat", "1/2", "1/4", "off"]);
    assert!(!grid.is_on(), "the cycle returns to off");

    // Armed, it hands out the beat-domain math the shell quantizes through.
    let bar = Grid::new(media::Division::Bar).grid(4).expect("a grid");
    assert_eq!(bar.step_beats(), 4.0);
    assert_eq!(bar.nearest(5.9), 4.0);
    let quarter = Grid::new(media::Division::Quarter).grid(4).expect("a grid");
    assert_eq!(quarter.step_beats(), 0.25);
}

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
