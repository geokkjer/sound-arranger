# RETIRED — the Tauri v2 + Vue 3 shell

**Status: retired 2026-09-22; the approach was tried and decided against.** No new
work goes here, and nothing should be ported *from* here on principle. The shells
are [**iced**](../../spikes/iced-shell/) (GUI) and [**ratatui**](../../spikes/tui-shell/)
(TUI), both in-process Rust over the same Host API. The decision and the reasoning
are in [the shells note](../../.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md);
what the rejection means for the docs, and the forward plan (iced carries the
recorder profile once the recorder is good enough), are in
[the 2026-09-30 framing note](../../.agents/notes/implemented/architecture/2026-09-30-tauri-was-an-experiment-and-iced-is-next.md).

It was built, it worked, and it proved the UI-as-plugin contract end to end — that
is why the cost it carried is a settled question rather than an open one: Vue
beside Rust, a webview, an IPC wire, a serde bridge, a `!Send` session behind a
mutex, an npm toolchain in the build, and webkit/GPU on the deploy path. The Rust
shells exist to remove exactly that, so this is history, not a reference.

What "retired" means concretely:

- **Out of the default build and test path.** `crates/shell/src-tauri` is excluded
  from the root workspace (`Cargo.toml`), so `cargo build --workspace`,
  `cargo test --workspace` and clippy no longer touch Tauri, wgpu/webkit or the
  Vue toolchain. `pnpm install && pnpm test` here are not run either.
- **Kept only until someone deletes it.** The directory survives because deleting
  it is not this decision — `git log` has the whole thing either way. It still
  builds standalone if someone needs to dig a fact out of it:

  ```sh
  cd crates/shell/src-tauri && cargo check   # its own workspace (see Cargo.toml)
  cd crates/shell && pnpm install && pnpm test
  ```

- **Frozen.** A change to `crates/host`'s API no longer has to keep this shell
  compiling. Deleting the directory is a one-command follow-up whenever the owner
  prefers the tree without it.
