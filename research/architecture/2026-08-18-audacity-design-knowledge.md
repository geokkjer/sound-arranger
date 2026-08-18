# Audacity Design Knowledge — Findings Report

Design documentation only (CMJ paper, AOSA chapter, archived wiki, PR/issue rationale, dev-mail postmortems). No C++ implementation read. All claims cited.

## 1. Block-file storage (BlockFile / Sequence / SampleBlock)

- **Origin**: audio is a *sequence* of disk blocks with a `k..2k`-sample size invariant ("unrolled linked list"); insert touches ≤6 nodes, delete a constant number, then blocks are merged/reapportioned. 2002 impl: 32–64 KB blocks; block size trades editing speed against playback bandwidth (1 MB nodes ≈ 80–93% of peak sequential read rate) — [Mazzoni & Dannenberg, CMJ 26:2 (2002)](https://www.cs.cmu.edu/~rbd/papers/audacity-cmj2002.pdf).
- **Shipped**: blocks "around 1 MB"; never have internal free space, never exceed max size, so an edit copies ≤1 block's worth; `.aup` XML master lists blocks; many files/dir was slow on Windows → subdirectory hierarchy capped at ~100 files — [AOSA, "Audacity" (James Crook)](https://aosabook.org/en/v1/audacity.html).
- **Refcount**: blocks are reference-counted; paste shares blocks by reference; delete frees at refcount 0; versions of the audio share blocks in agreeing parts — [wiki ArchitecturalDesign](https://web.archive.org/web/20201113145943/https://wiki.audacityteam.org/wiki/ArchitecturalDesign).
- **3.x**: `SampleBlock` abstraction over sqlite; `Sequence`/`WaveClip` internals hidden — [PR #2219](https://github.com/audacity/audacity/pull/2219); disk chunk ≈262144 samples (~1 MB @ 32-bit float) — [forum](https://forum.audacityteam.org/t/plugins-are-called-for-one-chunk-at-a-time/34674/1).
- **Edge cases**: clipboard refs leak or lose data at project close (deref ordering) — [bug 147](https://sourceforge.net/p/audacity/mailman/message/13612449/); blocks exposed to users → users moved `.aup` without `_data` and broke projects, a driver for single-file `.aup3` — [AOSA](https://aosabook.org/en/v1/audacity.html), [3.0.0 changelog](https://support.audacityteam.org/~/changes/8NxxSWYhU0HY4MB0E6vn/additional-resources/changelog/older-versions/audacity-3.0.0/new-features-in-audacity-3.0.0.md).

## 2. Waveform peak pyramid

- Per block, min/max of each 256-sample group ("reductions") is stored at the **start of the block file**; zoomed-out drawing computes per-pixel min/max in one pass over reductions (approx max rounds to 256-sample boundaries, off by ≤1 pixel); zoomed in uses raw samples — [CMJ paper](https://www.cs.cmu.edu/~rbd/papers/audacity-cmj2002.pdf); implemented as `Sequence::GetWaveDisplay` — [forum](https://forum.audacityteam.org/t/how-does-audacity-render-waveforms/13615/2).
- Separate *summary BlockFiles* cache min/max; **on-demand loading** builds them in a background task (striped placeholder until done; blocks processed out of order) — seed for real-time effects — [AOSA](https://aosabook.org/en/v1/audacity.html).
- Cache holds Min/Max/RMS per pixel; populating analyzes every sample once, then draws are fast — the analysis is the expensive step on huge recordings — [forum](https://forum.audacityteam.org/t/100-orphaned-blocks-i-would-not-call-this-program-reliable/44459/14).

## 3. Playback scheduling & edit-during-playback

- **Threads**: PortAudio device thread (`audacityAudioCallback`, ~5 s ring buffers) + `AudioIO` thread doing disk I/O (`FillBuffers`, also "software play-through" overdub) + GUI thread repainting on a 20 Hz timer; shared variables, no mutexes — [AOSA §2.5](https://aosabook.org/en/v1/audacity.html).
- `PlaybackSchedule`/`PlaybackPolicy` were split out of `AudioIO` "so that AudioIOBase is less involved with real-time playback", enabling looping, scrubbing, seeking — [PR #812](https://github.com/audacity/audacity/pull/812); scrubbing = scheduled playback at mouse-driven variable speed — [manual](https://manual.audacityteam.org/man/scrubbing_and_seeking.html).
- **Edge case**: `PlaybackSchedule::TimeQueue` has no atomics; it relies on `RingBuffer`'s release-ordered atomics, but `RepositionPlayback` runs *after* `RingBuffer::Put` — documented synchronization bug — [issue #2098](https://github.com/audacity/audacity/issues/2098).
- **Editing while playing**: destructive edits are disabled during playback; pausing then applying an effect stops the stream — [manual FAQ:Editing](https://manual.audacityteam.org/man/faq_editing.html).

## 4. Undo design

- **Snapshot of structure, not samples**: copy the whole node list onto an undo stack before each op; refcounted blocks mean only new blocks cost disk; undo reverts the pointer; ~64–128 KB wasted per op, "unlimited" levels, constant time — [CMJ paper "Undo"](https://www.cs.cmu.edu/~rbd/papers/audacity-cmj2002.pdf); old blocks "hang around to support undo until we save" — [AOSA](https://aosabook.org/en/v1/audacity.html).
- **Gesture grouping**: history has a 'summary' so consecutive commands lump into one undo step — [wiki ArchitecturalDesign](https://web.archive.org/web/20201113145943/https://wiki.audacityteam.org/wiki/ArchitecturalDesign). Undo retains unreferenced data; `.aup3` compaction removes undo history — [3.0.0 changelog](https://support.audacityteam.org/~/changes/8NxxSWYhU0HY4MB0E6vn/additional-resources/changelog/older-versions/audacity-3.0.0/new-features-in-audacity-3.0.0.md).

## 5. The ongoing refactor

- Licameli's 2020 modularization: **48 dynamically-loaded modules**; "retroactive plug-in development — discovering the interfaces that allow us to *subtract* existing features"; CMake emits Graphviz dependency graphs; core executable ~55% smaller — [audacity-devel](https://sourceforge.net/p/audacity/mailman/audacity-devel/thread/CAMe%3D4isgkGZobTaL7w-taWRuOWEoHhjy8ERVtauhsLeK-X%3Do8Q%40mail.gmail.com/).
- ~38 "Extract lib …" PRs (project [#1586](https://github.com/audacity/audacity/pull/1586), sample track [#2219](https://github.com/audacity/audacity/pull/2219), audio io [#2420](https://github.com/audacity/audacity/pull/2420), [#4173](https://github.com/audacity/audacity/pull/4173) "GUI Toolkit-neutral library for the audio engine"). Goal: break `AudioIO`↔`WaveTrack` coupling; both depend on `SampleTrack`; the engine must not know about clips/Sequence/sqlite.
- **Channel iteration rewrites**: replace stereo `GetLink` special-casing with iterators + a join/split/swap-channel API — [issue #5180](https://github.com/audacity/audacity/issues/5180); the original mono/stereo debt (~100 call sites, 26 files) is documented in [AOSA's "GetLink story"](https://aosabook.org/en/v1/audacity.html).

## 6. Effects / plugin processing

- **Offline, block-based, not real-time**: "you select audio, apply the effect, then listen" — [wiki ArchitecturalDesign](https://web.archive.org/web/20201113145943/https://wiki.audacityteam.org/wiki/ArchitecturalDesign); effects implement `ProcessBlock(float **in, float **out, size_t blockLen)` — [wiki Toolbox/Effects](https://web.archive.org/web/2020/https://wiki.audacityteam.org/wiki/Toolbox/Effects).
- **Chunked invocation**: effects are called per chunk, so state (e.g. biquad history) must survive across calls; Nyquist gets ~1020-sample buffers; disk chunks are 262144 samples — [forum](https://forum.audacityteam.org/t/plugins-are-called-for-one-chunk-at-a-time/34674/1).
- **Preview/latency**: real-time effects + background rendering are the most-requested features; plan: "render with a head start, let the playback cursor catch up" — needs block overlap for stateful effects and varying parameters; on-demand loading is the evolutionary step — [AOSA §2.8](https://aosabook.org/en/v1/audacity.html).

## 7. Known debt / would-do-differently

- BlockFile structure "exposed to end users" was the main flaw; a single file with GC (copy-on-save when >X% unused) "would increase performance rather than reduce it" — exactly what `.aup3`/sqlite delivered — [AOSA](https://aosabook.org/en/v1/audacity.html), [3.0.0 changelog](https://support.audacityteam.org/~/changes/8NxxSWYhU0HY4MB0E6vn/additional-resources/changelog/older-versions/audacity-3.0.0/new-features-in-audacity-3.0.0.md).
- TrackPanel ("truly horrible": GUI+app logic mixed, absolute positions) wants flyweight self-drawn widgets — [AOSA](https://aosabook.org/en/v1/audacity.html).
- Three thread models forced by library abstractions, plus gratuitous data copying — [AOSA §2.5/§2.9](https://aosabook.org/en/v1/audacity.html).
- Repeated 16-bit effects accumulate quantization error; paper recommends 32-bit float on disk; many small files give ~4% potential fragmentation and slow backups — [CMJ paper](https://www.cs.cmu.edu/~rbd/papers/audacity-cmj2002.pdf).
