# Agent Note: Host honesty fixes — accurate `Record` refusal and a non-lying `arrangement()`

Status: implemented

## Problem

Two host-surface behaviors misreported state to a future GUI, surfaced by the GLM-5.3-Flash value pass:

1. **`Record`'s refusal was misleading** — `crates/host/src/lib.rs` returned `"recording the input requires a device — the reference host bounces the master instead (see Bounce)"`. Recording is not bouncing (bounce renders the session offline; record is a device-input capture path), and the message presented bouncing as a substitute when the real story is that the device path exists in `media::devices` but is not wired into the host.
2. **`arrangement()` swallowed an editor error** — it did `.and_then(|e| e.snapshot().ok()).unwrap_or_default()`: a poisoned/errored snapshot silently produced an **empty** `Timeline`, so a GUI would draw a blank arrangement instead of surfacing that it could not build one. This violated the codebase's fail-loud-never-lie rule.

## Decision

In `crates/host`, implemented 2026-08-27 (code by GLM-5.3-Flash, [attribution convention](../process/2026-08-27-agent-attribution-convention.md)):

- **`Record` refusal** (lib.rs, `apply` arm, and the `Record` variant doc comment): the message now names the real gap — *"recording requires a device — the device input path exists in `media::devices` (`open_input`) but is not wired into the host; the reference host renders offline via `Bounce`"*. It no longer presents bouncing as a substitute for recording, and the variant comment now reads "Capture device input into a take" (the old "Record the master into the pool" had the same conflation).
- **`arrangement()`** (lib.rs): now `pub fn arrangement(&self) -> Result<media::Timeline, String>`. `None` editor → `Ok(media::Timeline::default())` (the legitimate "nothing built yet" case); `Some(e)` → `e.snapshot()` errors are **propagated**. It never silently returns an empty `Timeline` on a snapshot error.
- **Tests**: the four call sites in `crates/host/tests/arranger_commands.rs` now `.expect(...)` with intent-revealing messages; two new lib tests cover the no-editor path (`arrangement_without_an_editor_is_ok_default` → `Ok(default)`) and the error path (`arrangement_propagates_an_errored_snapshot_instead_of_an_empty_value` → `Err`, induced by a `u64` overflow in `SetClipFade` under `catch_unwind`, gated on `debug_assertions`).

## Alternatives considered

- **Keep `arrangement()` infallible and add a separate `try_arrangement()`** — two accessors to remember, and the buggy path stays reachable by the common one. Rejected: the honest reading of "the arrangement value" must be able to say "I couldn't."
- **Keep the current message but soften it** — still a lie (recording ≠ bouncing). Rejected.
- **Leave as-is and document the poison→empty as intended** — a GUI showing blank instead of error is never intended. Rejected.

## Consequences

- `cargo test -p host` passes (22 tests: 2 new lib + 10 `arranger_commands` + 4 `parse_script_robustness` + 6 `reference_host`); `cargo clippy -p host --all-targets -- -D warnings` is clean.
- The workspace clippy gate is red — but on a **pre-existing, unrelated** issue (`crates/engine/tests/parked_arrangements.rs:31`, `assertions_on_constants`), untouched by this change and tracked separately.
- The error-path test is `#[cfg(debug_assertions)]` because it relies on a debug overflow check; release builds have no reachable poison path through the public API. It is the clever kind of test — sound, but a known intentional trade.
- Implementation attributed to GLM-5.3-Flash via the commit `Assisted-by:` trailer, per the attribution convention.
