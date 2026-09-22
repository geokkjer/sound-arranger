# RETIRED — the Tauri v2 + Vue 3 shell

**Status: retired 2026-09-22.** No new work goes here. The shells are
[**iced**](../../spikes/iced-shell/) (GUI) and [**ratatui**](../../spikes/tui-shell/)
(TUI), both in-process Rust over the same Host API. The decision, the reasoning and
the open question — *which of the two is primary* — are in
[the shells note](../../.agents/notes/implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md).

What "retired" means concretely:

- **Out of the default build and test path.** `crates/shell/src-tauri` is excluded
  from the root workspace (`Cargo.toml`), so `cargo build --workspace`,
  `cargo test --workspace` and clippy no longer touch Tauri, wgpu/webkit or the
  Vue toolchain. `pnpm install && pnpm test` here are not run either.
- **Kept, not deleted.** It is the only shell that reached a full editor (four
  views, transport, zoom/pan, clip editing, undo/redo over the bridge) and its
  Vue/`src` is a reference for the ports: the timeline canvas, the mixer layout,
  the transport store. It still builds standalone if a dependency is ever needed
  to dig something out:

  ```sh
  cd crates/shell/src-tauri && cargo check   # its own workspace (see Cargo.toml)
  cd crates/shell && pnpm install && pnpm test
  ```

- **Frozen.** A change to `crates/host`'s API no longer has to keep this shell
  compiling. If that becomes a nuisance, delete the directory — `git log` has it.
