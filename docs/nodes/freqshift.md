---
title: Shifter
node: freqshift
group: effect
summary: "Moves every frequency by the same number of hertz, so the sound turns inharmonic: clangs, spirals and detuned shimmer."
---

# Shifter

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Moves every frequency by the same number of hertz, so the sound turns inharmonic: clangs, spirals and detuned shimmer.

| Control | Range | Default | What it does |
|---|---|---|---|
| Shifter | on or off | on | Moves every frequency by the same number of hertz, so the sound turns inharmonic: clangs, spirals and detuned shimmer. |
| Mix | 0% to 100% | 0% | How much shifted sound is blended with the original. |
| Shift | -2 kHz to 2 kHz | 0 Hz | How far every frequency moves, in hertz; negative moves them down. |
| Feedback | 0% to 95% | 0% | Sends the shifted sound round again, for a rising or falling spiral. |
| Spread | 0% to 100% | 0% | Shifts the left and right sides in opposite directions, for width. |
<!-- generated:end -->

## Using it

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Shimmer: Mix 50%, Shift 7 Hz, Spread 100%.*

<audio controls preload="none" src="audio/freqshift-1.m4a"></audio>

*Spiral: Mix 60%, Shift 120 Hz, Feedback 80%.*

<audio controls preload="none" src="audio/freqshift-2.m4a"></audio>

*Clang: Mix 100%, Shift -300 Hz.*

<audio controls preload="none" src="audio/freqshift-3.m4a"></audio>
<!-- examples:end -->
