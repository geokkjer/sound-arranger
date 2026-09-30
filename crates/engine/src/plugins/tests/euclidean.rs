use super::*;

#[test]
fn euclid_is_maximally_even() {
    for (steps, pulses) in [(8, 3), (8, 5), (16, 5), (12, 4), (7, 2), (16, 7)] {
        let pattern = euclid(steps, pulses, 0);
        assert_eq!(pattern.len(), steps as usize);
        assert_eq!(pattern.iter().filter(|p| **p).count(), pulses as usize);
        let positions: Vec<usize> = pattern
            .iter()
            .enumerate()
            .filter(|(_, p)| **p)
            .map(|(i, _)| i)
            .collect();
        let mut gaps: Vec<usize> = positions.windows(2).map(|w| w[1] - w[0]).collect();
        gaps.push(positions[0] + steps as usize - positions[positions.len() - 1]);
        let min = *gaps.iter().min().unwrap();
        let max = *gaps.iter().max().unwrap();
        assert!(
            max - min <= 1,
            "E({pulses},{steps}) gaps {gaps:?} not maximally even"
        );
    }
}

#[test]
fn euclid_rotation_shifts() {
    let base = euclid(8, 3, 0);
    let rot = euclid(8, 3, 2);
    let shifted = base
        .iter()
        .cycle()
        .skip(6)
        .take(8)
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(rot, shifted);
}

#[test]
fn euclid_edge_cases() {
    assert_eq!(euclid(0, 3, 0), vec![false]);
    assert_eq!(euclid(8, 0, 0), vec![false; 8]);
    assert_eq!(euclid(8, 99, 0), vec![true; 8]);
    assert_eq!(euclid(4, 4, 0), vec![true; 4]);
}

/// `steps` is one `bool` per step and the factory's `as u32` **saturates**,
/// so a mount param could ask for a ~4 GB pattern: allocated, cloned and
/// retained three times over, on the render thread, from a 30-byte script
/// line. The bound is one named constant, shared with the node, and the
/// refusal names it.
#[test]
fn the_factory_refuses_an_absurd_step_count() {
    let err = euclidean_factory(&[("steps", 4_294_967_295.0)])
        .err()
        .expect("a saturated `steps` is not a rhythm");
    assert!(err.contains("4294967295"), "it names the value: {err}");
    assert!(
        err.contains(&EUCLIDEAN_MAX_STEPS.to_string()),
        "and the limit, so the user knows what to type instead: {err}"
    );
}

/// The bound is a limit, not a hole in the surface: the largest allowed
/// pattern still mounts, so the refusal is about size and nothing else.
#[test]
fn the_factory_still_builds_at_the_limit() {
    for steps in [8.0, 1024.0, EUCLIDEAN_MAX_STEPS as f32] {
        let plugin = euclidean_factory(&[("steps", steps)])
            .unwrap_or_else(|e| panic!("{steps} steps must mount: {e}"));
        assert_eq!(plugin.id(), "euclidean");
    }
}

/// The allocation site refuses too, so a caller who reaches `euclid` without
/// the factory's door cannot ask it for gigabytes either.
#[test]
#[should_panic(expected = "over the 4096-step limit")]
fn euclid_refuses_an_absurd_step_count() {
    euclid(EUCLIDEAN_MAX_STEPS + 1, 3, 0);
}
