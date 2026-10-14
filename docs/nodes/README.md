---
title: Shard nodes
---

# Shard nodes

<!-- generated whole by preset-docs; write in the node pages, not here -->

## Arrangement

- [Arrangement](arrangement.md): The level above the patches: tempo, swing and the tracks of steps that play them.
- [Beat](beat.md): One position in a drums track's grid: how hard each drum hits there.
- [Drums](drums.md): A drum kit played by a grid of kick, snare and hat hits.
- [Output](master.md): The arrangement's master level and limiter, after every track and effect.
- [Step](step.md): One position in a track's row: whether it plays, and how.
- [Track](track.md): A row of steps that plays one patch as a melody.

## Effect

- [Band Delay](bands.md): Splits the sound into low, mid and high and gives each band its own echo, at times that fall into a harmonic series.
- [Chorus](chorus.md): Mixes in gently detuned, swept copies of the sound to widen and thicken it.
- [Crush](crush.md): Degrades the sound by throwing away resolution and sample rate, for gritty lo-fi digital crunch.
- [Delay](delay.md): Clean, faithful repeats of the sound, darkened only by Tone.
- [Drive](drive.md): Bends the sound out of shape with a curve, then darkens it with a tone filter, from warm overdrive to harsh fold.
- [Echo](echo.md): The worn counterpart of the delay: each repeat comes back darker, dirtier and slightly wandering, like a loop of tape.
- [Envelope](env.md): Shapes how each note swells in and dies away; at its neutral setting it passes the sound untouched.
- [Filter](filter.md): A resonant filter after all the effects, taking away the highs, the lows or everything outside a band.
- [Flanger](flanger.md): A very short swept delay mixed with the dry sound, so notches sweep through it with a jet-like whoosh.
- [Shifter](freqshift.md): Moves every frequency by the same number of hertz, so the sound turns inharmonic: clangs, spirals and detuned shimmer.
- [Ladder](ladder.md): A driven four-pole low-pass after the patch filter, modelled on the Moog ladder: growls when pushed, whistles at full resonance.
- [Overtone](overtone.md): Adds layers made from the sound itself: an octave down, an octave up and a fifth up.
- [Peaks](peaks.md): Two narrow boosted bands that ring like resonators on noise and whistle when swept.
- [Repeat](repeat.md): Grabs the last slice of sound and loops it, for stutters and freezes.
- [Reverb](reverb.md): A simulated space around the sound, from a small room to a long hall or plate tail.
- [Ring modulation](ring.md): Multiplies the sound by a tone, adding metallic, bell-like frequencies that do not follow the original harmonics.
- [Rise](rise.md): Repeats that each climb in pitch from the last, so the trail rises like a siren.
- [Transient](shaper.md): Pushes the attack and the tail of a sound up or down, by how fast it changes rather than how loud it is.
- [Wear](wear.md): Tape that has been looked after badly: pitch wobble, lurching, level dropouts and a dulled top end.

## Generator

- [Dust](dust.md): Sparse random clicks, from the odd tick to a dense crackle, like dust on a record.
- [FM](fm.md): A two-operator FM voice: a pitched tone that one oscillator bends with another, from soft bell to harsh metal.
- [Granular](grain.md): Cuts the sample into tiny grains and scatters them into a cloud, anywhere from a shimmer to a stuttering texture.
- [Sample](material.md): The loaded sample played as it is, ahead of the effects; the other generators sound alongside it.
- [Noise](noise.md): A continuous noise generator, from low rumble to bright hiss, with an optional pitched band for wind and whistles.

## Material

- [Material](sample.md): A WAV file, or the built-in drone, that a generator reads and plays.

## Modulator

- [LFO](lfo.md): A slow wave that a control can follow, so it drifts, wobbles or jumps by itself.
- [Mod envelope](mod-env.md): A curve drawn over each note that a control can follow, so it swells, dips or snaps back as the note plays.

## Patch

- [Output](amp.md): The patch's master level, after every generator and effect.
- [Patch](patch.md): One sound: its generators, effects and settings, played by one or more tracks.
- [Tape](tape.md): How the tape slows, stops or runs backwards when a brake or reverse is played.
