---
title: Ladder
node: ladder
group: effect
summary: "A driven four-pole low-pass after the patch filter, modelled on the Moog ladder: growls when pushed, whistles at full resonance."
---

# Ladder

<!-- generated:begin — rhizome-docs rewrites this block; write outside it -->
A driven four-pole low-pass after the patch filter, modelled on the Moog ladder: growls when pushed, whistles at full resonance.

| Control | Range | Default | What it does |
|---|---|---|---|
| Ladder | on or off | off | A driven four-pole low-pass after the patch filter, modelled on the Moog ladder: growls when pushed, whistles at full resonance. |
| Mix | 0% to 100% | 100% | How much of the ladder's sound is blended with what goes into it; full is all ladder. |
| Drive | 0% to 100% | 30% | Pushes the signal into the filter so it gets dirtier rather than louder. |
| Cutoff | 20 Hz to 20 kHz | 2 kHz | The frequency above which the ladder cuts, at a steep 24 dB per octave. |
| Resonance | 0% to 100% | 20% | Emphasis at the cutoff: it growls as it rises and whistles on its own at full. |
| Tracking | Off, Half, Full | Off | How far the ladder's cutoff follows each step's pitch: off, half, or full, separate from the filter's own. |
<!-- generated:end -->

## What it's for

The patch has two filters. The first is clean and exact. The Ladder comes after it and is neither: it is the second, analogue half of a digital-then-analogue pair, so an FM body and a sampled top can go through one dirty low-pass together.

**The name.** Bob Moog's filter of the 1960s is built from four transistor pairs stacked like the rungs of a ladder, and "ladder filter" became the name for that kind of four-pole low-pass. Shard's is a digital model of it: four stages in a loop, with the saturation inside the loop where the transistors would be.

**What it does as you turn it.**

- **Drive** gains the sound by up to ten times into a soft saturator, then takes the level back down, so a hot setting is dirtier, not louder.
- **Resonance** feeds the last stage back to the first. As it rises the peak at the cutoff grows into a growl rather than a scream, because the feedback is saturated too. Near the top the loop sustains itself and the Ladder whistles at the cutoff on its own, even with nothing going in.
- **Resonance thins the low end**, as real ladders do. Half of that loss is made up; the rest is the sound.
- **Tracking** makes the cutoff follow the notes, so a bright filter stays bright on high steps. It is the Ladder's own, so the clean filter can stay put while this one follows.

**Off costs nothing.** Switched off, the sound passes through bit for bit, and switching on fades in from a clean state, so there is no thump.

## Recipes

- **Acid.** A short FM or sample note, Resonance around 70%, Drive up, a modulation envelope on Cutoff.
- **Whistling noise.** Noise into the Ladder, Resonance near full, Cutoff low; sweep Cutoff with a slow LFO. The noise gives the whistle something to ride on.
- **Glitch.** The same, with a Random LFO on Cutoff and Repeat after it, Hold played by a second Random LFO. Record a take and harvest it.
