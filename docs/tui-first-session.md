# First session in the terminal shell — arrange a jam in twenty minutes

> 🕒 Last verified against commit `971524b` (2026-09-24). If the code has moved on,
> trust the code and move this line forward.

This is the hands-on tour of **sound-arranger's primary shell**: a terminal
program that arranges recorded audio into a piece and exports a mix. You need a
terminal, the Rust toolchain, and one WAV file (a jam, a take, anything you want
to cut up). Twenty minutes gets you from "I have a recording" to "I have a mix
file I can hand to someone".

Nothing here needs a sound card: without an audio device the transport still runs
and the meters still move in silence, so every step below is checkable on a
laptop with headphones unplugged. You will *hear* it when you plug one in.

If you would rather run a script than press keys, every step below also has a
`host v1` spelling — the shell's command line (`:`) takes the same lines, and
`docs/capabilities.md` lists the vocabulary.

## Before you start

```sh
cd spikes/tui-shell          # the shell is its own workspace
cargo build --release
```

Make a short WAV to play with if you do not have one (a two-note figure, 30
seconds):

```sh
python3 - <<'PY'
import math, struct, wave
sr = 48_000
d = bytearray()
for i in range(sr * 30):
    f = 220 if (i // (sr // 2)) % 2 == 0 else 330
    v = 0.5 * math.sin(2 * math.pi * f * i / sr) * (0.6 + 0.4 * math.sin(2 * math.pi * i / sr / 2))
    d += struct.pack('<h', int(v * 32767))
w = wave.open('/tmp/jam.wav', 'wb')
w.setnchannels(1); w.setsampwidth(2); w.setframerate(sr)
w.writeframes(bytes(d)); w.close()
PY
```

## Step 1 — open it and find your way

```sh
./target/release/tui-shell --wave /tmp/jam.wav
```

The screen has three panels — the **mixer console** on the right, the
**timeline** in the middle, the **pool** under it — and a status line at the
bottom. `Tab` cycles which panel has focus (the keys below marked *timeline* only
work there).

| what | key |
|---|---|
| move the playhead / seek | `,` `.` (seconds), `[` `]` (grid lines), click a lane or the ruler |
| play / stop | space |
| zoom | `+` / `-`, `0` fits the whole piece |
| arm the snap grid | `b` cycles **off → bar → beat → 1/2 → 1/4** |
| the full keymap | `?` (scrolls with `j`/`k`) |
| mouse capture | `m` (off by default: your terminal keeps its own copy/paste) |
| leave | `q` — and **`Esc` never quits**, it only cancels |

Spend a minute here: press `b` twice so the ruler shows bar numbers, then `[` and
`]` to step the playhead a grid line at a time. The frame an edit lands on is
quantized through the session's tempo map, so an edit lands on the grid you can
see.

## Step 2 — cut it up

The clip is the thing you cut, and *one gesture is one undo* (`u` / `Ctrl+r`).

| what | key |
|---|---|
| split at the playhead | `x` |
| select a range (visual mode) | `v`, move, `Esc` leaves it |
| delete the selection or the clip under the playhead | `d` |
| trim the clip's start / end to the playhead | `<` / `>` |
| trim to the selection | `t` |
| nudge a clip a grid step left / right | `H` / `L` |
| move a clip to the track below / above | `J` / `K` |
| clip gain | `g` / `G` (±1 dB) |
| a fade in / out ending at the playhead | `f` / `F` |

Cut your jam into an intro and a couple of sections. Nothing is destructive:
every cut is a line in the session log, and the pool source is never rewritten.

## Step 3 — keep the pieces, add tracks

| what | key |
|---|---|
| copy / cut the selection (or the clip under the playhead) | `y` / `c` |
| paste at the playhead / **append** after the last clip | `p` / `P` |
| add a track (the first free `t{n}`) | `a` |
| rename the active track (opens the command line prefilled) | `R` |
| delete a track **and its clips** as one gesture | `D` |
| move the active track up / down | `{` / `}` |
| place a pool source at the playhead | `Tab` to the pool, `j`/`k`, `Enter` |

A paste is one `group` of clips with 64-frame micro-fades at new boundaries, so
it is one undo step and it does not click. Appending (`P`) is how a piece grows:
it follows the track's own last clip (or the whole arrangement's end if the track
is empty).

## Step 4 — shape the take

One key each, one undo each:

| what | key |
|---|---|
| reverse (a clip property: the reader plays backwards) | `V` |
| normalize to the clip's peak | `U` |
| invert polarity | `i` |
| silence (a gain of zero — the material stays) | `E` |
| trim away the silence at both ends | `T` |

## Step 5 — make it fit the tempo

A take recorded at 90 bpm has to sit in a 120 bpm session. Two facts are enough:

1. Tell the session what the take was performed at, once:
   `: source_tempo jam 90` (the pool source id is the name in the pool panel).
2. Put the playhead on the clip and press **`W`** (warp).

The shell renders the clip's material through an offline WSOLA stretch (it keeps
the *pitch* and changes the *length*), writes it into the pool as a new source,
and points the clip at it: one render, one logged `arrange stretch`, one undo.
The status reports the ratio and the render time.

Without a recorded source tempo, `W` does not guess — it says
`no tempo recorded for source jam — set it with `: source_tempo jam <bpm>``.

## Step 6 — name the piece

| what | key |
|---|---|
| name a marker at the playhead | `'` (the command line opens prefilled; type one word, Enter) |
| jump to the next / previous marker | `;` / `"` (lands *exactly* on it) |
| name the clip under the playhead | `C` |

Markers are drawn on the ruler (`▼` and their name) and the readout names the
section the playhead is standing in. They are part of the session document:
logged, saved, and completely silent — a mix renders the same bytes with or
without them.

## Step 7 — master it and export

The mastering chain is a plugin on the mix bus. Type these at the command line
(`:`), in this order:

```text
: mount master
: patch mixer.audio master.audio
: set_param master threshold -18
: set_param master ratio 4
: set_param master ceiling -3
```

That is a stereo compressor into a lookahead brickwall limiter. `set_param
master ratio 1` makes the compressor exactly transparent (the limiter still
holds the ceiling). The console's readout then shows the chain is live.

Now export:

```text
: X
```

(`X` opens the command line prefilled with `export <dir>/mix.wav f32` — edit the
path, or add `s16` for a dithered 16-bit file, then Enter.)

- **f32 is the default**: the mix exactly as rendered, bit-exact.
- **`s16`** adds fixed-seed TPDF dither, so the file is reproducible *and* the
  quantisation error is decorrelated from the signal.
- The length is the arrangement's own — no frame count to compute — and the
  render starts at frame 0 whatever the playhead was doing.
- The status reports `exported N frames (f32, +M tail) — peak −x.x dBFS, rms −y.y
  dBFS`.
- **If the mix would clip, nothing is written.** The refusal names the peak and
  the fix; that is what the limiter above is for.

## Step 8 — keep it, and come back

Sessions are directories: the log as text plus the pool.

```sh
./target/release/tui-shell --script /tmp/saved/session.txt
```

The shell autosaves as you edit when the session lives in a directory (the
journal is appended per gesture, so a crash costs at most the gesture in
flight). Reopening replays the log; the pool comes with it.

Two properties worth knowing:

- **Replay is byte-identical.** The same session exports to the same bytes,
  twice, on any machine (the dither's seed is a constant, not entropy).
- **Seeking in a long piece is fast.** A jump renders a one-second run-in rather
  than the timeline from zero: on a 30-minute, four-track arrangement that is
  0.10 s instead of 10.6 s, and the bytes are proved equal to a full replay
  (`crates/host/src/lib.rs`, `a_warm_seek_equals_a_replay`).

## Break it three ways on purpose

1. **An export that would clip.** Raise a clip's gain until the mix exceeds full
   scale (`G` a few times), then `X` and Enter. Nothing is written, and the
   refusal says `export would clip: the mix peaks at 1.4x (−1.x dBFS) — lower the
   mix or mount a master chain with a ceiling; the file was not written`.
2. **A warp with no source tempo.** `W` on a fresh import says
   `no tempo recorded for source … — set it with \`: source_tempo … <bpm>\``.
   Refusals are loud and specific; a silent wrong answer is the failure mode this
   project spends its tests on.
3. **A paste with nothing copied.** `p` says
   `the clipboard is empty — \`y\` copies a clip or a selection`.

## Where to go next

- [capabilities.md](capabilities.md) — what the alpha does, and what it
  deliberately does not (with the reason).
- [beta-acceptance.md](beta-acceptance.md) — the checklist a *second* person
  runs from these docs alone, no author help.
- `?` in the shell — the full keymap, generated from the shared table both
  shells read.
- [FIRST_SESSION.md](FIRST_SESSION.md) — the same tour for the **headless host**
  (`host v1` scripts, no UI): the engine's patch bay and the log-is-the-document
  promise.
- [clip-arranger.md](clip-arranger.md) and
  [architecture-explainer.md](architecture-explainer.md) — why the arrangement
  value, the ops and the render node look the way they do.

---

*Authored with DeepSeek-V4-Flash · DeepSeek Harness, 2026-09-24. Every command and
key in this document was run against the commit in the banner; the terminal path
is re-checked by `spikes/tui-shell/scripts/pty-check.sh`, which drives these keys
through a real pty.*
