# Bol Processor BP2 / BP3 — Music Composition and Improvisation Software

## Overview

Bol Processor is a program for music composition and improvisation. It produces polyphonic music from a set of rules (a compositional grammar) or from text scores that can be typed in or captured from a MIDI instrument. The software supports real-time MIDI output, MIDI file generation, Csound score output, and text-based music representation.

The project spans over four decades. The earliest version (BP1) ran on an Apple II computer in 1981. The current version (BP3) is a multiplatform C-language console engine that runs on macOS, Linux, Windows, and WebAssembly (WASM). BP2, the Macintosh era version that followed BP1, won the **1997 Bourges International Prize** (ex aequo with Cecilia) in the category of computer-aided composition and realization software.

The Bol Processor is not a conventional sequencer or DAW. Its core contribution is a formal framework for music representation based on **polymetric structures**, **sound-objects**, **time-objects**, and **generative grammars**. These concepts emerged from ethnomusicological research into North Indian tabla drumming and generalize to any musical style.

## How It Works

### Compositional Grammars

At its heart, Bol Processor uses **formal grammars** — the same kind of rewrite rules that computer scientists use to define programming languages — to describe musical structure. A grammar contains rules like:

```
A --> B C
B --> D E
```

Each symbol in the grammar represents a musical unit. The system starts from a start symbol and applies rules to expand it into a sequence of **terminal symbols**, which correspond to playable sound-objects. Rules can include:

- **Context conditions**: a rule only applies when certain symbols appear before or after it
- **Weights**: stochastic control of rule selection for probabilistic variation
- **Flags**: numeric or logical variables that gate rule eligibility
- **Homomorphisms**: structure-preserving transformations of terminal symbols
- **Pattern markers**: structural bookmarks for repetition and transformation

The grammar formalism is more powerful than context-free grammars (Type 2 in the Chomsky hierarchy). It supports context-sensitive rules (Type 1) and includes BP's own **pattern grammar** extensions for repetitions and structured transformations.

### Polymetric Structures

Polymetric expressions are BP's foundational model for representing musical time. The term blends "polyphony" and "polyrhythm." A single polymetric expression can represent an entire musical work.

The key operators are:

- **Superimposition** `{field1, field2}` — events in field1 and field2 happen simultaneously; the total duration is set by field1
- **Consecutive** `field1 . field2` or `{field1 • field2}` — events play one after another
- **Rests** `-` (one unit), `_rest` (undetermined duration)
- **Tied notes** `&` — concatenates across structural boundaries
- **Speed up / slow down** `\` and `/` — tempo scaling within expressions

Undetermined rests are a unique feature: the composer marks a rest whose duration the algorithm computes automatically, chosen to minimise the complexity (least common multiple) of the overall polymetric structure.

### Sound-Objects and Time-Objects

A **sound-object** is a sequence of MIDI messages or Csound instructions. Each sound-object has properties that control how it behaves when placed in time:

| Property | What it controls |
|----------|-----------------|
| Duration | Absolute or scalable (expand/compress) |
| Pivot | The point used for alignment (beginning, end, first NoteOn, etc.) |
| Cover | How much overlapping with neighbours is allowed |
| Gap | Minimum spacing from neighbours |
| Relocation | Whether the object can be shifted to resolve conflicts |
| Pre-roll / Post-roll | Offset before/after the object's nominal time |

A **time-object** is an abstract temporal entity that does not itself produce sound. Time-objects are the atoms of the "smooth time" system — they define a grid of time points with metric and topological properties.

The **time-setting algorithm** is a constraint satisfaction engine. Given a sequence of sound-objects with symbolic timings (beats, ratios), it computes the physical timing (milliseconds) while respecting each object's constraints on covering, gap, relocation, and pivot alignment. If conflicts arise, the algorithm attempts resolution by shifting objects within their allowed ranges. If no solution exists, it reports the conflict.

### Symbolic-Numeric Quantization

BP uses a **rational number** representation for symbolic time. Beats are expressed as integer ratios, not floating-point approximations. This gives absolute accuracy for complex polymetric structures (e.g., 7:8 against 5:4). When the rational representation becomes too large (LCM explosion), the system quantizes at the symbolic level by simplifying ratios based on a user-supplied tolerance.

### Live Coding and Real-Time Interaction

BP3 gained live coding capabilities in March 2025. The user can modify grammar rules while MIDI output is playing, and changes take effect immediately. Combined with real-time MIDI input capture, interactive derivation mode, and adaptive rule-weight learning, BP supports a fluid improvisational workflow.

### Output Formats

Bol Processor can direct its output to:

- **Real-time MIDI** — via PortMidi or native MIDI drivers on macOS, Linux, and Windows
- **MIDI files** — standard format 0 or 1
- **Csound scores** — with full microtonal tuning support
- **MusicXML** — import entire scores as polymetric structures
- **Text/HTML** — human-readable score notation

## Historical Background and Research Origins

### The Tabla Problem

In 1981, ethnomusicologist **Jim Kippen** and computer scientist **Bernard Bel** met in Mussoorie, India. Kippen was studying North Indian tabla drumming under Ustad Afaq Husain Khan of the Lucknow gharana. He wanted to understand the implicit rules that governed improvisation.

Tabla music uses an oral notation of mnemonic syllables called **bols** — onomatopoeic representations of drum strokes: *dha, ti, ge, na, tin, ta, dhin*. A *qāida* (meaning "rule") is a theme-and-variation form. The musician states a theme and then improvises variations that conform to the underlying grammatical rules of the form.

The problem was that these rules were **implicit** — available only through the musician's ability to play correct sequences and recognize incorrect ones. No written grammar existed. Kippen and Bel proposed that formal grammars from computer science could model this knowledge.

### The Dialectical Method

Their research method became a three-way dialectic:

1. **Bel** created a basic grammar for a given qāida
2. **BP1** generated variations using that grammar
3. **Kippen** read the variations aloud (using bols) to **Afaq Hussain**
4. The master validated, corrected, or rejected them
5. Afaq Hussain offered his own variations
6. These were analysed against the grammar
7. Rules were adjusted iteratively

This was a concrete application of John Blacking's concept of "socio-musical grammars" — the idea that musical competence, like linguistic competence, rests on a system of internalised rules that can be modelled formally.

### BP1 on the Apple II (1981–1985)

BP1 ran on an Apple II with 64KB of RAM (later the portable Apple IIc with 128KB). It was an expert system that could:

- **Synthesize** new variations using modus ponens (rule application)
- **Analyze** musician-provided variations using a deterministic membership test
- **Learn** rule weights from examples — accepted variations incremented the weight of rules used in their derivation, making the stochastic model converge toward aesthetically acceptable output
- **Handle patterns** — structural markers that captured repetitive and transformational features of tabla compositions

The hardware constraints were extreme. Afaq Hussain marvelled that "a machine could think." The portable Apple IIc proved essential — Bel could bring it directly to locations in India for interactive sessions with the master.

### The ISTAR Project

BP1 formed part of a larger research effort by the **International Society for Traditional Arts Research (ISTAR)** in India. The project included acoustic analysis of tonal structures in raga, which later contributed to BP's sophisticated **microtonality** system — one of the first software environments to handle just intonation and arbitrary tuning systems without preconceptions.

### From BP1 to BP2 (late 1980s–1996)

As the Macintosh platform matured, Bel rebuilt Bol Processor from scratch in C. BP2 introduced:

- **Polymetric structures** — the core innovation, enabling polyphonic/polyrhythmic composition
- **Sound-object model** — parameterised containers for MIDI events with constraint-based placement
- **Time-setting algorithm** — constraint satisfaction for aligning sound-objects in time
- **Real-time MIDI** — direct output to synthesizers
- **Csound interface** — high-quality audio synthesis via Csound scores
- **Text-oriented representation** — scores as editable text with multiple abstraction levels

The theoretical foundation was laid in Bel's 1990 PhD thesis, *Acquisition et représentation de connaissances en musique* (Aix-Marseille III), which explored knowledge acquisition, formal grammars for music, inductive inference of regular languages, and the symbolic-numeric approach to musical time.

### The Bourges Prize (1997)

BP2 won the 1997 Bourges International Prize (ex aequo with Cecilia by Jean Piché and Robert Burton) — a prestigious award from the International Institute for Electroacoustic Music of Bourges (IMEB). The award recognised BP2's contribution to computer-aided composition.

### Open Source and Multiplatform Era (2006–present)

In spring 2006, **Anthony Kozar** joined the project and made BP2 open source under a BSD license. He completed the Mac OS X port in June 2007. BP 2.9.8 was the final vintage Mac release (2012), supporting Mac OS X 10.4 through 10.14 (Mojave).

In 2020, Kozar introduced **BP3** — a complete rewrite of the interface architecture. The C-language console engine separates computation from presentation. A PHP/HTML/JavaScript frontend provides the graphical interface, running on a local Apache server (MAMP or XAMPP). This architecture made BP3 truly cross-platform: macOS, Linux, and Windows.

In 2026, **Romain Peyrichou** began integrating BP3 with WebAssembly via Emscripten. His **BP2SC** project transpiles BP grammars into SuperCollider patterns, and his **BPscript** language provides a modern syntax for BP3. The WASM port enables BP3 to run entirely in a web browser.

## People

### Bernard Bel
Computer scientist at CNRS (French National Centre for Scientific Research), based in Aix-en-Provence and formerly deputed to the Centre de Sciences Humaines in New Delhi. Creator of the Bol Processor from 1981 onward. Also created the Speech Prosody Special Interest Group and the Speech & Language Data Repository. Now retired, maintains BP3, and manages a database of 110,000 grindmill songs from rural Maharashtra.

### Jim Kippen
Ethnomusicologist, former head of ethnomusicology at the University of Toronto. Studied under John Blacking and John Baily at Queen's University Belfast. Master of several Indo-Persian languages. Research areas include tabla, the evolution of North Indian rhythm, and historical notation systems.

### Ustad Afaq Husain Khan
The tabla maestro of the Lucknow gharana who served as the domain expert for the BP1 experiments. His willingness to subject his knowledge to computational modelling made the project possible.

### Srikumar Karaikudi Subramanian
Collaborator on the BP2 theoretical framework, creating complex Carnatic music examples and pseudo-Carnatic style compositions using BP2's numeric/logic flag system.

### Anthony Kozar
Open-sourced BP2 in 2006, ported it to Mac OS X, and later designed the BP3 multiplatform architecture with the C console engine and PHP frontend. Developed the standalone macOS application.

### Romain Peyrichou
Current contributor porting BP3 to WebAssembly. Creator of BP2SC (BP → SuperCollider transpiler) and BPscript (modern BP language). Extensive article series on BP3 internals at roomi-fields.com.

## Influence

The Bol Processor has been "heavily influential on the design of the TidalCycles system" (Alan Blackwell et al., *Live Coding: A User's Manual*, MIT Press 2022, p. 195). Alex McLean, creator of TidalCycles, has cited BP's rhythm representation — particularly the cycle-based organisation of events and its polymetric/polyrhythmic structures — as direct inspiration. TidalCycles' "mininotation" language for describing rhythm grew out of BP's pattern notation for tabla bols.

The broader influence includes the representation of music based on cycle durations rather than event durations — a conception drawn from Indian classical music that contrasts with Western staff notation. BP's time-object model has also been noted as applicable beyond music, to domains like video clip scheduling, robot command sequencing, and any field requiring precise temporal organisation of events.

## The Theoretical Framework

Bol Processor makes several distinctive theoretical commitments:

1. **Grammars are generative and analytic.** A grammar can both produce new music and test whether a given sequence conforms to its rules. This mirrors the dual competence of human musicians.

2. **Time is symbolic before it is physical.** Durations are represented as integer ratios, not milliseconds. Physical time is a computed mapping from symbolic structure.

3. **Polymetric structure is fundamental.** Music is not a linear sequence of events but a hierarchy of superimposed and consecutive temporal fields.

4. **Sound-objects are parameterised.** Every sound-object carries constraints (cover, gap, pivot, expandability) that determine how it fits into the temporal structure.

5. **Microtonality is a first-class concern.** The system does not assume twelve-tone equal temperament. Tuning can be defined arbitrarily — just intonation, historical temperaments, or custom scales derived from acoustic analysis of raga performances.

## A Partial Chronology

| Year | Milestone |
|------|-----------|
| 1979 | Bernard Bel develops the first real-time Melodic Movement Analyzer for raga intonation |
| 1981 | BP1 created, Apple II; Bel and Kippen meet in Mussoorie |
| 1981–1985 | BP1 used for tabla modelling experiments with Afaq Hussain |
| 1985 | Apple IIc portable version deployed in India |
| Late 1980s | BP2 development begins on Macintosh |
| 1990 | Bel's PhD thesis published |
| 1994 | CRONOS choreographic work (Andréine Bel) using BP2 |
| 1996 | BP2 reference manual published |
| 1997 | Bourges International Prize (ex aequo) |
| 2006 | Open-sourced on SourceForge by Anthony Kozar |
| 2007 | Mac OS X port completed |
| 2012 | BP 2.9.8 final vintage release |
| 2020 | BP3 multiplatform release (C console + PHP frontend) |
| 2025 | Live coding capabilities added to BP3 |
| 2026 | WASM port by Romain Peyrichou; BP2SC transpiler |

## Resources

- **Website:** [bolprocessor.org](https://bolprocessor.org/)
- **Source code:** [github.com/bolprocessor](https://github.com/bolprocessor)
- **Reference manual:** [Reference manual BP2.9.8](https://bolprocessor.org/reference-manual-bp2-9-8/)
- **Tutorials and examples:** [bolprocessor.org tutorials](https://bolprocessor.org/category/tutorials/)
- **Publications:** [bolprocessor.org/publications/](https://bolprocessor.org/publications/)
- **Romain Peyrichou's BP3 articles:** [roomi-fields.com](https://roomi-fields.com/en/articles/index-2/)
- **Authors' contact:** [contact@bolprocessor.org](mailto:contact@bolprocessor.org)
