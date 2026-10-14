---
title: Crush
node: crush
group: effect
summary: Degrades the sound by throwing away resolution and sample rate, for gritty lo-fi digital crunch.
---

# Crush

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Degrades the sound by throwing away resolution and sample rate, for gritty lo-fi digital crunch.

| Control | Range | Default | What it does |
|---|---|---|---|
| Crush | on or off | on | Degrades the sound by throwing away resolution and sample rate, for gritty lo-fi digital crunch. |
| Mix | 0% to 100% | 0% | How much crushed sound is blended with the clean. |
| Bits | 1 to 16 | 16 | How many bits of resolution are kept: fewer is noisier and more broken up. |
| Rate | 200 Hz to 48 kHz | 48 kHz | The sample rate the sound is reduced to; lower adds aliasing and a dull, metallic grain. |
<!-- generated:end -->

## Using it

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Old sampler: Mix 100%, Bits 8, Rate 12 kHz.*

<audio controls preload="none" src="audio/crush-1.m4a"></audio>

*Down to the bone: Mix 100%, Bits 3, Rate 2 kHz.*

<audio controls preload="none" src="audio/crush-2.m4a"></audio>
<!-- examples:end -->
