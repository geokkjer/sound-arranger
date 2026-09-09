# First session — make sound in fifteen minutes

> 🕒 Last verified against commit `7c5a2e7` (2026-09-05). If the code has moved on,
> trust the code and move this line forward.

You don't need to understand Rust, audio programming, or the architecture to do
this session. You need a working toolchain and fifteen minutes. By the end you
will have assembled a four-plugin signal chain, bounced it to a WAV file,
*listened to it*, broken the engine three ways on purpose, and proven its
central promise — identical input, identical output — with your own `cmp`.

Everything here runs against the **headless host**: a real binary that exercises
the full Host API contract with no GUI. Every fancy thing built later (the Tauri
shell, the timeline) drives this same contract.

## Before you start

Enter the dev shell (Nix/devenv — see [../README.md](../README.md)), then prove
the toolchain:

```sh
cargo test -p engine          # ~56 core tests, should pass in about a second
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
host: bounced 192044 bytes to /tmp/hello.wav
engine log events: 8
media commands: 1
underruns: 0
deferred splices: 0
master out node: Some(NodeId(3))
```

Play the WAV in any audio player. You should hear a syncopated 5-pulses-in-
8-steps blip pattern at 120 BPM. What happened, as data: the euclidean plugin
emits *triggers* (sample-timestamped events), the scale plugin translates them
into *notes* (pitched, timed), the tone plugin turns notes into *audio*, and the
mixer sums channels onto the master bus. Each stage is a plugin mounted into the
core's patch bay; none of them are special-cased anywhere in the engine.

## Step 2 — read the summary honestly

Each line of the output is an invariant made visible:

- **`engine log events: 8`** — four mounts, three patches, one param change: all
  logged with their absolute frame. *The log is the session.* Nothing else was
  consulted to produce the sound.
- **`underruns: 0`** — every media reader kept ahead of the render clock. A
  nonzero count means disk/threads lost the race and zeros were emitted instead
  of samples (audible as gaps).
- **`master out node: Some(NodeId(3))`** — the mixer claimed the master bus when
  it mounted. Mount order mattered.

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
rerun. Refused instantly with the registry listed (there is no reverb plugin):
exit code **2**, nothing rendered. Unknown words fail *before* anything executes.

**B. Using something before mounting it.** Delete the `mount mixer` line, keep
everything else. Now `patch tone.audio mixer.ch0` has no endpoint and
`run_script` fails: exit code **1**, printed reason (`plugin 'mixer' is neither
scheduled nor mounted`). Crucially, *every event up to the refusal stays logged
and the refused one does not* — bad commands can't corrupt history.

**C. The units trap** (I hit this writing this guide — it's real): change
`note_len=2400` to `note_len=0.25`. The bounce succeeds and produces **silence**.
Length parameters are measured in **sample frames**, not seconds — `0.25`
truncates to zero frames, so every note is zero-length. Audio params have units;
a plugin's declared ranges live in its `ParamDef` table (see `note_len` in
`crates/engine/src/plugins/scale.rs`). Silent-but-valid output is always worth
sampling-counting:

```sh
python3 -c "
import struct
d=open('/tmp/hello.wav','rb').read()
print(sum(1 for i in range(44,len(d),4) if struct.unpack('<f',d[i:i+4])[0]!=0.0),'nonzero')"
```

## Where to go next

- To understand *why* the chain looks like triggers→notes→audio, read
  [architecture-explainer.md](architecture-explainer.md) §3 (the patch bay) and
  §4 (the log and the engine).
- To be able to *modify* these crates, work through
  [rust-course/README.md](rust-course/README.md) — nine lessons, each anchored
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
with DeepSeek-V4-Flash · DeepSeek Harness, 2026-09-05.*
