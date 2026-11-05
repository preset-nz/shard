---
title: Crush
node: crush
group: effect
category: distortion
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

Crush throws away detail. Fewer bits make the sound noisy and broken up; a lower sample rate adds a dull, metallic grain. It sounds like an old sampler or a bad digital connection.

- **Mix** starts at 0%, so raise it first. At 100% the whole sound is crushed.
- **Bits** is the resolution kept. Down to about 8 it sounds like vintage hardware; at 3 or fewer the sound is mostly noise and stepping, and at 1 little of it is left.
- **Rate** is the sample rate the sound is reduced to. Lowering it folds the highs back down as aliasing, so the sound goes dull and inharmonic. At 48 kHz nothing is lost.

Bits and Rate do different things, so set one at a time and then combine them. A Drive before Crush gives it harmonics to break up. Old sampler below is a starting point.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Old sampler: Mix 100%, Bits 8, Rate 12 kHz.*

<audio controls preload="none" src="audio/crush-1.m4a"></audio>

*Down to the bone: Mix 100%, Bits 3, Rate 2 kHz.*

<audio controls preload="none" src="audio/crush-2.m4a"></audio>
<!-- examples:end -->
