# plugins/ — sidecar plugins (placeholder)

The platform's contract is **everything is a plugin**. This directory is where the
product-level *sidecar* plugins live — things that are deliverable artifacts rather than
engine crates:

- **VST3 / CLAP plugin builds** of core capabilities (e.g. the soft mixer, a future
  sampler/arranger node) for use in other DAWs.
- **CDP / offline-process sidecar** (the deferred "sound sculptor" profile: CDP8,
  PaulStretch, Csound offline, phase-vocoder) as `OfflineProcess` plugins.

This is a **placeholder** — no producers yet. Engine-side capabilities live in
`crates/*`; a plugin is a thin wrapper that exposes one to a plug-in API.
