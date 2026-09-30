# Beta acceptance — the checklist

> 🕒 Last verified against commit `2f6ad78` (2026-09-30 — §5's crash-recovery items
> re-verified against the shipped host; the rest checked 2026-09-24). If the code
> has moved on, trust the code and move this line forward.

This is the checklist a **second person** runs to accept the alpha. The point of
it is independence: everything below is doable from
[tui-first-session.md](tui-first-session.md) and
[capabilities.md](capabilities.md) alone, with your own audio files, no help from
the author, and no knowledge of the code.

Run it top to bottom. Each item states **what to do**, **what you should see**,
and **how to tell it failed**. Copy the checklist into a file and tick as you go;
if an item fails, stop and capture the evidence (see the last section) — a failed
acceptance run is more valuable than a passed one.

Time: about an hour, plus however long you spend listening.

## 0. You are the right person to run this

- [ ] You have never used this tool before, and you have not read the code.
- [ ] You have your own audio: at least one WAV you recorded, **plus one at a
      different sample rate** (e.g. 44.1 kHz) and, if you have one, a stereo or
      24-bit file.
- [ ] You have twenty minutes to read
      [tui-first-session.md](tui-first-session.md) before starting.

## 1. It opens and it makes sense

- [ ] `cargo build --release` in `spikes/tui-shell` finishes without errors.
- [ ] `./target/release/tui-shell --wave <your file>.wav` opens a screen with a
      mixer console, a timeline, a pool panel and a status line, and the file's
      name appears somewhere.
- [ ] Pressing `?` shows a keymap you can scroll; pressing `Esc` closes it and
      does **not** quit.
- [ ] Pressing space starts a moving playhead and moving meters. *Failure looks
      like*: a frozen playhead, or a panic message.
- [ ] **A different sample rate works**: open your 44.1 kHz file. It appears in
      the pool with the session's rate (48 kHz), and its pitch sounds right when
      you play it. *Failure looks like*: a refused import, or audio that plays
      fast/slow.
- [ ] A **stereo** file appears as two pool sources (`….ch0`, `….ch1`) and, when
      placed, they play on opposite sides. A **24-bit** file imports without
      complaint.

## 2. Cut, move, undo — one gesture is one undo

- [ ] Split a clip (`x`), and the waveform shows two clips.
- [ ] `u` restores it; `Ctrl+r` redoes it.
- [ ] Copy (`y`) and paste (`p`) a clip; **one** `u` removes the whole paste
      (not one piece of it).
- [ ] Append (`P`) after the track's last clip; the piece gets longer.
- [ ] Add a track (`a`), name it (`R`, type a word, Enter), move a clip to it
      (`J`), delete the track (`D`) — and **one** `u` brings back both the track
      and its clips.
- [ ] Arm the grid with `b` until the ruler shows bar numbers, then `[`/`]`: the
      playhead lands on grid lines, and an edit made with the grid armed lands on
      one too.
- [ ] Reverse a clip (`V`): the envelope mirrors. Normalize (`U`), invert (`i`),
      silence (`E`), trim to content (`T`) each change the picture and each undo
      in one step.

## 3. Tempo match

- [ ] `: source_tempo <source-id> <bpm>` with the tempo you actually played the
      take at, then `W` on a clip of it: the status reports a ratio and a render
      time, and the clip gets shorter (or longer) **without changing pitch**.
- [ ] Pressing `W` on a source with no recorded tempo prints a message naming the
      command to set one. *This is a pass*: a guess would be a silent wrong
      answer.

## 4. Markers, names, navigation

- [ ] `'` opens the command line prefilled; type a one-word name and press Enter.
      A `▼` and the name appear on the ruler, and the readout names the section
      when the playhead is on it.
- [ ] `;` and `"` jump to the next/previous marker, landing exactly on it.
- [ ] `C` names the clip under the playhead; the label is drawn on the lane.
- [ ] Adding, renaming and removing markers changes **nothing about the audio**:
      export before and after (step 6) and compare the files.

## 5. Nothing is lost (the part that matters most)

- [ ] `: save <dir>` (or work in a directory the shell autosaves to), then quit
      and reopen the session. Everything is back: clips, names, markers, the
      tempo, the pool.
- [ ] **Crash recovery**: with the session autosaving, make an edit and
      `kill -9` the shell. Reopen with **`: load <dir>`** (which replays the
      journal; `--script <file>` reads only the saved baseline); the edit you made
      is there (at worst, the very last gesture is missing), and a torn last line
      costs nothing — the session opens without it. (The host counts what was
      torn in its recovery record; no shell prints that count yet — the walkthrough
      with a simulated crash is [FIRST_SESSION.md](FIRST_SESSION.md) Step 5.)
- [ ] Re-saving does not grow the file: save, reopen, save again, and
      `session.txt` is the same length (and the same bytes) as after the first
      save.
- [ ] The saved `session.txt` is readable: a header, then one line per gesture.
      Reading it, you can tell what you did.

## 6. The mix: master it, export it, hand it over

- [ ] `: mount master`, `: patch mixer.audio master.audio`, then
      `: set_param master ceiling -3` (and optionally `threshold`/`ratio`).
- [ ] `X` opens the command line prefilled with an `export … f32` line; Enter
      runs it.
- [ ] The status reports the file's length, the format and the **peak and RMS**.
- [ ] The exported file plays in a player you did not write, and it is the piece:
      it starts at the beginning, it ends where the arrangement ends, and it is
      not silent.
- [ ] **It does not clip**: no sample is at full scale on material that was not
      already there. Push the mix too hot (raise a clip gain a few dB) and export
      again: the export is **refused**, it says why, and **no file is written**
      (or, if one existed, it is unchanged).
- [ ] **`s16` works**: append `s16` to the prefill and export; the file is
      16-bit, and it sounds like the mix (a dithered noise floor, no gritty
      distortion on fades).
- [ ] **Byte-reproducible**: export the same session twice (to two paths) and
      `cmp` them — identical, f32 and s16 alike.
- [ ] **You can hand the file to someone else**: send it to a person with no
      context and no software from this project. It plays.

## 7. Long material (only if you have a long piece)

- [ ] With a piece of ten minutes or more, jump to its end (`]` many times, or
      `: transport seek <frame>`). It should feel immediate (well under a
      second), not a freeze.
- [ ] If it *is* a freeze: note whether the session has a mid-piece
      `mount`/`set_param` (state placed inside the seek's one-second run-in makes
      the jump fall back to a full render — correct, and slower). Report it
      either way.

## 8. The other shell (optional, but part of the claim)

- [ ] `cd spikes/iced-shell && cargo run` opens a window with the same console.
- [ ] Keys that the iced canvas cannot render yet report *what* they need rather
      than doing nothing: press `x` and read the status line.
- [ ] The workflow table is the same one the terminal shell reads (`?` in both).

## 9. What "pass" means

The alpha is accepted when:

- every box in sections 1–6 is ticked on a machine that is not the author's;
- no step required reading the source code to complete;
- the only refusals you saw were **explained** (a message naming the missing
  fact), never silent;
- the exported file is something you would send to another person.

Sections 7 and 8 are informative: a failure there is a bug to report, not a
blocker for the alpha's own criteria.

## Reporting a failure

Capture, in this order — the first two are what make a report actionable:

1. **What you did** (the keys or `:` lines, in order) and **what happened**
   (the status line's exact text, or a screenshot/copy of the screen).
2. **The session directory** (`session.txt` + `journal.txt`): the log is the
   document, so it *is* the bug report. Copy it somewhere safe before touching
   the session again.
3. The commit the shell was built from (`git rev-parse HEAD` in the repo).
4. Your terminal, its size, and whether an audio device was present.

If the failure is "the same input produced different output" (an export that
differs from a previous one, a replay that does not match), that is the highest
severity class this project has: include both files and both `session.txt`s.

---

*Authored with DeepSeek-V4-Flash · DeepSeek Harness, 2026-09-24; §5's
crash-recovery items re-verified against `2f6ad78` with GLM-5.3 · OpenCode,
2026-09-30.*
