# Porting Bol Processor BP2/BP3 to a Modern Platform

## Background

**BP2** started as a Mac-only Carbon app (closed-source shareware, later open-sourced). **BP3** (~58K LOC of C) is the current multiplatform evolution: a pure C console engine that communicates via command-line/JSON/file I/O, fronted by a **PHP** web interface running on Apache+MAMP/XAMPP. The PHP frontend wraps the C binary, which reads grammar/text files, produces output (MIDI, Csound scores, text traces), and sends structured data back using text files + JSON.

The project already has nascent **WASM** support (`__BP3_WASM__` guards in several files). Romain Peyrichou is actively porting BP3 to WASM/Emscripten for integration with SuperCollider. The C code is **single-threaded**, uses **Handle-based Mac-style memory** via pointers to pointers, and has zero external UI dependencies—making it an excellent candidate for porting.

## Architecture Overview

```
┌──────────────────────────────────────────────────────────┐
│                  CURRENT BP3 ARCHITECTURE                 │
├──────────────────────────────────────────────────────────┤
│                                                          │
│  ┌────────────────────────────┐   PHP Desktop / Browser │
│  │    PHP Frontend (Vue/JS?)  │◄──────► HTTP Apache     │
│  │    ~477 commits            │                         │
│  └───────────┬────────────────┘                         │
│              │ exec() / file I/O                        │
│  ┌───────────▼────────────────┐                         │
│  │  C Console Engine (BP3)    │                         │
│  │  ~58K LOC, 52 source files │                         │
│  │                            │                         │
│  │  ┌──────┬──────┬────────┐  │                         │
│  │  │Grammar│MIDI  │Csound  │  │                         │
│  │  │Engine │I/O   │Output  │  │                         │
│  │  ├──────┼──────┼────────┤  │                         │
│  │  │Poly-  │Time  │Sound   │  │                         │
│  │  │metric │Set   │Objects │  │                         │
│  │  └──────┴──────┴────────┘  │                         │
│  └────────────────────────────┘                         │
│                                                          │
│  Data files: .gr (grammar), .ho (alphabet), .da (data), │
│  .se (settings), .mi (object prototypes), .wg (weights) │
└──────────────────────────────────────────────────────────┘
```

## Recommended Approach: Hybrid (Tauri + WASM)

The best strategy is a **two-pronged** approach with Tauri as the flagship:

### Primary: Tauri Desktop App (Rust shell + Vue + compiled C)

- **Rust backend** (Tauri) wraps the existing C console engine as a compiled native binary or statically-linked library
- **Vue 3** frontend with full rich UI replacing PHP
- IPC: Tauri commands call the C engine, read/write the project workspace
- MIDI I/O via Rust MIDI crates (e.g., `midir`) or bundled PortMidi
- Csound integration via spawning csound process or libcsound bindings
- Cross-platform: macOS, Windows, Linux from day one

### Secondary: Web App via WASM

- Compile the C engine to WASM via Emscripten (already partially supported)
- Same Vue 3 frontend can be reused for web deployment
- All computation runs in-browser
- MIDI output via Web MIDI API
- Csound via Csound WASM build or replace with Web Audio API synthesis
- This can be served as a static site

---

## Porting Plan — Phased Execution

### Phase 1: Foundation (4-6 weeks)

**Goal:** Get the C engine compiling cleanly, understand all file formats, establish the Tauri skeleton.

| Step | Task | Details |
|------|------|---------|
| 1.1 | **Audit C source** | Merge BP3 C code into a proper build system (CMake or Meson). Remove all Carbon/CodeWarrior cruft. The `source/not_used/` dir already isolates legacy Mac GUI code. |
| 1.2 | **WASM compilation** | Get BP3 compiling to WASM via Emscripten. The `__BP3_WASM__` guards provide a starting point. Need to add a WASM entry point replacing `ConsoleMain.c:main()` with an exported function. |
| 1.3 | **Document file formats** | Reverse-engineer and document all .gr, .ho, .da, .se, .mi, .wg, .tb file formats in a neutral format (JSON schema). The `docs-developer/` files plus `-BP3.h` structs are the source of truth. |
| 1.4 | **Set up Tauri + Vue 3** | Create `bp4-tauri/` repo with Tauri v2 + Vue 3 + TypeScript. Establish IPC protocol, workspace/project file management. |
| 1.5 | **Rust bindings** | Create a Rust crate that communicates with the C binary: spawn as subprocess or FFI-link as static lib. Use `std::process::Command` initially. |

### Phase 2: Core Engine Integration (4-6 weeks)

**Goal:** All BP3 console functionality accessible through the Tauri backend.

| Step | Task | Details |
|------|------|---------|
| 2.1 | **File I/O management** | Tauri manages the user's workspace directory. File picker for grammar/alphabet/data files. Port the file management (currently in PHP's `php/`) to Rust. |
| 2.2 | **Grammar compilation** | Tauri command → call C engine's `CompileGrammar()` → return results (error messages, grammar stats) via JSON. |
| 2.3 | **Item production** | Run `ProduceItems()` → collect produced items. The BP3 console already outputs JSON for produced items. |
| 2.4 | **Time-setting** | Run `TimeSet()` → return phase diagram. The `FillPhaseDiagram.c` and `TimeSetFunctions.c` handle this. |
| 2.5 | **Playback / rendering** | Generate phase table → convert to MIDI events or Csound score. Tauri can write MIDI files via Rust crate or stream to MIDI out. |
| 2.6 | **Score text generation** | BP3 generates text and HTML output natively. We'll capture and display in the frontend. |

### Phase 3: User Interface — Vue Frontend (6-8 weeks)

**Goal:** Full composer UI replacing PHP interface.

| Step | Task | Details |
|------|------|---------|
| 3.1 | **Grammar editor** | Monaco Editor or CodeMirror with syntax highlighting for BP grammar language. Grammar token definitions from `-BP3.h` define bold/italic/color conventions. |
| 3.2 | **Alphabet editor** | Table-based editor for terminals/variables. Show mappings to sound-object prototypes. |
| 3.3 | **Sound-object editor** | Piano-roll / MIDI editor for `.mi` prototype files. Visual editing of note events, controllers, object properties (pivot mode, expand/compress flags, etc.). |
| 3.4 | **Score visualization** | Piano-roll display of the produced phase diagram. Timeline with zoom, pan, note selection. |
| 3.5 | **Polymetric display** | Visualize polymetric expression trees. Show time-object lattice and beat structure. |
| 3.6 | **MIDI setup / transport** | MIDI port selection (Web MIDI / native), transport controls (play/stop), tempo settings. |
| 3.7 | **Csound configuration** | Csound orchestra setup, argument mapping, instrument assignment UI. |
| 3.8 | **Workspace / project management** | File tree browser for the workspace. Open/save projects. Multiple workspaces. |
| 3.9 | **Keyboard mapping** | MIDI keyboard → BP instructions mapping UI. |
| 3.10 | **Real-time interaction** | Interactive derivation mode UI, MIDI capture mode, learning weights, keyboard shortcuts. |

### Phase 4: Audio & MIDI Backend (4-6 weeks)

**Goal:** Real-time audio/MIDI output from the Tauri app.

| Step | Task | Details |
|------|------|---------|
| 4.1 | **Native MIDI output** | Use `midir` Rust crate or wrap PortMidi to send MIDI from the phase table. This replaces the `MIDIdriver.c` + CoreMIDI pathway. |
| 4.2 | **MIDI input** | Capture real-time MIDI input for interactive composition. Route through Rust to the C engine's `Encode()` pipeline. |
| 4.3 | **Csound integration** | Bundle Csound or call `csound` binary as subprocess. Send Csound scores from BP. Optionally use libcsound Rust bindings. |
| 4.4 | **Audio playback** | For the WASM/web target, implement Csound via WASM or replace with Tone.js / Web Audio API synthesis for preview. |

### Phase 5: Polish & Web Deployment (4-8 weeks)

**Goal:** Shipping desktop app + web demo.

| Step | Task | Details |
|------|------|---------|
| 5.1 | **Desktop packaging** | Tauri bundler for macOS (.dmg), Windows (.msi), Linux (.AppImage/Flatpak). Code signing, notarization. |
| 5.2 | **WASM web app** | Build the same Vue frontend to work with WASM BP3 engine in the browser. Static site deployment. |
| 5.3 | **Import/export** | MusicXML import, MIDI file export (already exists in `MIDIfiles.c`), Csound score export. |
| 5.4 | **Tutorial / documentation** | Port the BP3 help system and tutorials to the new UI. |
| 5.5 | **Testing** | Port the `bp3-ctests` example suite. Unit tests for Rust backend. C test suite via CTest. |

---

## Key Technical Decisions

| Decision | Recommendation | Rationale |
|----------|---------------|-----------|
| **Build system** | CMake for C engine + Rust's cargo for Tauri | CMake is portable; Emscripten supports CMake; `cc` crate can link C libs into Rust |
| **C → Rust integration** | Subprocess (spawn C binary) initially, then FFI for performance | Subprocess is low-risk path to MVP; FFI allows deep integration (shared memory for phase diagrams) |
| **WASM entry point** | Single exported function `bp3_process(input_json)` returning `output_json` | Keeps the C engine as a pure computation unit; no need for WASM threads initially |
| **Memory management** | Keep BP3's Handle-based system in C layer; convert on boundary | Too risky to refactor memory model; boundary conversion via JSON serialization |
| **MIDI on WASM** | Web MIDI API | Standard browser API, no extra dependencies |
| **Audio rendering** | MIDI file → Web Audio API (Tone.js) for WASM; native MIDI for desktop | Pragmatic: MIDI is the primary output format; Csound for advanced users |
| **State management** | Pinia (Vue 3 standard) | Well-documented, TypeScript-first, devtools support |
| **UI framework** | Vue 3 + Nuxt UI (or Naive UI) | Tauri already supports Vue; Nuxt UI provides polished components |

## Risk Assessment

| Risk | Mitigation |
|------|-----------|
| **C engine subtleties** ~38 years of edge cases in time-setting algorithms | Port `bp3-ctests` first to establish a regression test suite; compare output character-by-character with BP3 |
| **Handle-based memory** Mac Toolbox-style handles (`**`) in C | Wrap all handle access through accessor macros (already mostly done). No refactoring needed—just don't touch it |
| **MIDI timing accuracy** Real-time scheduling in BP3 is complex | On desktop: use `midir` with high-res timer. In WASM: use AudioContext timing |
| **PHP frontend feature parity** The PHP interface has ~477 commits of UI work | Prioritize based on user surveys. Core composition workflow first; advanced features (like continuous parameter curves) later |
| **Community fragmentation** BP2/BP3 already has multiple forks/approaches | Contribute upstream; align with Bernard Bel & Romain Peyrichou's WASM efforts |

## Files to Port / Reference in the C Engine

| C module | What it does | Porting approach |
|----------|-------------|-----------------|
| `CompileGrammar.c` (2288 loc) | Grammar parsing & compilation | Call as-is via subprocess or FFI |
| `CompileProcs.c` (1856 loc) | Grammar procedure compilation | Same |
| `Polymetric.c` (2179 loc) | Polymetric expression engine | Core algorithm—test exhaustively |
| `FillPhaseDiagram.c` (2496 loc) | Time-setting engine | Same |
| `TimeSet.c` (735 loc) + `TimeSetFunctions.c` (1211 loc) | Phase table building | Same |
| `MakeSound.c` (2460 loc) | MIDI event scheduling | Tauri backend converts to MIDI |
| `MIDIfiles.c` (1211 loc) | MIDI file I/O | Can use directly or replace with Rust |
| `MIDIstuff.c` (2290 loc) | MIDI data processing | Keep as-is |
| `MIDIdriver.c` (1744 loc) | Hardware MIDI driver | Replace with Rust `midir` or Web MIDI |
| `Csound.c` (1507 loc) | Csound score generation | Keep as-is; spawn csound |
| `SoundObjects2/3.c` (1687 loc total) | Sound object management | Keep as-is |
| `ProduceItems.c` (2060 loc) | Item production engine | Keep as-is |
| `Encode.c` (1711 loc) | MIDI input encoding | Keep as-is |
| `Graphic.c` (1581 loc) | Score-to-graphics | Replace entirely with Vue canvas/SVG rendering |
| `DisplayArg.c` (1963 loc) + `DisplayThings.c` (893 loc) | Text display | Replace with Vue component rendering |
| `HTML.c` (454 loc) | HTML generation | Not needed; Vue renders directly |
| `Interface2.c` (421 loc) | Mac UI stubs | Already stubbed for console; remove |

## Out of Scope (Phase 1)

- Csound WASM integration—use MIDI playback only for web MVP
- Real-time MIDI capture in WASM—desktop only initially
- Tonal analysis visualizations (microtonality graphs)
- AI/ML integration (weight learning via parametric controllers)
- Undo/redo system—BP2 never had one; add in later phase
- VST/AU plugin hosting

---

## Summary

The C engine is solid, well-structured (~58K LOC, modular), and already multiplatform. The PHP frontend is the bottleneck and the primary thing to replace. **Tauri + Vue 3** is the ideal path: it gives a native desktop app with a modern UI toolkit while keeping the battle-tested C engine intact. WASM compilation is a free second target once the C engine builds with Emscripten.

Recommended next step: set up a `bp4-tauri` GitHub repo with the CMake build for the C engine + `cargo init` for Tauri, and run the existing `bp3-ctests` test suite against the WASM-compiled engine.
