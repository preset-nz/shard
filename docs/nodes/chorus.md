---
title: Chorus
node: chorus
group: effect
category: modulation
summary: Mixes in gently detuned, swept copies of the sound to widen and thicken it.
---

# Chorus

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Mixes in gently detuned, swept copies of the sound to widen and thicken it.

| Control | Range | Default | What it does |
|---|---|---|---|
| Chorus | on or off | on | Mixes in gently detuned, swept copies of the sound to widen and thicken it. |
| Mix | 0% to 100% | 0% | How much of the copies is blended with the original sound. |
| Type | Chorus, Ensemble, Choir | Chorus | The voicing: Chorus is plain, Ensemble is wider like a string machine, and Choir holds copies sharp and flat for voices. |
| Rate | 0.05 Hz to 5 Hz | 0.6 Hz | How fast the copies sweep. |
| Depth | 0% to 100% | 50% | How far the copies sweep: more is a deeper wobble and a stronger detune. |
| Voices | 1 to 8 | 4 | How many copies are mixed in: a few beat against each other, many blur into a wall. |
| Spread | 50% to 200% | 100% | Moves the copies closer together or further apart, from half to twice their usual distance. |
| Low cut | 20 Hz to 1 kHz | 240 Hz | Keeps the lows out of the copies so the bass stays solid. |
| EQ | -12 dB to 12 dB | 0 dB | Tilts the copies darker or brighter without touching the original sound. |
<!-- generated:end -->

## Using it

Chorus mixes in slightly detuned, slowly swept copies of the sound. It widens and thickens it without changing the notes.

- **Mix** starts at 0%, so raise it first. Around 30 to 50% is thick; higher moves the copies forward.
- **Type** is the voicing. Chorus is plain, Ensemble is wider and more string-machine, and Choir holds copies sharp and flat so a single voice sounds like several.
- **Rate** and **Depth** set how fast and how far the copies sweep. Slow and shallow is a gentle shimmer; fast and deep is a seasick wobble.
- **Voices** is how many copies there are. Few of them beat against each other; many blur into a wall.
- **Spread** moves the copies closer or further apart.
- **Low cut** keeps the bass out of the copies so the low end stays solid.
- **EQ** brightens or darkens the copies, not the original.

The Choir example below uses eight voices.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Wide and shimmering: Mix 50%, Type Ensemble, Depth 70%.*

<audio controls preload="none" src="audio/chorus-1.m4a"></audio>

*A choir of one: Mix 80%, Type Choir, Voices 8.*

<audio controls preload="none" src="audio/chorus-2.m4a"></audio>
<!-- examples:end -->
