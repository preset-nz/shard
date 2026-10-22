---
title: Filter
node: filter
group: effect
category: filter
summary: A resonant filter after all the effects, taking away the highs, the lows or everything outside a band.
---

# Filter

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
A resonant filter after all the effects, taking away the highs, the lows or everything outside a band.

| Control | Range | Default | What it does |
|---|---|---|---|
| Filter | on or off | off | A resonant filter after all the effects, taking away the highs, the lows or everything outside a band. |
| Mix | 0% to 100% | 100% | How much filtered sound is blended with the unfiltered; full is all filtered. |
| Cutoff | 20 Hz to 20 kHz | 2 kHz | The frequency where the filter starts to cut. |
| Resonance | 0% to 100% | 20% | Emphasis around the cutoff: a gentle bump at low settings, a ringing whistle at high ones. |
| Type | Low-pass, High-pass, Band-pass | Low-pass | Which part of the sound is kept: Low-pass lets the lows through, High-pass the highs, Band-pass a band around the cutoff. |
| Slope | 12 dB, 24 dB | 12 dB | How steeply the filter cuts past the cutoff: 12 dB is gentle, 24 dB is steep. |
| Tracking | Off, Half, Full | Off | How far the cutoff follows each step's pitch: off, half, or a full octave for every octave. |
<!-- generated:end -->

## Using it

The filter is a section of the patch, not a chain effect. It comes after all the effects and takes away the highs, the lows, or everything outside a band. It starts off.

- **Mix** is how much filtered sound is blended with the unfiltered. It starts at 100%, so it is all filtered once the filter is switched on.
- **Cutoff** is where the filter starts to cut. Sweeping it is the main use.
- **Resonance** emphasises the sound around the cutoff: a gentle bump at low settings, a ringing whistle at high ones.
- **Type** sets what is kept: Low-pass keeps the lows, High-pass the highs, Band-pass a band around the cutoff.
- **Slope** is how steeply it cuts. 12 dB is gentle, 24 dB is steep, as in the Behind a wall example.
- **Tracking** makes the cutoff follow each step's pitch, so higher steps stay as bright as lower ones. Half follows half as far.

The Ladder comes after this filter and has more bite.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Behind a wall: Cutoff 500 Hz, Slope 24 dB.*

<audio controls preload="none" src="audio/filter-1.m4a"></audio>

*Swept band: Type Band-pass, Resonance 70%, Cutoff swept from 200 Hz to 5 kHz.*

<audio controls preload="none" src="audio/filter-2.m4a"></audio>

*Thin: Type High-pass, Cutoff 1.5 kHz.*

<audio controls preload="none" src="audio/filter-3.m4a"></audio>
<!-- examples:end -->
