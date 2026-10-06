# Research: the CDP WASM Suite — Oli Larkin's CDP8→WASM port, explored hands-on

> **Date:** 2026-10-06. **Scope:** [cdp-wasm-suite](https://cdp-wasm-suite.github.io) — the WASM port of the Composer's Desktop Project ([CDP](https://www.composersdesktop.com)) published as the npm engine **`cdp-wasm`**, plus its apps ([cdp-web](https://cdp-web.app), DAW plug-in, Ableton extension) and its agent skills — evaluated as a *distribution of CDP8* for this platform's deferred **sound-sculptor** profile. **Method:** desk research (landing page, engine/app/skill READMEs, npm registry metadata) plus a hands-on run: `cdp-wasm@0.7.0` installed under Node 24.19.0 in a scratch dir, driven through both the `cdp` CLI and the typed library API, timings taken on this machine (trivial sine material — treat as order-of-magnitude, not benchmarks). **Status:** research, not a decision — it feeds the [no-sidecars / external-programs seam](../../.agents/notes/proposed/architecture/2026-09-21-external-programs-not-sidecars.md) in [§6](../../RESEARCH.md) and moves nothing that is locked.

## 0. Verdict on one screen

| Question | Answer |
|---|---|
| Is this real CDP or a reimplementation? | **The same code.** The CDP8 C sources, compiled to WASM via Emscripten (a pinned submodule of a [fork](https://github.com/cdp-wasm-suite/CDP8) of [ComposersDesktop/CDP8](https://github.com/ComposersDesktop/CDP8), portability fixes proposed upstream). Same programs, arguments, file formats; deterministic effects are **parity-tested bit-for-bit against the native binaries** (`npm run test:parity`, in CI). |
| How much of CDP ships? | **215 program modules** (197 side modules + 18 self-contained externals) on one shared `cdp-core`; a curated typed catalog of **232 effects across 110 programs + 16 synthesis generators**, each param with `min/max/default/step/help`. |
| Is it usable? | Yes, today. Public npm package (Node ≥ 18, MIT wrapper + LGPL-2.1-or-later modules), 3.22 MB tarball / 11 MB installed, **zero toolchain** — no CMake, no Emscripten, no native CDP install. `cdp doctor` green on the first try here. |
| Is it fast? | Fast enough to feel interactive at clip scale: a 30 s 48 kHz file took **0.28 s** (`modify speed`) and **0.75 s** (full `pvoc anal → blur → synth` chain) here. WASM is still slower than native, and one run is capped at **4 GB** (wasm32, everything staged in an in-memory FS). |
| Does it change our locked plans? | **No.** The locked seam is "invoke external programs on rendered sources" — `cdp-wasm` is the strongest *distribution* for that seam (prebuilt, byte-in/byte-out, catalog metadata included), not a new architecture. Native CDP8 remains the no-Node / max-performance path. |
| What is worth stealing regardless of engine choice? | **The typed catalog** — it *is* the [macro-knob map](2026-09-09-ableton-prior-art-ui.md) (someone else already curated 232 effects with ranges and help text, MIT data); the pvoc auto-bracketing / per-channel split-recombine plumbing; the parity-test methodology; and the agent-skill surface design. |
| Blockers? | Project age (first real release ~2026-08-01, 0.7.0 now — ten-plus releases in two months, single maintainer, a [KVR Developer Challenge 2026](https://www.kvraudio.com/kvr-developer-challenge/2026/) entry); the apps (plug-in/extension) are still "coming soon"; AGPL on cdp-web (irrelevant to us — we don't embed it). |

## 1. The suite map

One org, four products around one engine ([landing page](https://cdp-wasm-suite.github.io)):

| Part | What | Status / license |
|---|---|---|
| **`cdp-wasm`** ([repo](https://github.com/cdp-wasm-suite/cdp-wasm), [npm](https://www.npmjs.com/package/cdp-wasm)) | The engine: every CDP program as a `.wasm` side module on a shared `cdp-core`, plus a typed JS API (`CDP.run/process`, `EFFECTS`/`applyEffect`, `GENERATORS`/`applyGenerator`, analysis helpers) and a **`cdp` CLI that takes the same arguments as native CDP** | Live, npm 0.7.0. Wrapper MIT; `.wasm` modules LGPL-2.1-or-later (© Trevor Wishart, Richard Dobson, Martin Atkins, CDP Ltd). |
| **cdp-web** ([repo](https://github.com/cdp-wasm-suite/cdp-web), [cdp-web.app](https://cdp-web.app)) | Retro-computing CRT node-graph front-end; PWA; **patch-as-link sharing** (the whole patch is packed into the URL); embeds the [Faust compiler](https://github.com/grame-cncm/faustwasm) for custom DSP nodes | Live. **AGPL-3.0-or-later** — not a candidate for embedding here (our shells are ratatui/iced anyway; AGPL §13 network clause). |
| **cdp-plugin** ([repo](https://github.com/cdp-wasm-suite/cdp-plugin)) | VST3/AU/CLAP *instrument* embedding the cdp-web environment: renders WAVs, then plays them **polyphonically across the keyboard** | "Coming soon" (the landing page's own alert) — pre-release. |
| **cdp-extension** ([repo](https://github.com/cdp-wasm-suite/cdp-extension)) | Ableton Live extension: offline process from the session/arrangement, results written back in | "Coming soon" — pre-release. |
| **Agent skills** ([dir](https://github.com/cdp-wasm-suite/cdp-wasm/tree/main/plugins/cdp-sound-design)) | Three skills (`cdp-sound-design`, `build-cdp-web-patches`, `drive-cdp-web`) installable into Codex/Claude Code/Cursor or any `npx skills` client; portable [Agent Plugins](https://github.com/agentplugins/agent-plugins-spec) 1.0.0 manifest | Live in the repo; the engine-side skill works against loose files today. |

Author: **Oli Larkin** (iPlug2, well-known in the audio-dev community) — this is his KVR DC 2026 entry. CDP itself: developed since the late 1980s, out of York (Trevor Wishart, Richard Dobson and [many others](https://www.composersdesktop.com/about.html)).

## 2. The engine, layer by layer

The package is a small stack, each layer usable on its own ([README](https://github.com/cdp-wasm-suite/cdp-wasm)):

```
applyEffect(cdp, effect, values, wav)   typed catalog — named params, pvoc + channel handling
  └─ cdp.process(program, args, bytes)   byte interface — $IN/$OUT virtual paths, one in / one out
       └─ cdp.run(program, argv)         in-memory FS staging + callMain(argv)
            └─ blur.wasm on cdp-core     a CDP program, same arguments/formats as native
```

- **Build shape:** Emscripten `-sMODULARIZE -sEXPORT_ES6`, `callMain` with `INVOKE_RUN=0`/`EXIT_RUNTIME=0` so the in-memory filesystem stays readable after the program returns. Shared `cdp-core` linked `MAIN_MODULE=2` (dead-code-eliminated against the side modules); per-run instantiation "a few milliseconds". A hand-written **SIMD128 FFT** replaces the Singleton FFT in the pvoc path, with **runtime detection** and bundled `.scalar` fallbacks for engines without SIMD128 (old webviews, pre-16.4 Safari/Node).
- **The typed catalog:** each `EFFECTS` entry is *one mode of one program* — `id`, `label`, `category`, `program`, domain flags (spectral/mono/spatial), the **argument template** (with `$IN`/`$OUT` tokens and `{p: '<param>'}` refs), and `params: [{name, label, min, max, default, step, help}]` — a complete UI-generatable spec. The runner (`applyEffect`) brackets spectral effects in `pvoc anal → effect → pvoc synth` automatically, splits/recombines channels for mono-only programs, handles two-input effects and breakpoint automation. A `docs/guide/catalog-and-cdp.md` explains what each entry decides on your behalf and when to drop to raw `cdp.run`.
- **Data helpers:** `extractEnvelope` (→ `[[time, value], …]` breakpoint arrays), `warpBreakpoints`/`parseBreakpoints`/`formatBreakpoints`, `getPitch` (pitch contour), `findPeaks` (transient times) — plus a survey of bundled data-producing programs (onset, partials, spectral and MIDI analysis). Envelope-accepting params are declared in an `ENVELOPE_PARAMS` map.
- **CLI:** `cdp <program> [args]` with file staging (audio/analysis extensions become files; outputs saved back to disk), `--pvoc` for one-shot spectral bracketing, `cdp help <program> [mode]` printing **the native usage text plus a link to the online CDP reference**, `cdp list [--spectral]`, `cdp doctor`. **120 generated man pages ship in the tarball.**
- **Formats:** WAV and AIFF/AIFF-C in (PCM 8/16/24/32, `sowt`/`fl32`/`fl64`), 32-bit float WAV out.
- **License mechanics:** the wrapper drives the programs at arm's length (argv + in-memory FS), so using the package does not make an app a derivative — but redistributing the `.wasm` modules brings LGPL-2.1 terms (keep the notice, keep the modules swappable). The loader's `baseUrl` option exists precisely so modules can be hosted outside the app bundle and user-swapped. Under our GPL-3.0-or-later app this is the comfortable side of LGPL (LGPL-2.1-or-later upgrades into GPLv3; §12 of [RESEARCH.md](../../RESEARCH.md)).
- **Limits:** wasm32 — one `run()` holds input + output + working buffers under **4 GB**; no streaming. Rule of thumb from the README: ~10 MB/min mono float WAV, so ~1 GB ≈ 100 min mono / 50 min stereo; spectral chains reach the ceiling sooner (analysis files are several × the source). For clip-scale sculptor work this is a non-issue.

## 3. Hands-on observations (2026-10-06, this machine)

Node 24.19.0, scratch install, trivial sine material — order-of-magnitude only:

| Step | Result |
|---|---|
| `npm install cdp-wasm` | 3.22 MB tarball → **11 MB** installed, **242 files** in `wasm/`, no peer deps |
| `cdp doctor` | green: node ok, **wasm ok (215 programs, 26 spectral)**, catalog ok (**232 effects, 16 generators**) |
| catalog histogram | Spectral 44 · Extend & segment 35 · Waveset distortion 26 · Spatialisation 19 · Filter & dynamics 15 · Granular 12 · Envelope 12 · Pitch & time 9 · Spectral pitch 11 · Combine 11 · Texture 8 · Delay & reverb 7 · Pitch-sync grains 5 · Morph 5 · Synthesis 4 · Mix 3 · Formants 3 · Pitch-data 3 (= 232) |
| `synth wave` 30 s mono 48 kHz | **0.26 s** wall |
| `modify speed 2 … -12` (60 s output) | **0.28 s** |
| `cdp --pvoc blur blur … 10` (full anal→blur→synth, 30 s) | **0.75 s** |
| `applyEffect('stretch.time', {factor: 4})` on 10 s (→ 40 s out) | **0.49 s** |
| `applyEffect('blur.blur', {windows: 40})` on the 40 s result | **0.77 s** |
| `getPitch` on the sine | `[]` — exactly as documented ("needs harmonic material; a pure sine returns []") |
| `findPeaks` / `extractEnvelope` | 1 peak / 3 points, sane for a constant sine |

Gotchas confirmed by hitting them:

- **`stretch` is invoked as `stretch NAME (spectrum|time) …`** — my raw CLI guess (`stretch stretch …`) failed with CDP's own diagnostic, while the catalog entry `stretch.time` (which encodes the mode choice in its arg template) just worked. That is the catalog doing its job, and a good demonstration of *why* a curated layer on top of raw CDP argv is worth having.
- The native progress tickers print through (`0 min 5.46 sec …`) — an artifact of CDP's stdout, not a timing; the wall-clock numbers above are what matter.

## 4. The agentic surface — the "perfect for agentic sound design" claim, examined

It is fair. Three skills, all shipped in the engine repo ([plugin README](https://github.com/cdp-wasm-suite/cdp-wasm/blob/main/plugins/cdp-sound-design/README.md)):

- **`cdp-sound-design`** — the core one. Its [SKILL.md](https://github.com/cdp-wasm-suite/cdp-wasm/blob/main/plugins/cdp-sound-design/skills/cdp-sound-design/SKILL.md) is a well-designed agent surface: a `check-env` bootstrap that *locates* the library (project-local > global > repo checkout) and prints the exact import line; discovery scripts (`list-effects`, `describe-effect` — "describe every effect you're about to use: param meanings are often not what the name suggests"); a `render-chain.mjs` for linear chains from JSON; cookbook references for chains/breakpoints/analysis/raw-programs/batch-jobs; an explicit gotchas section (multiout effects return `{outputs, names}`; spectral wrapping; CDP prints diagnostics to stdout; **"the user must listen to judge them; you cannot"**); and a rendering convention (`./cdp-renders/<job-slug>/…`, print every path). Orchestration model: *write small Node ESM scripts against the library* rather than a fixed tool surface — code is the tool.
- **`build-cdp-web-patches`** — emits editable `.cdp` graph files (the *user-facing* format, as opposed to the internal chain spec), auto-inserts the explicit PVOC nodes cdp-web needs, and produces compressed open-in-app links.
- **`drive-cdp-web`** — drives a *running* cdp-web in Chrome through the app's **WebMCP** tools (14 tools on `document.modelContext`, [webmachinelearning/webmcp](https://github.com/webmachinelearning/webmcp); behind `chrome://flags/#enable-web-mcp-testing` in Chrome 146+, origin trial from 149) using [chrome-devtools-mcp](https://github.com/ChromeDevTools/chrome-devtools-mcp) as transport; a `window.__webmcp` debug surface serves external MCP clients. Tools return patch structure and audio *metadata* only — never bytes.

For us: the *skill* pattern (bootstrap → discover → script-orchestrate → render-convention → listen gate) is the model for any future sculptor agent surface in this repo, and it works today against loose files with any `npx skills`-compatible agent.

## 5. Relevance to this platform

1. **The sculptor seam.** The locked direction since 2026-09-21 is *invoke external programs* on rendered sources and import the result as a new pool source ([no-sidecars note](../../.agents/notes/proposed/architecture/2026-09-21-external-programs-not-sidecars.md), [§6](../../RESEARCH.md)). `cdp-wasm` is the strongest distribution of CDP8 for that seam: **prebuilt** (kills the "CDP8 sidecar adds a C/CMake toolchain step to the build matrix" trade-off recorded in §13), byte-array in/out (no temp-file ceremony needed if we ever go in-process via Node), and a `cdp` CLI **argument-identical to native CDP** — meaning the seam we build can invoke CDP through either distribution interchangeably (native binary where it's installed, npm where it isn't). The 4 GB/run ceiling is irrelevant at clip scale.
2. **The macro-knob map already exists.** The Ableton prior-art pass concluded we need "a curated set of CDP params rather than ~800 programs" ([research](2026-09-09-ableton-prior-art-ui.md), [note](../../.agents/notes/proposed/architecture/2026-09-09-ui-revision-project-library-and-clip-model.md)). This catalog *is* that curation, done by someone else, MIT-licensed data: 232 effects, each param with range/default/step/help, domain flags, and a blurb. A sculptor UI could be **generated** from it — and the same schema could be applied to a native CDP8 distribution if we ever prefer the binaries.
3. **The plumbing answers are already solved and copyable** (MIT): pvoc auto-bracketing, per-channel split/recombine for the mono-only classics, breakpoint envelopes as `[[t, v]]` values feeding `envelope: true` params, two-input and multiout effect contracts. Even on a native path, reimplementing any of this from scratch would be pure waste.
4. **Parity testing as a methodology.** Deterministic effects match native bit-for-bit, checked in CI; RNG/platform-dependent ones are exempted *explicitly* rather than silently. That is exactly the shape of an integration test for *any* external-program seam we build.
5. **Faust, again.** cdp-web embeds `@grame/faustwasm` (LGPL) for user-authored DSP nodes — the same direction as our [Faust evaluation](2026-09-20-faust-libfaust-and-hise-evaluation.md) but JIT-in-browser rather than AOT-to-Rust in-engine. Their choice validates the *language* as the user/agent-facing DSP surface; our AOT plan remains the better fit for the realtime path we care about.
6. **What not to adopt:** AGPL cdp-web (embedding would drag §13 into a desktop product we don't need — and our shells are ratatui/iced); a Node runtime inside the Rust engine (the external-programs seam keeps Node outside the process boundary, which is the whole point of the seam).

## 6. Risks and open questions

- **Project age and bus factor.** First real release ~2026-08-01 → 0.7.0 at exploration time; ten-plus releases in two months, one principal author, KVR DC entry (support is bug reports + Patreon). The engine is the stable-looking part (it *is* CDP8, plus a thin wrapper); the wrapper and catalog are where churn lives. Pin the version if we depend on it.
- **WASM vs native performance on real material.** My numbers are trivial sines; native CDP8 remains faster, and long spectral chains hit the 4 GB ceiling sooner. If the sculptor profile ends up doing hour-scale batch work, the native distribution wins.
- **The apps are pre-release.** Plug-in and extension are "coming soon"; cdp-web is live but browser-variable on big files. None of this affects the engine path.
- **Open:** would our seam prefer (a) `npm i -g cdp-wasm` + the `cdp` CLI (file staging, argument-identical to native — seam-identical too), or (b) a Node sidecar script against the library API (byte in/out, catalog metadata reachable, `applyEffect` plumbing free)? (a) is the no-code answer and matches the locked seam exactly; (b) is where the catalog metadata and channel/pvoc plumbing pay off. Decidable when sculptor starts — not now.

## 7. Impact on locked decisions

None. "Embed the C as a sidecar CLI / invoke external programs" stands ([§3](../../RESEARCH.md), [no-sidecars note](../../.agents/notes/proposed/architecture/2026-09-21-external-programs-not-sidecars.md)); LGPL-2.1-or-later CDP8 remains the compliant PaulStretch-sound path ([§12](../../RESEARCH.md)); GPL-2.0-only PaulStretch stays excluded. What this exploration adds is a *distribution candidate* for the CDP8 half of that seam, a ready-made curated parameter layer (MIT), and a reference agent-skill design — all recorded here for the day the sound-sculptor profile opens.
