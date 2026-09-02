# Autechre — generative composition, "the studio as an instrument", and Macero from a 2000s perspective

Prior-art / design-inspiration study, 2026-09-01. Documentation and interviews read; no source code or sessions. All claims cited. This is research for the sound-arranger thesis — the section on Macero (§6) is the owner's framing, developed here into an explicit argument; the rest is reported from primary interviews and encyclopedic sources.

---

## 1. Who they are

**Autechre** (pronounced /ɔːˈtɛkər/) are an English electronic duo — **Sean Booth** and **Rob Brown** — both from Rochdale, Greater Manchester, formed in **1987**. They are among the best-known acts on **Warp Records**, through which all their albums have been released, beginning with *Incunabula* (1993). ([Wikipedia — Autechre](https://en.wikipedia.org/wiki/Autechre))

- **Origin:** they met in 1987 through Manchester's graffiti/breakdance scene, shaped by electro, hip-hop and acid house. They started by exchanging cassette mixes and making tracks on cheap gear — a **Casio SK-1 sampler** and a **Roland TR-606 drum machine**. ([Wikipedia — History](https://en.wikipedia.org/wiki/Autechre))
- **Name:** Booth says it came from working on an Atari — "'au' for the sound, then the rest bashed randomly on the keyboard"; they then used it because it "looked good" on a cassette. ([Sound On Sound, Apr 2004](https://web.archive.org/web/20150924120348/http://www.soundonsound.com/sos/apr04/articles/autechre.htm))
- **Classic statement of intent:** neither played instruments in the traditional sense. Booth's background was **"pause-button edit mixes on compact cassettes"** — the *editing* of recorded material as the primary creative act, before any notion of "owning" a song. This is the thread that makes them relevant to an arranger. ([SOS 2004](https://web.archive.org/web/20150924120348/http://www.soundonsound.com/sos/apr04/articles/autechre.htm))
- Early: first release a self-titled 12-inch as **Lego Feet** (Skam); first Autechre single **"Cavity Job"** (1991). Signed to Warp in 1992 (contributed "Crystel" and "The Egg" to the *Artificial Intelligence* compilation). ([Wikipedia](https://en.wikipedia.org/wiki/Autechre))
- Booth is based in Suffolk, Brown in London; each has a studio with **largely identical gear** (Booth's slightly bigger). They often work separately and share everything. (SOS 2004)

## 2. The style, and the discography arc

Booth has dismissed the "IDM" label as "silly," and their work is described as evolving *"from early, melodic techno recordings to later works often considered abstract and experimental, featuring complex composition and few stylistic conventions."* ([Wikipedia](https://en.wikipedia.org/wiki/Autechre))

Approximate era arc (Wikipedia discography + SOS 2004):
- **1993** *Incunabula* (Booth: "more of a compilation of old material"); **1994** *Amber* ("the first album we put out on Warp"), *Basscadet Mixes*, and the **Anti EP** — a protest against the 1994 Criminal Justice Bill; the piece "Flutter" was *programmed so no bar contained identical beats*, and the sleeve advised DJs to have "a lawyer and a musicologist" present. This is early, explicit evidence of **rhythm as an authored, rule-based construct**. ([Wikipedia](https://en.wikipedia.org/wiki/Autechre))
- **1995** *Tri Repetae* + *Garbage* / *Anvil Vapre* ("Second Bad Vilbel" video by Chris Cunningham). **1997** *Chiastic Slide* (often called the pivot from beat-centred to textural/abstract), *Envane*, *Cichlisuite*. **1998** *LP5* (untitled). **1999** *Peel Session*, *EP7*. **2000** *Peel Session 2*. ([Wikipedia](https://en.wikipedia.org/wiki/Autechre))
- **2001** *Confield* — the big turn. *Pitchfork* called it "one of the duo's most abstract and difficult releases… ordered soundscapes built from textural, dissonant and atonal repetition." Booth: *"much of Confield grew out of experiments with [Max] that were not suited to a club environment."* ([Wikipedia — Confield](https://en.wikipedia.org/wiki/Autechre))
- **2003** *Draft 7:30* — Booth: *"Draft is really straight, using straight-up normal sequencers and samplers. It's written note by note… Only 'Reniform Puls' has some generative stuff, done by Max."* (SOS 2004)
- **2005** *Untilted*; **2008** *Quaristice* (+ *Versions*); **2010** *Oversteps* + *Move of Ten*; **2013** *Exai*. Then **2016** *elseq 1–5* and the live-series **AE_LIVE**; **2018** *NTS Sessions 1–4*; **2020** *SIGN* and *PLUS*. In **2019** the *Warp Tapes 89-93* archive was broadcast on NTS. In **2024** Autechre were announced as **BBC Radio 6 artists-in-residence**. ([Wikipedia](https://en.wikipedia.org/wiki/Autechre))

Also: **Gescom** — their collective for dancefloor-oriented material, plus extensive remix work. (SOS 2004)

## 3. The core idea: "the studio as an instrument"

The 2004 SOS piece frames it at the top: *"In producing their complex, abstract electronica, Autechre have taken the idea of the studio as an instrument to new extremes."* ([SOS 2004](https://web.archive.org/web/20150924120348/http://www.soundonsound.com/sos/apr04/articles/autechre.htm))

- **No centrepiece.** *"Because of their hunger for exploring different pieces of gear… there's no centrepiece in their studio that dictates their method of working."* (SOS 2004) Contrast with the typical "pick one DAW and master it" approach — Booth calls the opposite stance being *"a bit of a mutant."*
- **Connectivity is the point.** *"A lot of the time we have the studio set up a certain way for one track, and then we have to completely rewire it for the next track. That's mostly what we're doing: putting the studio together in a certain way for each track, and I guess that when we saw Max and later MSP it was exciting. It mirrored the way we used to think about stuff. It was all about connectivity."* (SOS 2004)
- **Engineering as beauty.** Booth went to a 6-month audio engineering/electronics school but found it *"not exciting… I didn't want to learn how to mike up a drum kit, I wanted to know how to use the studio as an instrument. It was the opposite angle."* And: *"the idea of engineering being beautiful… Constructing harmony from a load of predefined frequencies is essentially no different [from building a bridge from metal girders]. To me it's all construction, building."* (SOS 2004)
- **Software-as-instrument-builder.** Because they were always modifying/building hardware, *"Cycling 74's Max/MSP allows them to take the same creative approach in software, designing and implementing their own controllers, connections and sound generators."* (SOS 2004) This is the key move for taste-work: **the instrument is something you write**, and its "patch" is a value you author.

## 4. The generative / Max-MSP layer (the owner's specific interest)

### 4.1 The algorithm is the song

In a 2022 AMA session (summarised by Mark Hurrell): *"Each song is effectively a self-contained algorithm (MaxMSP patch), with its own set of behaviours, inputs and data. Because they behave in unexpected ways every performance of the algorithm is unique — the same algorithm might create a totally different piece of music when recorded for an album compared to performing for thousands."* And *"since the late 90s most of their music has been composed as algorithms (frequently using software called MaxMSP). The notes and rhythms are set up as data sequences with different inputs and variables, that then play out and evolve in unexpected ways."* ([Mark Hurrell, 2022](https://markhurrell.com/prospects/autechre-and-responsible-algorithms/), citing Booth's Twitch AMA)

The 2015 Zaldua interview likewise notes they discussed *"writing music and algorithms with Cycling '74's Max."* ([Words With: Autechre, 2015](https://certainsound.net/words-with-autechre-2015-1/))

### 4.2 The method (from Booth, in his own words)

The most detailed public description is Booth on *Confield*/*Draft 7:30* (SOS 2004):

> "When we do generative stuff we work with real-time manipulation of MIDI faders that determines what the rhythms sound like. A sequencer is spitting out stuff and we're using our ears and the faders to make the music. There's no event generation taking place other than within the system we've designed. Sometimes we'll stripe a whole load of stuff down as MIDI data, because there may be a couple of things we want to change. We generate these beats in Max and with home-made sequencers. And there are models of analogue sequencers in the computer that are doing manipulation like gating and compressing some of the beats."

So the generative process is **you write the system, then you *play* it** — a fader and your ears drive the emergent rhythm. This is the "jam": not with a band, but with a system whose behaviour you authored and then steer.

### 4.3 "It's not random" — the crucial distinction

Autechre are emphatic that this is **not** random, and the distinction is central to understanding them:

- SOS 2004, on so-called "random beats": *"There's a lot of maths and generated beats on Confield… even when the beats sound like they are moving around in time and space, they're not random. They're based on sets of rules and we have a good handle on them."*
- The method: *"We don't use random operators because they're irritating to work with — every time you run the process it sounds different. How we play the system dictates how the system responds."* (SOS 2004)
- Booth, 2001 (Reynolds interview): *"For a start, the word 'random' — it takes the shit right out of me. There's absolutely nothing random about what we do."* And: *"I don't use random number generators — I fucking hate em. They're rubbish. I use a few chaotic operators."* ([Grooves / Reynolds, Sep 2001](https://web.archive.org/all/20060721134153/http://www.sas.upenn.edu/~reynolda/music_ae_092801.html))
- And, deeply: *"all music is generative. Any music. As long as there's a rule and there's a determination in terms of process and you've got an algorithm. Any music can be broken down like that."* (Reynolds 2001) — i.e. "generative" is not a special category; everything reduces to rule+process. The algorithm just gets more or less complex.

### 4.4 Analog/mechanical analogues

They don't only do this in Max — the *Confield* era mixed software with patched analog gear:

> "On Confield we also used analogue sequencers and drum machines, because you can do a lot with restarting patterns. You can hack things and maybe use a control volume to determine what step the drum machine is playing from… you send that control volume from an analogue sequencer, so the drum machine is skipping around. And then you get another analogue sequencer to drive that analogue sequencer with a different timing. Immediately you have something that some people would call random, but I would say is quantifiable." (SOS 2004)

This is a beautiful, concrete illustration of the design: **recursive, cross-modulating systems** where one sequencer drives another's step position, gating/compressing beats — a classic propagation-of-state machine. And it maps directly to the project's own **graph interpreter / patch-bay** abstraction.

### 4.5 Studio vs live: recursive "research" versions vs "responsible" versions

The 2022 AMA (via Hurrell) gives the modern shape:

- *"they actually create different versions of each algorithm for the studio and for live performance. The studio versions have more recursive behaviours (the generated sounds, melodies and beats are then reused as data inputs to create new sounds, melodies and beats) whereas the live versions depend much more on human intervention from himself and Rob on stage."*
- *"The studio versions are great for research, you can record them playing all day, isolate the good sections, pull apart how they work and learn from the process. It doesn't really matter if it doesn't produce anything good for days on end as long as you learn from it."*
- Live: *"when performing live you've got a responsibility to your audience… to nudge the algorithm into giving everyone a good time."* (Hurrell 2022)

Hurrell's read is apt and worth borrowing: it's *"ad-hoc implementations of things like versioning, branching, containerisation, being able to play back outputs to understand their behaviour"* — with distinct **research** (studio) vs **responsible** (live) builds. This is literally a two-profile workflow, and a strong precedent for a "profile" concept.

### 4.6 Releasing the system, not just the track

- Booth, 2001: *"We've already done a couple of releases of recordings of systems that generate recordings. I think our first Fals.ch release came out about 18 months ago… It's exactly like releasing every version of a track we'd do. Instead of doing eight versions of a track… a lot of them are just sort of remixes of the same track… Quite often it's user input-based, but the users don't know that they're creating input."* And *"I think these are the most interesting systems, really, cause they think it's random, but you know it isn't."* (Reynolds 2001)
- The **AE_LIVE** series (2016, 2018, and 2022–) is a large catalogue of live concert recordings, and there's an official [AE_LIVE bandcamp](https://autechre.bandcamp.com/album/ae-live-2016-2018) — live recordings where the same algorithms run under human steering. ([Wikipedia — AE_LIVE](https://en.wikipedia.org/wiki/AE_LIVE))

## 5. Composition techniques (a working list)

From the interviews, the recurring techniques/attitudes:

1. **Real-time "playing" of the system.** Generative runs steered by MIDI faders and ears; no event generation outside the designed system. (SOS 2004)
2. **Material reused generations-deep.** *"Things can be three or four generations down the line before they are used. It's hard for us to trace the origins of the tracks that we've released."* (SOS 2004) — a literal **non-destructive edit pipeline**: the output of one process becomes the input of the next, many times over.
3. **Note-by-note vs generative as orthogonal choices.** *Draft 7:30* is "written note by note, where we know exactly what we put on"; *Confield* is heavily generative. The two modes coexist in one practice. (SOS 2004)
4. **Structure from variables, not song-form.** In the studio they build *"something that controls a set of variables that makes the track progress in a certain way."* (Reynolds 2001) The "arrangement" is a trajectory through a variable space.
5. **Out-of-time rhythm as lead instrument.** SOS 2004 notes the development toward *"out-of-time playing rhythm boxes promoted to the role of lead instruments."*
6. **Taste as the only authorship.** *"It's all about taste, completely about taste."* and *"the algorithm… just facilitates. It's just tools."* — the system has no taste; you provide it. (Reynolds 2001) Booth on DNA-to-music: *"it's not a direct-er communication… [I] would rather [create] than be an observer."*
7. **Anti-anthroporphism about software.** "*you can't start treating software like it's got a personality or taste… It's not 2001 — I mean, it is, but it's not… we're not talking about HAL, we're talking about a few bits of number crunching objects.*" (Reynolds 2001)
8. **Fluency through deep use.** *"the machine has just become an extension and you don't have to think about learning about using it at all. It's like having an extended finger."* (Reynolds 2001)
9. **Toy/serious discipline.** *"You could be working for five years with a crappy drum machine and delay unit and still find new things in there."* (SOS 2004)
10. **Titles as code.** Many track titles are *"working titles, or abbreviations of working titles,"* or *"the code number of a track"* from *"a folder of stuff that shares the same characteristics"* — reflecting the numbered-folder, batch-of-material workflow. *Draft 7:30*'s "6IE.CR" was "606IE" (a manipulation of a 606 sound). (SOS 2004)

## 6. Macero from a 2000s perspective (the owner's framing, developed)

The owner's hypothesis is that Autechre are **"doing something similar to Macero but from a 2000s perspective."** This holds up surprisingly well, and the differences are exactly the interesting part.

**The shared thesis — composition happens in the edit, after the performance.** In the project's own framing (RESEARCH §1), Teo Macero recorded long live sessions (Miles Davis) and then *composed by splicing tape, looping fragments, dropping tracks in/out* — the edit *is* the composition, the performance is raw material, and the form is non-goal-directed, emerging from selection. Autechre do the same, but with one inversion:

**The generator replaces the band.** Macero's raw material was human improvisation. Autechre's raw material is **software run for a long time** — *"record them playing all day, isolate the good sections."* The "take" isn't a live band performance; it's a generative run. The jam is the machine. This is Macero with the band replaced by an authored process — hence "a 2000s perspective."

**But the composer also authors the instrument.** Macero edited the output of musicians he didn't build. Autechre build the system *and* curate its output — they are simultaneously **instrument/algorithm author and editor**. This is exactly the Synclavier line the project already traced (a software instrument on a general-purpose computer; the patch as a structured value), extended so the instrument *generates*, not just plays. It's the strongest connection to the umbrella-first / "software as instrument" thesis.

**Rule-based, non-human time.** Macero worked with the human swing of jazz. Autechre's time is *"quantifiable"* — *"I want this to go from this beat to that beat over this amount of time, with this curve, which is shaped according to this equation."* The feel is authored as math + system dynamics, not performer feel. Importantly they reject "random": the system is deterministic-ish, chaotic-but-controlled, not arbitrary. So the humanity is *removed from the performance* but *fully present in the rules and the curation.*

**Taste/selection is the authorship, in both.** Macero's editing choices were taste. Autechre: *"It's all about taste."* The algorithm contributes no taste; the authors' ears make the call. So in *both* cases the actual compositional act is the editorial/selection layer over raw material — precisely the sound-arranger thesis ("composition happens in the edit, after the performance"), just with the "performance" being either a live band (Macero) or a running algorithm (Autechre).

**One modern refinement:** Autechre's studio-vs-live split (recursive research builds vs responsible, human-gated live builds) is a *two-profile* take on the same material — a genuinely 2000s/software idea (versioning, branching, "play back outputs to understand behaviour") that Macero never had. This is the clearest "from a 2000s perspective" marker.

## 7. Studio / gear setup

From SOS 2004 (Booth), the closest thing to a public gear picture:

- **Signal routing philosophy.** Shure Auxpander (an **8×8 patchbay with knobs** instead of patch cables) + **Mackie 16:8 and 24:8** desks. *"Together with the Mackies we're pretty limitless. Stuff can go back in and back out as many times as we want it to."* — the studio-as-instrument is literally a giant routing matrix. (SOS 2004)
- **Computers & software (2004):** Apple **G4 Powerbook / Mac** running OS X, with **Max/MSP**, **MOTU Digital Performer**, **Emagic Logic Audio**, **Steinberg Cubase SX**; earlier they mention loving **Sound Edit 16** and **Turbo Synth**, and an "Atari" in the early years. (SOS 2004)
- **Hardware synths/samplers:** Nord Lead 1 v2 (rhythm patches, 32 sounds — used a lot, live too), Yamaha FS1R (a "pretty mean thing") and DX100 (used for brassy/reedy, not just bass), Korg MS10 & MS20, Roland SH-2, MC-202, R-8, TR-606, Doepfer modules, early **Boss rack units** ("with beautiful-sounding chips in them"), Ensoniq ESR & EPS, Kurzweil K2500, Emu E-Synth, and a fleet of **Casio** samplers (FZ1, FZ10, SK1, SK5, SK100). (SOS 2004)
- **DIY / hacking.** *"Changing them is brilliant fun… get the backs off them and a few bits of wire and have an amazing time. We mess around with electronics, and have loads of broken half-bits of gear lying around. I learned some things at college and can use a soldering iron."* (SOS 2004) — cited in the earlier [Daisy Seed / DIY-synth learning avenue](.agents/notes/proposed/architecture/2026-09-01-input-jam-layer-device-registry-io.md).
- **Collaboration model.** Work separately in two studios, share everything, *"we meet up with laptops and exchange large volumes of data."* ~**a third** of released material is a solo track by one of them. (SOS 2004, Reynolds 2001)

The recent MusicRadar piece (2025) frames the same ethos as an explicit principle: *"There isn't one thing about the gear that we're using now that we don't understand"* — a **diligent tech-learning ethos**. ([MusicRadar, 2025](https://www.musicradar.com/artists/there-isnt-one-thing-about-the-gear-that-were-using-now-that-we-dont-understand-autechre-on-their-diligent-tech-learning-ethos-and-very-first-workflow))

## 8. Why this matters for sound-arranger

Autechre are almost a **reference implementation of the sound-arranger thesis**, with generative material:

1. **"Composition happens in the edit, after the performance" — literally.** They record long generative runs (the "performance") and then cut, isolate, reuse, rearrange the good sections (the edit). This is the exact workflow the platform is built to make easier and non-destructive.
2. **The "performance" can be a generative run, not a band.** The platform's phase-2 generators (euclidean rhythm, chord progressions, "improv" plugins consuming the session log) and the patch-bay graph are precisely the kind of instrument Autechre author. The arranger is the natural host for a `SOURCE` that is a generative subgraph, not just a microphone/audio input.
3. **Studio as instrument.** Autechre's "rewire per track, no centrepiece, connectivity is the point" is a strong argument for the **patch-bay as a loggable, diffable value** and for the **graph interpreter** — the connection you already built in Phase 0/Spike A.5. Their Max patches are, structurally, the same thing (a connectivity graph) plus the discipline of *"define everything before you can even start."* (Reynolds 2001: *"you have to define everything before you can even start."*)
4. **Two profiles (studio/research vs live/responsible).** The studio-vs-live algorithm split is a clean precedent for making the **profile** a real concept — the same material, different assembly, different constraints. Umbrella-first already says this; Autechre are the strongest external validation.
5. **"Not random" / controlled systems.** For any generative design in the platform, Autechre are the cautionary-and-exemplary voice: randomness is *irritating* (unrepeatable, uncuratable); preferred are deterministic-with-chaos systems you can *play* and *steer*, that produce interesting but controllable trajectories. That argues for the project's own **determinism / replay** invariants (byte-identical replay) as the foundation, and for treating "generative" as authored rules + steering, not RNG.
6. **The input/jam layer and the interop thread.** Their DIY/hardware-hacking ethos, "engineering being beautiful," and "use the studio as an instrument" align with the open/interoperable direction of the [input/jam-layer note](.agents/notes/proposed/architecture/2026-09-01-input-jam-layer-device-registry-io.md) — instruments-as-authorable, connected, non-black-box systems (the counterweight to a closed Synclavier-style silo).
7. **Taste as the only authorship.** A strong reminder that no matter how generative the system, the *product* is a curated/edited selection. The arranger's value is in making that curation fast, reversible, and legible — the whole point.

## 9. Sources

- **Wikipedia: Autechre** — history, style, discography, Anti EP / "Flutter", Confield-as-Max-experiments, equipment, AE_LIVE, live. <https://en.wikipedia.org/wiki/Autechre>
- **Sound On Sound, April 2004 ("Recording Electronica", Paul Tingen)** — the studio-as-instrument, no centrepiece, connectivity, Max/MSP mirrors hardware patching, the generative method (MIDI faders, home-made sequencers, analog sequencer stacking, "not random"), note-by-note vs generative, the gear list, DIY, collaboration. <https://web.archive.org/web/20150924120348/http://www.soundonsound.com/sos/apr04/articles/autechre.htm>
- **Grooves / Alex Reynolds, Sep 2001** — "nothing random," "all music is generative," chaotic operators, tools-not-members, software-as-tools, Fals.ch system-recordings, taste, studio-vs-live variables, DNA-sequence criticism. <https://web.archive.org/all/20060721134153/http://www.sas.upenn.edu/~reynolda/music_ae_092801.html>
- **Mark Hurrell, "Autechre and responsible algorithms" (2022)** — algorithm-as-song, studio vs live versions, recursive research builds, "responsible" live versions, versioning/branching/containerisation reading. <https://markhurrell.com/prospects/autechre-and-responsible-algorithms/>
- **Words With: Autechre, Vol. 1 (2015, Chris Zaldua)** — confirms music + algorithms in Cycling '74's Max; studio/composition practices. <https://certainsound.net/words-with-autechre-2015-1/>
- **MusicRadar (2025)** — the tech-learning ethos quote in the title. <https://www.musicradar.com/artists/there-isnt-one-thing-about-the-gear-that-were-using-now-that-we-dont-understand-autechre-on-their-diligent-tech-learning-ethos-and-very-first-workflow>
- **AE_LIVE** — live-concert recording series. <https://en.wikipedia.org/wiki/AE_LIVE>

Further reading (not read in full): [Analysis and recreation of key features in selected Autechre tracks from 1998–2005](https://figshare.mq.edu.au/articles/thesis/Analysis_and_recreation_of_key_features_in_selected_Autechre_tracks_from_1998-2005/19440656/1) (MQ thesis on their techniques); Cycling '74 community threads on reconstructing their Max patches ("[See On See patch concept](https://cycling74.com/forums/autechre-%22see-on-see%22-patch-concept)", "[Gantz Graf audio in MaxMSP](https://cycling74.com/forums/tips-on-recreating-the-audio-side-of-gantz-graf-in-maxmsp-not-video)"); the Elektronauts thread on the Untilted/Quaristice gear.

---

*Authored with DeepSeek-V4-Flash · deepseek-harness, 2026-09-01.*
