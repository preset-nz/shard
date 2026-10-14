---
title: Rise
node: rise
group: effect
summary: Repeats that each climb in pitch from the last, so the trail rises like a siren.
---

# Rise

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Repeats that each climb in pitch from the last, so the trail rises like a siren.

| Control | Range | Default | What it does |
|---|---|---|---|
| Rise | on or off | on | Repeats that each climb in pitch from the last, so the trail rises like a siren. |
| Mix | 0% to 100% | 0% | How much of the rising repeats is blended with the original sound. |
| Time | 80 ms to 1.2 s | 380 ms | The gap between repeats. |
| Feedback | 0% to 90% | 60% | How many climbing repeats you hear: higher feeds each back into the next for a longer climb. |
| Shift | -12 st to 12 st | 7 st | How many semitones each repeat moves from the one before; negative makes the trail sink. |
| Wobble | 0% to 100% | 30% | Wanders the shift for an unsteady climb. |
| Tone | 800 Hz to 14 kHz | 6 kHz | Rolls off the highs so the climbing repeats do not turn to glass. |
<!-- generated:end -->

## Using it

Rise repeats the sound with each repeat higher in pitch than the last, so the trail climbs like a siren. A negative Shift makes it sink instead.

- **Mix** starts at 0%, so raise it first.
- **Time** is the gap between repeats.
- **Feedback** is how many climbing repeats you hear. Higher is a longer climb.
- **Shift** is how many semitones each repeat moves from the one before. 7 is a rising fifth each time; small values give a slow drift; negative values sink, as in the Sinking example below.
- **Wobble** makes the shift wander, so the climb is unsteady.
- **Tone** rolls off the highs so the top of the climb does not turn to glass.

A longer Time with a high Feedback leaves room to hear each step of the climb.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Climbing fifths: Mix 50%.*

<audio controls preload="none" src="audio/rise-1.m4a"></audio>

*Sinking: Mix 50%, Feedback 80%, Shift -5 st.*

<audio controls preload="none" src="audio/rise-2.m4a"></audio>
<!-- examples:end -->
