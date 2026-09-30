# First session — make sound in fifteen minutes

> 🕒 Last verified against commit `ddb03a7` (2026-09-30). If the code has moved on,
> trust the code and move this line forward.

You don't need to understand Rust, audio programming, or the architecture to do
this session. You need a working toolchain and fifteen minutes. By the end you
will have assembled a four-plugin signal chain, bounced it to a WAV file,
*listened to it*, broken the engine three ways on purpose, crashed it mid-edit
and lost nothing, and proven its central promise — identical input, identical
output — with your own `cmp`.

Everything here runs against the **headless host**: a real binary that exercises
the full Host API contract with no GUI. Every fancy thing built later (the
terminal and iced shells, the arranger timeline) drives this same contract.

## Before you start

You need the Rust toolchain and `node` on `PATH` (see the
[dev environment](../README.md#development)), then prove the toolchain:

```sh
cargo test -p engine          # ~130 core tests, should pass in about a second
```

## Step 1 — assemble a profile from four plugins

Write this script anywhere (e.g. `/tmp/hello.script`):

```text
host v1

# Four plugins assemble the profile: a rhythm generator, two translators,
# and the soft mixer that owns the master bus.
mount euclidean steps=8 pulses=5
mount scale root=0 note_len=2400
mount tone gain=0.9 blip_len=1800
mount mixer channels=4

# Wire them: triggers -> notes -> audio -> channel 0.
patch euclidean.triggers scale.trigger
patch scale.note tone.note
patch tone.audio mixer.ch0

# The master fader.
set_param mixer master.gain 0.8 @0

# Render 2 seconds to a WAV file.
bounce 96000 /tmp/hello.wav
```

Run it:

```sh
cargo run -q -p host -- /tmp/hello.script
```

Expected output:

```text
host: bounced 384044 bytes to /tmp/hello.wav
engine log events: 9
media commands: 1
underruns: 0
deferred splices: 0
master out node: Some(NodeId(3))
drain tail frames: 0
```

Play the WAV in any audio player. You should hear a syncopated 5-pulses-in-
8-steps blip pattern at 120 BPM. What happened, as data: the euclidean plugin
emits *triggers* (sample-timestamped events), the scale plugin translates them
into *notes* (pitched, timed), the tone plugin turns notes into *audio*, and the
mixer sums channels onto the master bus. Each stage is a plugin mounted into the
core's patch bay; none of them are special-cased anywhere in the engine.

## Step 2 — read the summary honestly

Each line of the output is an invariant made visible:

- **`bounced 384044 bytes`** — 96 000 frames of **stereo** 16-bit audio (the
  mixer's master bus is two channels) plus a 44-byte WAV header: 96 000 × 2 × 2
  + 44. The byte count is the arithmetic of the format, not a mystery number.
- **`engine log events: 9`** — four mounts, three patches, one param change, and
  the bounce itself (a media command is logged too, so a replay reproduces the
  whole session): all logged with their absolute frame. *The log is the
  session.* Nothing else was consulted to produce the sound.
- **`underruns: 0`** — every media reader kept ahead of the render clock. A
  nonzero count means disk/threads lost the race and zeros were emitted instead
  of samples (audible as gaps).
- **`master out node: Some(NodeId(3))`** — the mixer claimed the master bus when
  it mounted. Mount order mattered.
- **`drain tail frames: 0`** — after the last scheduled frame the render drained
  nothing extra. A nonzero tail is the *good* outcome of a bounce that ends on
  a ringing effect: the tail is rendered, counted, and reported rather than cut.
  A ` (CAPPED)` suffix would say the drain hit its bound and stopped early —
  reported, never hidden.

## Step 3 — prove the central promise

The engine's golden property is *rendering is a pure function of the log*: same
commands ⇒ byte-identical audio. You just ran it once; run it again with a
different filename and compare:

```sh
sed 's/hello.wav/again.wav/' /tmp/hello.script > /tmp/again.script
cargo run -q -p host -- /tmp/again.script
cmp /tmp/hello.wav /tmp/again.wav && echo IDENTICAL
```

That `IDENTICAL` is the same property the test suite proves at engine level,
media level, and WAV-bytes level. It's what makes replay, undo, and forks
trustworthy later.

## Step 4 — break it three ways on purpose

**A. Unknown name at parse time.** Add `mount reverb` above the bounce line and
rerun. Refused instantly with the registry listed (`unknown plugin 'reverb'
(registry: euclidean, scale, tone, mixer, master, clock_out)` — six plugins,
and still no reverb): exit code **2**, nothing rendered. Unknown words fail
*before* anything executes.

**B. Using something before mounting it.** Delete the `mount mixer` line, keep
everything else. Now `patch tone.audio mixer.ch0` has no endpoint and
`run_script` fails: exit code **1**, printed reason (`plugin 'mixer' is neither
scheduled nor mounted`). Crucially, *every event up to the refusal stays logged
and the refused one does not* — bad commands can't corrupt history.

**C. The units trap** (I hit this writing this guide — it's real): change
`blip_len=1800` to `blip_len=0.25`. The bounce succeeds and produces **silence**.
Length parameters are measured in **sample frames**, not seconds — `0.25`
truncates to zero frames, so every blip is zero-length. Audio params have units;
a plugin's declared ranges live in its `ParamDef` table (see `blip_len` in
`crates/engine/src/plugins/tone.rs`). Silent-but-valid output is always worth
sample-counting — the file is 16-bit stereo, so unpack signed 16-bit samples at
a 2-byte stride:

```sh
python3 -c "
import struct
d=open('/tmp/hello.wav','rb').read()
print(sum(1 for i in range(44,len(d),2) if struct.unpack('<h',d[i:i+2])[0]!=0),'nonzero')"
```

On the trap run that prints `0 nonzero`; on the original it prints `35656
nonzero`. (The older edition of this guide used `note_len=2400 → 0.25` as the
trap; in today's chain that parameter is audibly inert — the tone's envelope
length is `blip_len`, so the trap moved with the code.)

## Step 5 — save it, crash it, recover it

Everything so far lived in one process. A **session directory** makes it survive:
`save` writes the document itself, and a journal keeps the edits you made after
the save — so a crash costs at most the write it was in the middle of.

**Save the session.** Append one line to the script (or edit your copy) and rerun:

```text
save /tmp/hello-session
```

(keep the bounce line above it). Two files appear: `session.txt` — open it and
*read* it. It is the script you ran, in the log's own words:

```text
host v1
session_rate 48000
mount euclidean steps=8.0 pulses=5.0
...
```

That is the "the log is the document" claim made literal: the session is a
text file you can read, diff and copy. Beside it sits `journal.txt`, empty —
the autosave ledger, clean so far.

**Prove the reload.** In a fresh process, load the directory and bounce again:

```text
host v1
load /tmp/hello-session
bounce 96000 /tmp/hello-again.wav
```

`cmp /tmp/hello.wav /tmp/hello-again.wav` — **identical**. The directory is the
session; replaying it is byte-exact.

**Now crash it.** The journal is written per edit, so a `kill -9` mid-write can
leave a torn final line. Simulate that: put two complete edits in the journal
and one half-written line (note the last line has no newline and stops
mid-word — exactly what a killed process leaves):

```sh
printf 'set_param mixer master.gain 0.4 @0\nset_param mixer master.gain 0.5 @0\nset_param mixer master.gai' \
  > /tmp/hello-session/journal.txt
```

Load and bounce again, the same way as above. The first line of the output is
new — the report of what the recovery did:

```text
host: journal: 2 applied, 1 torn, 0 refused
host: bounced 384044 bytes to /tmp/crashed-recovered.wav
engine log events: 11
```

Four things happened, and each is visible:

- **The session opens.** Exit code 0. A torn autosave line is *never* fatal —
  refusing to open a session because its own autosave was interrupted would be
  the worse failure.
- **`journal: 2 applied, 1 torn, 0 refused`** — the recovery's own account:
  both complete edits replayed, the torn tail dropped, nothing refused. The
  terminal shell says the same thing beside its `: load` line.
- **`engine log events: 11`** — the baseline 8, plus both journal edits, plus
  the bounce. Recovery is per gesture: whole entries apply, the torn tail is
  dropped. One bad line does not take the edits after it anywhere, because
  there is nothing after it — the journal is append-only.
- **The edits survived.** `cmp` against the clean reload's WAV: different. Count
  the peak with the Step 4 snippet: it is **10248** where the clean bounce
  peaked **16397** — a ratio of exactly 0.625, which is `0.5 / 0.8`: the
  journaled gain change is *audibly present* in the recovered render. The
  crash cost nothing but the write it was in the middle of.

The report line is built from the host's recovery record — applied gestures,
torn lines, refused entries, and the first refusal's own words — through the
same read-side discipline as the engine's apply faults: the record is data,
the sentence is a read. A clean load (an empty journal) has no story and stays
quiet.

## Where to go next

- To understand *why* the chain looks like triggers→notes→audio, read
  [architecture-explainer.md](architecture-explainer.md) §3 (the patch bay) and
  §4 (the log and the engine).
- To be able to *modify* these crates, work through
  [rust-course/README.md](rust-course/README.md) — twelve lessons, each anchored
  in one real file of this repo.
- Beyond this tour: the same script format speaks `pool` / `arrange ...`
  commands (cut-and-arrange clips from recorded WAVs — see
  `crates/host/src/lib.rs`, function `parse_arrange`), and the media engine's
  multi-channel capture (real device I/O into the float-WAV pool) lives under
  `crates/media/src/capture.rs` — its hardware tests run with
  `cargo test -p media -- --ignored`. Gear-specific USB-capture tooling lives in
  the separate studio project, not this repo.

---

*Authored with GLM-5.3 Flash · ZCode, 2026-08-27; re-verified against `7c5a2e7`
with DeepSeek-V4-Flash · DeepSeek Harness, 2026-09-05; re-verified against
`2f6ad78` (every output above re-run) and against `ddb03a7` (step 5's report
line) with GLM-5.3 · OpenCode, 2026-09-30.*
