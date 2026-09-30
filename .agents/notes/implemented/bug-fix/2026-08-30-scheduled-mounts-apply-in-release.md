# Agent Note: Scheduled mounts apply in release — `debug_assert!` swallowed `apply_mount`

Status: implemented

## Problem

A **release-only** engine bug: no scheduled mount was ever applied, so the engine rendered silence on every path that needs a mounted node. Root cause: the Mount arm of `Engine::apply_event` (`crates/engine/src/render.rs`) wrapped the apply in a `debug_assert!`:

```rust
SchedEvent::Mount { plugin, params } => {
    debug_assert!(
        self.apply_mount(plugin, &params).is_ok(),
        "scheduled mount must apply (log was validated)"
    );
}
```

`debug_assert!` **never evaluates its argument in release builds**, so `self.apply_mount(...)` was never called in release → the plugin was never mounted → its node was never added to the graph → silence. This predates the native-soft-synth work: in a `--release` build the engine's OWN tests failed (e.g. `mixer_owns_the_master_bus`, `set_param_*`, `remove_middle_node_rewires_cords`, and the tone-chain `chain_trigger_is_sample_accurate`), all with no DSP code involved. Confirmed 2026-08-30 while validating the feature-gated fundsp spike in release: a `panic!` at the top of a plugin's `apply` fired in debug but not in release, proving `apply_mount` was never reached.

## Decision

Apply the scheduled mount in **both** builds — call `self.apply_mount(...)` directly and assert on the result, mirroring the sibling `Patch`/`SetParam`/`Arrangement` arms, which already run in release:

```rust
SchedEvent::Mount { plugin, params } => {
    // `apply_mount` must run in BOTH builds. The previous
    // `debug_assert!(self.apply_mount(...).is_ok(), ...)` form is a *no-op in
    // release* (debug_assert! never evaluates its argument), so no scheduled
    // mount was ever applied there and the engine rendered silence on every
    // mounted path. Now always applied; like the sibling arms, an `apply`
    // failure is a log-order error that is debug-asserted and skipped in
    // release, where replay reproduces the same state.
    if let Err(e) = self.apply_mount(plugin, &params) {
        debug_assert!(false, "scheduled mount must apply (log was validated): {e}");
    }
}
```

After this, `cargo test -p engine --release` fully passes (previously 5 failures) and the fundsp voice sounds in release.

## Alternatives considered

- **`expect` on an apply error** — louder, but violates the core's "don't panic on the release path" discipline (a refused mount is a log-order bug, not a runtime fault) and diverges from the sibling arms, which swallow the (impossible) error. Rejected; assert-in-dev is the established pattern here.
- **Keep the `debug_assert!` form** — this is the bug (no-op in release). Rejected.
- **Apply mounts eagerly at `mount()` instead of scheduling** — changes the event-sourcing model: a mount is a log event applied by the render loop for byte-identical replay (`same log ⇒ same audio`). Eager apply would defeat that. Rejected.

## Consequences

- Release builds now apply scheduled mounts; the entire engine works in release, not just debug.
- Byte-identical replay and determinism are unaffected: the mount applies at the same absolute frame; the only change is that release no longer skips it.
- This is the enabling fix that let the native-soft-synth fundsp spike ([note](../../proposed/architecture/2026-08-30-native-soft-synth-building-blocks.md)) pass in release.
- **Honest caveat** (independent review 2026-08-30): "a scheduled mount must apply" is *not* type-enforced. `validate_mount` dry-runs `inject()` but not `apply`, so a plugin whose `apply` returns `Err` (e.g. an op-handler collision) is a live release-silent path for such a plugin. Dead for today's plugins (mixer/tone/euclidean/scale/fundsp all return `Ok`), but if "must apply" is a hard contract, the fallibility belongs in `validate_mount` (a refused mount is then never logged) — a design change, since `apply` isn't dry-runnable today.
- **Superseded in part (2026-09-29)** — the caveat above predicted this exactly, and the euclidean
  plugin's `apply` is what made it reachable: it re-checks its `steps` bound because its fields are
  public, so it is the first `apply` in the tree that can return `Err`. What this note left as a
  `debug_assert!` is now a **recorded fault** — `Engine::apply_faults()`/`is_degraded()`, bounded and
  loud in every build — and `apply_mount` is a transaction, so a refused mount releases the name it
  had reserved instead of wedging it for the rest of the session. The decision and its reasoning live
  in the [euclidean step-walk note](./2026-09-29-the-euclidean-step-walk-is-bounded-and-counted.md);
  what is *not* decided there is this note's own open question, which stands: whether the fallibility
  belongs in `validate_mount` instead, so a mount a plugin would refuse is never logged at all. The
  engine now *survives* a refusal; it does not yet *prevent* one.
- **The `Patch` arm has followed the same road (2026-09-29)** — a cord the graph refuses
  at apply is a recorded `ApplyFault` too, not a `debug_assert!`, so a release build
  reports a channel it could not feed instead of rendering it silent (the
  [logged-patch note](./2026-09-29-a-logged-patch-is-a-patch-the-graph-will-make.md)).
  That note also answers the *prevent* half for cords: a patch whose order the graph
  will refuse is now refused in `validate_patch` and never logged.

*Authored with deepseek-v4-flash-vision-exp · DeepSeek Harness, 2026-08-30.*
