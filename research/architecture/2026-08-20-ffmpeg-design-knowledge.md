# FFmpeg Design Knowledge — the media-processing giant as prior art and as a potential plugin

> **Date:** 2026-08-20. **Scope:** targeted design-doc + integration pass (official docs, dev-list rationale, the 2026 Rust binding landscape; no C implementation read — the point is *design independence* and a use-it-or-not decision, not a port). **Sources:** [ffmpeg.org documentation index](https://ffmpeg.org/documentation.html), [ffmpeg(1) manual](https://ffmpeg.org/ffmpeg.html), [ffmpeg-filters](https://ffmpeg.org/ffmpeg-filters.html), [developer docs (API/ABI rules)](https://ffmpeg.org/developer.html), [legal page](https://ffmpeg.org/legal.html), the [send/receive API rationale](https://ffmpeg-devel.ffmpeg.narkive.com/J8hajC4W/patch-1-3-lavc-introduce-a-new-decoding-encoding-api-with-decoupled-input-output), the [channel-layout rework thread](https://ffmpeg.org/pipermail/ffmpeg-devel/2022-January/291250.html) + [rationale post](https://ffmpeg.org/pipermail/ffmpeg-devel/2022-January/291727.html), [OSS-Fuzz's FFmpeg case study](https://deepwiki.com/google/oss-fuzz/7.2-ffmpeg:-large-scale-media-library), the [FFmpeg filtering guide](https://trac.ffmpeg.org/wiki/FilteringGuide), and [the state of media processing in Rust (2026)](https://dev.to/yeauty/the-state-of-media-processing-in-rust-2026-what-each-crate-actually-covers-2k2c). **Why:** FFmpeg is one of the best-maintained audio code bases in existence (~10–15 active core maintainers, 30+ years, 538 filters of which 122 are audio, 300+ fuzzer targets). Two questions: (1) what design lessons — especially audio-specific ones — should we absorb, as we did with Audacity and Ardour; (2) in the spirit of not inventing the wheel, can FFmpeg serve us *in* a plugin or *as* a plugin, and at what cost. RESEARCH §10 already floats an "FLAC/MP3 via a sidecar (ffmpeg/sox)" for master export — this pass tests that and more.

## 1. What FFmpeg is — the library platform underneath the CLI

FFmpeg is **not one program**: it is ~8 C libraries (`libavutil`, `libavcodec`, `libavformat`, `libavfilter`, `libswscale`, `libswresample`, `libavdevice`, `libavutil`-adjacent) plus *programs* built on them (`ffmpeg`, `ffprobe`, `ffplay`). The CLI is a thin, opinionated orchestration layer over the libraries — the muxing/filter-graph/scheduling logic lives in `fftools/`, not in the libraries. Two consequences matter for us:

- **The CLI is not a stable API.** Its behavior is a program, not a contract. The libraries carry the stability promise: *"Public APIs must be backward compatible within a given major version… We also guarantee backward ABI compatibility"* — [developer.html §3.4](https://ffmpeg.org/developer.html). Anything we build on the CLI is built on a moving, unversioned surface.
- **The library boundary is where reuse lives.** When projects say "use FFmpeg," they mean `libav*` (or the CLI for batch jobs). The [documentation index](https://ffmpeg.org/documentation.html) is explicit about the split: Command-line tools / Components (codecs, filters, formats, resampler) / Libraries / Developer docs.

Scale facts (2026): FFmpeg 8.x is current ([8.0 "Huffman" late 2025, 8.1 March 2026](https://master--endoflife-date.netlify.app/ffmpeg)); the [filter docs](https://ffmpeg.org/ffmpeg-filters.html) list **538 filters, 122 of them audio** (aecho, aformat, amix, anull, apad, aresample, atrim, volume, pan, tremolo, acompressor, alimiter, afftdn, afwtdn, acrossfade, …); [OSS-Fuzz](https://deepwiki.com/google/oss-fuzz/7.2-ffmpeg:-large-scale-media-library) auto-generates **300+ fuzzers** (decoders, encoders, demuxers, bitstream filters) by parsing `config_components.h`.

## 2. The audio model: sample formats, channel layouts, timebases, refcounted frames

- **Sample formats are a first-class enum.** `AVSampleFormat` covers u8/s16/s32/s64/flt/dbl × interleaved/planar. Filters declare what they accept; the graph negotiates; **conversion is auto-inserted** (`aformat`, `aresample`) where links disagree. There is no implicit "the audio is f32" — format is explicit at every edge.
- **Channel layout is a graph, not a count.** The 2022 rework replaced the old `uint64_t` bitmask + `channels` count with `AVChannelLayout` (named channels, custom layouts, order). The rationale, from the series: *"The new API is more extensible and allows for custom layouts. More accurate information is exported, **eg for decoders that do not set a channel layout, lavc will not make one up for them**"* — [291727](https://ffmpeg.org/pipermail/ffmpeg-devel/2022-January/291727.html). The old API silently *invented* layouts, and that was a decade of wrongness.
- **Time is rational, never float.** `AVRational` timebases thread through everything; every stream has its own timebase and conversion happens only at edges. This is the same discipline our absolute-frame clock and Ardour's superclock already follow — a third validation.
- **Buffers are refcounted.** `AVBufferRef` / `av_frame_ref` share decoded frames zero-copy; the [send/receive doc](https://ffmpeg-devel.ffmpeg.narkive.com/J8hajC4W/patch-1-3-lavc-introduce-a-new-decoding-encoding-api-with-decoupled-input-output) explicitly recommends refcounted packets/frames "or libavcodec might have to copy the input data." Sharing by reference is the scaling answer, not copying.

## 3. The two API reforms that are really architecture lessons

### 3.1 Decoupled input/output (send/receive) — FFmpeg 4.0, 2016/2018

The single most instructive change in FFmpeg's history, and it is *exactly* our patch-bay doctrine restated in C. From wm4/Anton Khirnov's commit: *"Until now, the decoding API was restricted to outputting 0 or 1 frames per input packet. It also enforces a somewhat rigid dataflow in general. This new API seeks to relax these restrictions by **decoupling input and output**. Instead of doing a single call on each decode step, which may consume the packet and may produce output, the new API requires the user to **send input first, and then ask for output**"* — [narkive](https://ffmpeg-devel.ffmpeg.narkive.com/J8hajC4W/patch-1-3-lavc-introduce-a-new-decoding-encoding-api-with-decoupled-input-output). One input may yield 0, 1, or many outputs; buffering is internal; and there is an explicit **flush/EOF protocol**: send `NULL` to enter draining mode, receive until `AVERROR_EOF`, reset with `avcodec_flush_buffers()` to resume.

The flush protocol is the part our design should absorb: **any stateful producer needs an explicit drain signal, not just an "end of input."** Our render loop treats blocks as a pure stream; when offline bounce reaches the end, codecs (and any node with internal buffering) need a draining phase to emit what they hold. We have no such signal today — `render()` is called per block until the timeline ends, and the tail is whatever the last block emitted.

### 3.2 Channel layout as a typed value — FFmpeg 6.0 era, 281 patches (2022)

The bitmask→`AVChannelLayout` rework was 281 patches touching every demuxer/muxer/filter/codec. It happened because "channel count" and "channel layout" had been conflated, arbitrary orders were unrepresentable, and decoders without layout info got a fabricated one. The lesson is architectural: **a channel model decided late is a 281-patch retrofit** — decide it before the log schema hardens. Our mixer's `channels: 1..=8` count is precisely the kind of field that becomes a layout later; the Soundcraft Notepad-12FX's 4 capture channels (2 mono + 1 stereo pair) are already a non-trivial layout.

## 4. libavfilter: the graph and the format-negotiation machinery

FFmpeg's filter graph is our patch bay's closest relative: nodes (filters) with named pads (`AVFilterPad`), connected by links (`AVFilterLink`) that carry format state, driven by push/pull callbacks (`filter_frame`/`activate`) — [deepwiki filter system](https://deepwiki.com/FFmpeg/FFmpeg/3.4-libavfilter-filter-system). The hard part is **format negotiation**: each filter declares, per pad, a set of acceptable formats via `query_formats`; `avfilter_graph_config` intersects the sets along links, with **auto-inserted conversion filters** (scale/aresample/aformat) where no common format exists — the CLI's `-pix_fmt` option even disables auto-conversion inside filtergraphs. The merge machinery is visible in [`libavfilter/formats.c`](https://raw.githubusercontent.com/FFmpeg/FFmpeg/master/libavfilter/formats.c) (`MERGE_FORMATS`), and the process itself was formally documented only in 2021 — after ~15 years of organic growth — [ffmpeg-devel 2021-08](http://lists.mplayerhq.hu/pipermail/ffmpeg-devel/2021-August/284041.html).

Two lessons:

- **Format/caps negotiation is the hardest problem in an audio graph, and it deserves explicit design, not ad-hoc conversion.** Our typed signal kinds (audio/control/trigger/note) are the seed; when we hit real mismatches (44.1 kHz capture vs 48 kHz pool, stereo vs mono, f32 vs f64), we need explicit negotiation at patch time — or silent auto-insertion of converters like ffmpeg's, but *designed*, with the insertion visible in the log.
- **Silently making up format facts is a bug factory.** "lavc will not make one up for them" is a rule we should adopt verbatim: never invent a channel count, sample rate, or layout that the producer didn't declare — refuse loudly instead.

## 5. Audio issues we can learn from (the "lessons" the user asked for)

1. **Encoder delay / priming breaks sample accuracy — [AV_CODEC_CAP_DELAY](http://ffmpeg-d.dpldocs.info/v4.4.1/ffmpeg.libavcodec.codec.AV_CODEC_CAP_DELAY.html).** *"Encoder or decoder requires flushing with NULL input at the end in order to give the complete and correct output."* Lossy encoders (MP3 padding, AAC priming) buffer internally; without drain, the tail is truncated, and without delay compensation, the *head* is shifted. For a sample-accurate clip arranger whose bounce is byte-identical, exporting to lossy codecs must model encoder delay explicitly — a decode of our own export must land back on the same frames, or clip edges drift. This is the audio-specific correctness trap most relevant to us.
2. **Resampling delay and drift.** `libswresample` (and `soxr` via it) must report delay (`swr_get_delay`) for sample-accurate trimming; we already use `rubato` for realtime and know this territory (Spike B drift), but the ffmpeg lesson is the *API shape*: resampler delay is a queryable property, not a surprise.
3. **Never trust container metadata.** Demuxers parse untrusted bytes; [OSS-Fuzz](https://deepwiki.com/google/oss-fuzz/7.2-ffmpeg:-large-scale-media-library) has found thousands of bugs in exactly this surface (e.g. use-of-uninitialized-value in `mxf_read_packet`). Sample rate, channel count, and durations from a container are *claims to be validated*, not facts. Our media pool imports arbitrary user files — that is a security boundary that deserves fuzzing from day one, not after.
4. **Format negotiation is where graphs go wrong.** The 2021 "document the negotiation process" commit exists because the behavior was subtle enough to need a spec. If we ever auto-insert converters, the insertion must be *logged and inspectable* (which our event log makes natural).
5. **Thread at pipeline boundaries, not inside DSP.** FFmpeg's own CLI pipelines demux→decode→filter→encode→mux as separate tasks (fftools' `ffmpeg_demux.c`/`ffmpeg_dec.c`/`ffmpeg_filter.c`/`ffmpeg_enc.c`/`ffmpeg_mux.c`/`ffmpeg_sched.c`/`sync_queue.c` — the file-for-file model behind the Rust `ez-ffmpeg` port), while individual codecs are mostly single-threaded with optional frame/slice threading. This validates our SPSC-ring pipeline: threads between stages, never inside a node's DSP.
6. **API stability is a commitment, priced in discipline.** FFmpeg's [developer docs](https://ffmpeg.org/developer.html) make backward/ABI compatibility a hard rule within a major version, maintained by a deprecation calendar and an `APIchanges` log. Our plugin API and log schema already freeze at Phase 1; ffmpeg shows the *process* cost of keeping that promise (deprecation periods, compat shims, the 2022 channel-layout compat layer).

## 6. Licensing — clean for us, with one caveat

[FFmpeg's legal page](https://ffmpeg.org/legal.html): core is **LGPL-2.1-or-later**; several optional parts/optimizations are **GPL-2-or-later**, and *"if those parts get used the GPL applies to all of FFmpeg."* Also: *"FFmpeg is not available under any other licensing terms, especially not proprietary/commercial ones, not even in exchange for payment."*

We are **GPL-3.0-or-later**: LGPL-2.1-or-later is compatible (the combined work ships under GPL-3.0), and even the GPL-2-or-later parts are fine for us. The caveat is future-facing: our own licensing story must keep GPL compatibility if we ever link libav in-process, and the compliance checklist (compile without `--enable-gpl` unless needed, ship license texts, etc.) is a packaging cost to budget.

## 7. Using FFmpeg *in* our plugins or *as* a plugin — the critical analysis

The Rust binding landscape as of mid-2026 is decisive, and it is not encouraging for in-process embedding — [the 2026 state-of-the-art survey](https://dev.to/yeauty/the-state-of-media-processing-in-rust-2026-what-each-crate-actually-covers-2k2c) (fuller text in the [cnblogs mirror](https://www.cnblogs.com/Yeauty/p/22018803)):

- **`ffmpeg-next`** (1.28 M downloads/mo): maintainer's own words — *"maintenance-only mode for the most part… Any PR to improve existing API is unlikely to be merged"*; fork chain `ffmpeg → ffmpeg-next → ffmpeg-the-third`; requires hand-writing the 40+ line `send_packet`/`receive_frame` boilerplate. Rust veteran kornel's verdict: *"avoid ffmpeg… all incomplete and poorly maintained."*
- **`rsmpeg`** (877★, larksuite): tracks FFmpeg 6/7/8 (0.18.0 followed 8.0) — the closest to current — but ~11 months between significant updates.
- **`video-rs`**: high-level, video-only, self-described *"still a work-in-progress."* **`ffmpeg-sidecar`** (~130 K/mo): subprocess wrapper — no FFI, but data crosses a process boundary.
- **`ez-ffmpeg`**: the in-process runtime modeled file-for-file on fftools; ~340★, one maintainer, links through `ffmpeg-next` (inherits its freeze risk), and its `cli` facade executes only a deliberately narrow verified subset on FFmpeg 7.1.
- The universal structural risk: *"safe 的皮, FFI 的骨"* — a safe wrapper skin over FFI bones; there is a public precedent of a "safe" binding API marked "might trigger undefined behavior" (zmwangx/rust-ffmpeg#225).

**The honest conclusion: in-process libav embedding is a high-cost path for us.** It adds a C toolchain + FFI glue + version-drift treadmill to a clean pure-Rust media core (`symphonia`/`hound`/`rubato`), and every binding layer in 2026 is either frozen, stale, or single-maintainer. We should **not** build the realtime core on it.

**What we *should* take, in the spirit of not reinventing the wheel:**

- **Lessons (free, high value):** §3.1 flush protocol, §3.2 channel-layout-as-type, §4 explicit negotiation, §5 encoder-delay model, §5 fuzz-the-import-path.
- **Code reuse, staged:**
  - **Export codecs (Phase 4) — ffmpeg CLI as an `OfflineProcess` sidecar**, exactly the CDP8 sidecar pattern already locked for the sculptor profile and already named in RESEARCH §10 ("FLAC/MP3 via a sidecar (ffmpeg/sox)"). Zero linking, process isolation (a hostile file crashes the sidecar, not us), the entire 30-year command-line knowledge base available, and our event log can drive it as an ordinary logged `OfflineProcess` event.
  - **Long-tail import decode (sculptor profile)** — same sidecar slot: symphonia's codec table (WAV/FLAC/MP3/AAC/Ogg) covers the pool; HE-AAC/Opus/AC-3 and exotic formats are offline-converted by the sidecar before entering the pool.
  - **Do *not* import libavfilter for realtime effects** — our patch bay + `fundsp` covers the 122-audio-filter need; importing the filter graph would duplicate the graph machinery we already built. Steal the *negotiation idea*, not the filters.
- **Revisit trigger:** only if realtime needs a codec/format outside symphonia's table *and* the binding landscape stabilizes (rsmpeg tracking each FFmpeg major is the pattern to watch) — and even then, prefer the sidecar.

## 8. Mapping to sound-arranger — their experience → our disposition

| Our concern | What FFmpeg says | Disposition |
|---|---|---|
| **Realtime decode (media pool)** | libavcodec covers everything; symphonia covers WAV/FLAC/MP3/AAC/Ogg but hits walls (HE-AAC, Opus incomplete, AC-3 unsupported — [2026 survey](https://dev.to/yeauty/the-state-of-media-processing-in-rust-2026-what-each-crate-actually-covers-2k2c)) | Keep symphonia for the pool (pure Rust, already chosen); the wall IS the sidecar's job (offline convert → pool). |
| **Bounce/export codecs** | FLAC/MP3/AAC via ffmpeg; encoder delay is explicit (`AV_CODEC_CAP_DELAY`) | **Adopt the sidecar** (CDP8 pattern, already in RESEARCH §10); model encoder delay so lossy export stays sample-accurate at clip edges. |
| **Channel model** | 281-patch retrofit of a bad early decision; "never make one up" | **Adopt early:** a channel-layout type before `channels: 1..=8` hardens in the log; the Soundcraft 4-capture (2 mono + 1 stereo pair) is already non-trivial. |
| **Format negotiation** | The hardest part of libavfilter; auto-inserted converters; documented only after 15 years | Adopt the *discipline*: explicit negotiation at patch time for sample-rate/channel mismatches, visible in the log; refuse silently-invented formats. |
| **Drain/flush semantics** | send `NULL` → drain → `AVERROR_EOF` → `flush_buffers` | **Adopt:** an explicit end-of-stream/drain signal for offline bounce and any stateful node — today our render loop has no drain phase. |
| **Buffer sharing** | Refcounted `AVBufferRef`/`av_frame_ref`; "or libavcodec might have to copy" | Validates our no-alloc render path + rings; adopt reference-counted audio buffers when the pool grows beyond capture→playback. |
| **Threading** | Pipeline tasks, not per-node threads | Validates SPSC-ring pipeline (Spike B); keep threads between stages. |
| **Import security** | 300+ fuzzers; thousands of demuxer bugs found | **Adopt:** treat media-pool import as a security boundary; fuzz the import path from day one. |
| **API stability** | Backward+ABI compatibility within a major version, via deprecation calendar | Keep our freeze-at-Phase-1 discipline; adopt a deprecation calendar for the plugin API. |
| **Licensing** | LGPL-2.1+ core, GPL-2+ optional; GPL if enabled | Compatible with GPL-3.0; budget the compliance checklist only if we ever link in-process (we plan not to). |

## 9. Debt evidence — validating our minimal-core choice

FFmpeg's own history is the strongest argument for our direction: the format-negotiation machinery grew organically for 15 years before it was documented; the channel model had to be ripped out in a 281-patch series; the decode API's rigid 1-in/0..1-out dataflow had to be replaced by a decoupled send/receive model — the very model our patch bay ships from day one. Each of these was a *late redesign of a core abstraction*, and each one is a seam we have already drawn. The counterweight is scope: FFmpeg is 30 years of one team's obsessive generality. We are building a small platform and one profile; the lesson is to absorb the *shapes* (negotiation, drain, channel layout, refcounted buffers) without importing the *weight* (in-process libav).

## 10. Decision candidates (for Agent Notes when the phases land)

1. **Media pool decode stays pure Rust (symphonia) in Phase 1; no libav in the realtime core.** In-process libav from Rust is a high-cost path (binding zoo: frozen `ffmpeg-next`, stale `rsmpeg`, single-maintainer `ez-ffmpeg`; FFI-safety risk; C toolchain; version treadmill).
2. **Master export (Phase 4) and long-tail import (sculptor): an ffmpeg CLI sidecar as an `OfflineProcess` plugin** — the CDP8 pattern, zero linking, logged as an ordinary event. Already anticipated by RESEARCH §10's "FLAC/MP3 via a sidecar (ffmpeg/sox)".
3. **Introduce a channel-layout type before `channels: 1..=8` hardens in the log schema** (AVChannelLayout-style: named channels, explicit order, "never make one up") — see the [channel-layout note](.agents/notes/proposed/feature/2026-08-20-channel-layout-typed-value.md).
4. **Model encoder delay/priming explicitly in bounce** (`AV_CODEC_CAP_DELAY` awareness) so lossy export is sample-accurate at clip edges and decodes back onto the same frames.
5. **Add an explicit drain/EOF phase to offline bounce and any stateful node** (the send/receive flush protocol) — see the [drain/EOF note](.agents/notes/proposed/feature/2026-08-20-drain-eof-phase.md).
6. **Fuzz the media-pool import path from day one** (the 300-fuzzer lesson).
7. **Explicit format negotiation for sample-rate/channel mismatches at patch time, logged** — or deliberate auto-insertion, designed and visible — never silent invented formats.

## Sources

- [ffmpeg.org/documentation.html](https://ffmpeg.org/documentation.html) — doc index (the linked entry point)
- [ffmpeg(1) manual](https://ffmpeg.org/ffmpeg.html) — CLI semantics, stream selection, `-pix_fmt` auto-conversion switch
- [ffmpeg-filters](https://ffmpeg.org/ffmpeg-filters.html) — 538 filters, 122 audio
- [developer.html §3.4 Library public interfaces](https://ffmpeg.org/developer.html) — API/ABI compatibility rules
- [legal.html](https://ffmpeg.org/legal.html) — LGPL-2.1+ / GPL-2+ terms, no other licensing
- [send/receive API rationale (wm4, 2016)](https://ffmpeg-devel.ffmpeg.narkive.com/J8hajC4W/patch-1-3-lavc-introduce-a-new-decoding-encoding-api-with-decoupled-input-output) — "decoupling input and output"
- [New channel layout API series (2022)](https://ffmpeg.org/pipermail/ffmpeg-devel/2022-January/291250.html) and [rationale post](https://ffmpeg.org/pipermail/ffmpeg-devel/2022-January/291727.html) — "lavc will not make one up for them"
- [libavfilter negotiation documentation commit (2021)](http://lists.mplayerhq.hu/pipermail/ffmpeg-devel/2021-August/284041.html) — the negotiation process spec'd late
- [`libavfilter/formats.c`](https://raw.githubusercontent.com/FFmpeg/FFmpeg/master/libavfilter/formats.c) — `MERGE_FORMATS` merge machinery
- [deepwiki: libavfilter filter system](https://deepwiki.com/FFmpeg/FFmpeg/3.4-libavfilter-filter-system) — pads/links/callbacks
- [OSS-Fuzz FFmpeg case study](https://deepwiki.com/google/oss-fuzz/7.2-ffmpeg:-large-scale-media-library) — 300+ fuzzers, 15 external deps
- [AV_CODEC_CAP_DELAY doc](http://ffmpeg-d.dpldocs.info/v4.4.1/ffmpeg.libavcodec.codec.AV_CODEC_CAP_DELAY.html) — flush-to-drain semantics
- [FFmpeg filtering guide (wiki)](https://trac.ffmpeg.org/wiki/FilteringGuide) — filtergraph syntax, chains
- [The state of media processing in Rust (2026)](https://dev.to/yeauty/the-state-of-media-processing-in-rust-2026-what-each-crate-actually-covers-2k2c) + [cnblogs mirror](https://www.cnblogs.com/Yeauty/p/22018803) — binding landscape, `ffmpeg-next` maintenance-only, `rsmpeg` FFmpeg 8.0, `ez-ffmpeg` internals, symphonia codec walls
- [endoflife.date — FFmpeg](https://master--endoflife-date.netlify.app/ffmpeg) — 8.0/8.1 current
