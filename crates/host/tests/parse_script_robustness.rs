//! Parse-script robustness: a truncated/malformed command line must be a clean
//! `Err` (the designed `bad script` path), never an index-out-of-bounds panic —
//! this is the wire schema the shells feed, so it cannot panic.

use host::parse_script;

fn err(text: &str) -> String {
    match parse_script(text) {
        Err(e) => e,
        Ok(_) => panic!("expected a parse error for: {text:?}"),
    }
}

#[test]
fn host_v1_empty_cmd_is_err() {
    assert!(err("host v1\nmount\n").contains("line 2"));
    assert!(err("host v1\nset_param\n").contains("line 2"));
}

#[test]
fn truncated_operands_are_errs_not_panics() {
    // one command per case, each missing a *required* operand (mount params are
    // optional, so `mount tone` is valid and not listed here)
    for line in [
        "mount",
        "set_param",
        "set_param mixer",
        "set_param mixer ch0.gain",
        "set_tempo",
        "set_tempo 120",
        "unmount",
        "play",
        "play /a.wav",
        "splice",
        "splice 4000",
        "splice 4000 /a.wav",
        "record",
        "bounce",
        "bounce 5000",
        "patch",
        "patch euclidean.triggers",
    ] {
        let text = format!("host v1\n{line}\n");
        assert!(
            parse_script(&text).is_err(),
            "line '{line}' must be refused, not panic"
        );
    }
}

#[test]
fn known_bad_operands_are_errs_not_panics() {
    assert!(parse_script("host v1\nmount nope gain=1\n").is_err()); // unknown plugin
    assert!(parse_script("host v1\nset_tempo nope 4\n").is_err()); // bad bpm
    assert!(parse_script("host v1\nplay /a.wav ch8\n").is_err()); // channel out of range
    assert!(parse_script("host v1\nbounce nope /out.wav\n").is_err()); // bad frames
}

#[test]
fn a_valid_script_still_parses() {
    let cmds = parse_script(
        "host v1\n\
         mount mixer channels=4 @0\n\
         set_tempo 120 4 @0\n\
         bounce 1000 /tmp/out.wav\n",
    )
    .expect("a well-formed script parses");
    assert_eq!(cmds.len(), 3);
}
