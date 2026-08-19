# Research: DAWproject adoption & the EP-133 → DAW export tool

> **Date:** 2026-08-18. **Scope:** is the Bitwig `dawproject` interchange format being adopted, and what does the EP-133 export tool (phones24) mean for the platform and for the owner's hardware workflow. **Sources:** [bitwig/dawproject](https://github.com/bitwig/dawproject) (MIT), [Bitwig DAWproject FAQ](https://www.bitwig.com/support/technical_support/system-requirements-62/), [phones24/ep133-export-to-daw](https://github.com/phones24/ep133-export-to-daw), Tracktion Waveform manual, Steinberg forums, music-tech press.

## 1. What dawproject is

An **open, vendor-agnostic project-exchange format** for DAWs, launched 2023-09 by **Bitwig + PreSonus**. v1.0, declared stable. Container = **ZIP**, contents = **XML** (`project.xml` + `metadata.xml`), UTF-8; the exporter chooses the internal directory structure for media/plugin files. **MIT licensed** — directly compatible with our GPL-3.0-or-later; implementable from the spec (XML + ZIP parsing) with no heavy dependencies, in the same spirit as our hand-rolled WAV module.

What it can carry (vs MIDI files, which are "lower-level" note data with no ramps, and AAF, which is seconds-only post-audio): audio clips with fades/crossfades/amplitude/pan/time-warp/transpose; notes + note expressions; automation (tempo, time signature, MIDI, volume, pan, mute, sends, plugin parameters, built-in device parameters); full plugin states (always embedded); clip-launcher clips/scenes. **Time: beats and seconds combined** — the exporter keeps its own model, the importer flattens as needed. Generic built-in devices (EQ/comp/gate/limiter) travel as such.

## 2. Adoption status (as of the 2024-11-06 Bitwig FAQ + later sightings)

| Support | Products |
|---|---|
| **Native import + export** | Bitwig Studio 5.0.9+, **Studio One 6.5+** (PreSonus), **Cubase 14**, **Cubasis 3.7.1**, **VST Live 2.2** (Steinberg), **Tracktion Waveform** (import & export per its manual) |
| **Converters / tools** | [Moss's ProjectConverter](https://github.com/git-moss/ProjectConverter) (third-party DAW → dawproject), [ep133-export-to-daw](https://github.com/phones24/ep133-export-to-daw) (EP-133 → dawproject) |
| **Not native** | Logic Pro, FL Studio, REAPER (forum scripts only), Ardour, Nuendo/WaveLab (feature requests/discussions) |

**Verdict: adopted, and the leading candidate for the interchange standard — but not universal.** The co-founders (Bitwig, Studio One) plus Steinberg and Tracktion shipping it natively, an MIT spec, and a growing converter ecosystem (including hardware-export tools) make it the strongest open format for *project* transfer. Logic/FL/REAPER/Ardour have not joined natively, so "import any project" is not yet true. Cross-DAW fidelity is imperfect by design (e.g. Studio One doesn't read Bitwig's Clip Launcher data; plugin states only mean something where the plugin exists).

## 3. EP-133 → DAW export tool (phones24)

A **browser app (PWA)** — not TE-affiliated, reverse-engineered — that exports **EP-133 / EP-1320 / EP-40** projects to **Ableton Live 11+ (project with samples), DAWproject (project + archived samples), REAPER (project with samples), and MIDI (file + samples)**. It works two ways: live via **WebMIDI** (reads the device's structured filesystem through TE's sysex protocol), or **offline by dropping a `.pak`/`.ppak` backup** from the EP Sample Tool (the backup is an archive of the device filesystem). Supports sampler envelopes, trim points, stretching, playback modes, fader params, FX send/returns, the 4 device groups (incl. Drum Rack export), scenes, time signatures. Actively developed (changelog through 2026-05).

**For the owner:** the `.pak`-backup route works without the hardware attached; the **DAWproject target is the one that can eventually land in sound-arranger** — the tool makes the EP-133 a *project* source, not just an audio jam source.

## 4. Mapping to sound-arranger

- **dawproject is the natural project-level import/export seam** — the composition-seams note's `Codec` seam (WAV/FLAC/MP3) covers audio; dawproject is the *project* tier (tracks/clips/notes/automation). MIT + XML/ZIP + a stable v1.0 spec → implementable as a plugin behind that seam with no licensing or dependency friction.
- **Fit with our model:** its beats+seconds time model matches our tempo-map clock; notes with pitch/expressions match our continuous-pitch musical-event model; clips as `{path, start, len}` refs with embedded-or-referenced media match our media pool. Export = *fold the session log* to a dawproject snapshot; import = *parse dawproject into logged events* (one `ImportProject` event, replayable, media content-hash referenced) — our event-sourced log makes both directions deterministic and undoable, which no DAW-importer offers.
- **The owner's hardware becomes a real provider:** EP-133 → (tool) → dawproject → our import. This is a genuine second-provider demand for the import seam, not hypothetical.
- **When:** Phase 1–2, behind the import/export seam — not before (umbrella-first budget rule). Recorded as a decision candidate, not a locked note.

## 5. Decision candidates (when the import/export seam is built)

- **Project interchange = dawproject** (import + export), MIT, XML+ZIP, as a plugin behind the `Codec` seam.
- **EP-133 path:** support importing dawproject files produced by the export tool (and, for the personal workflow, note the `.pak` offline route).
- Caveats to design around: plugin states only transfer where plugins exist; Studio One/Bitwig Clip-Launcher fidelity limits; generic devices (EQ/comp/gate/limiter) as the portable effect tier.
