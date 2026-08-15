# sound-arranger (working title)

An **audio platform where everything is a plugin**: a minimal core — clock · audio graph
interpreter · session event log · context plumbing — with every capability as a plugin, and
the product an **assembled profile**. Rust audio engine (standalone crate, `cpal`) +
Tauri v2 + Vue 3.

> **Working title.** The repo name names the *first profile*, not the platform; a rename
> ("audio" / "sound") is an open question — see RESEARCH.md §14.

**Status: pre-code** — research and decisions only. The direction is locked
([umbrella-first note](.agents/notes/proposed/architecture/2026-08-15-umbrella-first-product-direction.md)):

- **Profile #1 — the clip arranger ("sound-arranger"):** record long live jams, then cut,
  splice, rearrange and mix them into a finished piece. ACID-style clips-as-objects with the
  tape heritage (Macero, dub, musique concrète) as inspiration. Chosen first because it
  stresses the substrate end-to-end: recording, editing, mixing, the realtime path.
- **Deferred, own profile — "sound sculptor":** offline/non-realtime processing (CDP8,
  PaulStretch, phase-vocoder) as `OfflineProcess` plugins.

## Documentation map

- [RESEARCH.md](RESEARCH.md) — working research & architecture (verified crate versions,
  licensing matrix, latency notes, plugin-architecture research)
- [.agents/notes/](.agents/notes/README.md) — decision records (Agent Notes); standing
  orders in [AGENTS.md](AGENTS.md)
- [docs/audio-latency.md](docs/audio-latency.md) — Linux kernel/userspace latency tuning
- [research/](research/) — dated research inputs (gear notes, external reviews)

The surrounding ecosystem (composition-theory corpus, jam rig, sibling projects) is indexed
by the [`~/Projects/music`](../music/README.md) symlink view.

## Dev environment

Nix + devenv (`flake.nix` / `devenv.nix` / `devenv.yaml` / `.envrc`):

```sh
direnv allow        # or: devenv shell
```

One-time git setup — hooks live in-repo:

```sh
git config core.hooksPath .githooks
```

The pre-commit hook verifies the Agent Notes tree and refuses edits under
`.agents/notes/archived/`. It needs `node` from the devenv shell; without node it warns
and skips rather than blocking the commit.

## License

GPL-3.0-or-later ([LICENSE](LICENSE)) — rationale and the dependency compatibility matrix
are in RESEARCH.md §12.
