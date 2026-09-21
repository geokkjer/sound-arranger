# Agent Note: the dev toolchain is rustup-managed, not the distro's `rust`

Status: implemented

## Problem

Dev builds ran on the Arch `rust` package — the mechanism the
[native host toolchain note](2026-09-10-native-host-dev-toolchain.md) chose when
the Nix/devenv shell was parked. That was the right call for *where* the toolchain
comes from, but the package is a poor *source* of one:

- **It is unpinnable.** The version moves with `pacman -Syu` and cannot be held
  back per project; the project's compiler is a property of the machine, not of
  the repo. Nothing in the tree declared which rustc the code was expected to
  build under.
- **It cannot coexist with rustup.** The package declares
  `Conflicts With: rust cargo rustfmt` — so as long as `rust` was installed,
  rustup was not an option at all.
- **Components are not a toolchain decision.** `rustfmt`/`clippy` happen to come
  from the package, but `rust-analyzer` was not installed on this host at all,
  and component versions could drift from the compiler's.
- **The compiler became a variable in an experiment.** Taking
  [iced](https://github.com/iced-rs/iced) seriously for the shell means the
  toolchain matters: iced 0.14 requires rustc ≥ 1.88 and iced `master`
  (0.15.0-dev) requires ≥ 1.93. A shell evaluation should measure iced, not
  whatever rustc a distro upgrade happened to leave behind — and if a pin is ever
  needed, it must be expressible in the repo.

## Decision

**rustup owns the toolchain; the repo declares it.** Concretely:

- `rustup` 1.29.1 is installed from the Arch package, which **replaced** the
  `rust` package (the package `Provides: rust cargo rustfmt` and conflicts with
  them, so there is exactly one toolchain provider on the machine).
  `rustup default stable` installed `stable-x86_64-unknown-linux-gnu`
  (rustc 1.98.1), and `rustup component add rust-analyzer` completed the dev set.
- **`rust-toolchain.toml` at the repo root declares the toolchain**:
  `channel = "stable"`, `components = ["rustfmt", "clippy", "rust-analyzer"]`,
  `profile = "default"`. rustup installs missing components on first use, so a
  fresh machine needs only `rustup` itself.
- **The channel is `stable`, not a version pin.** This project tracks a rolling
  target and `rustup update` is the intended way to move; the file's job is to
  make the toolchain *declared and per-repo*, which the distro package could not
  do. A hard pin is a one-line change the moment something needs one.
- `/usr/bin/{rustc,cargo,rustfmt,clippy,rust-analyzer}` are now **rustup shims**,
  so no `PATH` edit is required for the toolchain to resolve; toolchains live in
  `~/.rustup`, cargo home stays `~/.cargo`. Verified: `rustup show` reports
  `stable-x86_64-unknown-linux-gnu (active, default)` and `cargo test --workspace`
  passes under it.
- The **Nix/devenv config stays parked** under `nix/` for a future reproducible
  build — this note changes only who provides the host toolchain.

## Alternatives considered

- **Keep the Arch `rust` package.** Superseded: unpinnable, conflicts with
  rustup, no `rust-analyzer`, and the compiler stays an undocumented property of
  the machine. It was right when the only goal was "a host build that opens a
  window"; it is wrong once the toolchain is something the project reasons about.
- **Pin an exact version (`channel = "1.98.1"`).** Kept in reserve, not taken:
  nothing here needs a pin (no nightly features, edition 2024 is stable, no MSRV
  gate in CI), and a pin means every contributor pays a download for a version
  bump. Switching is a one-line edit when a dependency's MSRV or a CI build
  demands one.
- **`rustup-init` (the official installer) instead of the distro package.** Works
  without root, but it leaves the distro `rust` package installed and creates two
  toolchain providers whose shadows depend on `PATH` order — the exact
  ambiguity the move was meant to remove. Rejected for the one-provider route.
- **Nightly via rustup.** No unstable feature is used; nightly would add churn
  for nothing.
- **Un-park the Nix/devenv shell.** Still blocked on the measured non-NixOS
  failure (no EGL vendor in the Nix closure; host WebKit wants GLIBC 2.44 against
  the Nix toolchain's 2.42) — see the
  [host toolchain note](2026-09-10-native-host-dev-toolchain.md).

## Consequences

- The build is `cargo` → rustup shim → `~/.rustup/toolchains/stable-…`. `rustup
  show` is the one command that answers "which compiler is this repo building
  with"; the answer no longer depends on the distro's package state.
- `rust-toolchain.toml` applies to the **whole tree by directory walk**, including
  the nested `spikes/iced-shell` workspace — one declaration covers the core and
  the evaluation.
- rustfmt, clippy, and rust-analyzer are now version-matched to the compiler and
  installed by rustup rather than by pacman.
- One-time cost: the toolchain download into `~/.rustup`, and the distro `rust`
  package is gone — `pacman -S rust` would now conflict with `rustup`.
- Reproducibility is still **not** version-pinned; that remains a deliberate
  deferral (the packaging story owns it), now with a place to express it.
- This supersedes the *toolchain mechanism* half of the
  [native host dev toolchain note](2026-09-10-native-host-dev-toolchain.md), which
  now cross-links here; its Nix diagnosis and its "no Nix in the dev loop"
  decision are unchanged.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-21.
