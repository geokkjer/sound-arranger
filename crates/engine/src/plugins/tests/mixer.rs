use super::*;

#[test]
fn param_namespace() {
    let mut m = MixerNode::new();
    m.set_param("ch1.gain", 0.5);
    m.set_param("ch0.mute", 1.0);
    m.set_param("ch2.solo", 1.0);
    m.set_param("master.gain", 0.25);
    assert_eq!(m.gains[1], 0.5);
    assert!(m.mutes[0]);
    assert!(m.solos[2]);
    assert_eq!(m.master_gain, 0.25);
}

/// The only bound left is a **sanity** one, and it is not a design ceiling: nine
/// channels is a valid mount (it used to be refused), and so is anything up to the
/// bound. What the bound protects is gross typos, not width.
#[test]
fn factory_accepts_any_sane_width_and_refuses_the_absurd() {
    assert!(mixer_factory(&[("channels", 0.0)]).is_err());
    assert!(mixer_factory(&[("channels", 4.0)]).is_ok());
    for wide in [9.0, 24.0, MIXER_CHANNELS_SANITY as f32] {
        assert!(
            mixer_factory(&[("channels", wide)]).is_ok(),
            "{wide} channels is a valid mount now"
        );
    }
    assert!(mixer_factory(&[("channels", (MIXER_CHANNELS_SANITY + 1) as f32)]).is_err());
    // A fractional count is still refused: channels are lanes, not a knob.
    assert!(mixer_factory(&[("channels", 2.5)]).is_err());
}

/// The mounted surface follows the mount, not a constant: a 12-channel mixer offers
/// twelve channel ports and twelve channels' worth of parameters.
#[test]
fn the_mounted_surface_follows_the_mount() {
    let plugin = mixer_factory(&[("channels", 12.0)]).expect("a wide mixer mounts");
    let ports = plugin.mounted_ports();
    let params = plugin.mounted_params();
    assert_eq!(
        ports.iter().filter(|p| p.name.starts_with("ch")).count(),
        12,
        "one channel input per mounted channel"
    );
    assert!(
        ports.iter().any(|p| p.name == "audio"),
        "the master out is there"
    );
    assert!(
        params.iter().any(|p| p.name == "ch11.gain"),
        "the twelfth channel has params"
    );
    assert!(
        !params.iter().any(|p| p.name == "ch12.gain"),
        "and no thirteenth"
    );
    // The catalog stays nominal, so the patch bay has something to offer before a
    // mount exists — but it is not what the instance answers with.
    assert_ne!(ports.len(), MIXER_PORTS.len());
}
