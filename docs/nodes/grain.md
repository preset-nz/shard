---
title: Granular
node: grain
group: generator
summary: Cuts the sample into tiny grains and scatters them into a cloud, anywhere from a shimmer to a stuttering texture.
---

# Granular

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Cuts the sample into tiny grains and scatters them into a cloud, anywhere from a shimmer to a stuttering texture.

| Control | Range | Default | What it does |
|---|---|---|---|
| Granular | on or off | off | Cuts the sample into tiny grains and scatters them into a cloud, anywhere from a shimmer to a stuttering texture. |
| Gain | 0% to 200% | 100% | How loud the grain cloud is, from silence to double. |
| Position | 0% to 100% | 25% | Where in the sample the grains are taken from, from the start to the end. |
| Jitter | 0% to 100% | 5% | How far each grain strays from Position at random: none keeps them together, more smears the cloud across the sample. |
| Size | 5 ms to 500 ms | 180 ms | How long each grain is: short grains buzz and blur, long ones keep the sample recognisable. |
| Density | 0.5 Hz to 200 Hz | 60 Hz | How many grains start each second: sparse grains are heard one by one, dense ones fuse into a continuous sound. |
| Pitch | -24 st to 24 st | 0 st | Shifts every grain up or down in pitch, in semitones. |
| Pitch spread | 0 st to 24 st | 0 st | Gives each grain a random pitch within this many semitones, from a plain cloud to a detuned swarm. |
| Pan spread | 0% to 100% | 60% | How widely the grains scatter between left and right; none keeps every grain in the centre. |
| Reverse | 0% to 100% | 0% | The share of grains that play backwards. |
| Window | Hann, Triangle, Expodec, Rexpodec | Hann | The fade shape of each grain: Hann is smooth, Triangle is slightly brighter, Expodec clicks like a percussive hit, and Rexpodec swells then cuts off. |

References: `grain.source`.
<!-- generated:end -->

## Using it

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*A cloud: Position 30%, Jitter 20%, Size 120 ms, Density 40 Hz.*

<audio controls preload="none" src="audio/grain-1.m4a"></audio>

*Frozen shimmer: Position 45%, Size 300 ms, Density 80 Hz, Pitch spread 12 st.*

<audio controls preload="none" src="audio/grain-2.m4a"></audio>

*Scattered backwards: Jitter 100%, Size 60 ms, Density 15 Hz, Reverse 100%, Window Expodec.*

<audio controls preload="none" src="audio/grain-3.m4a"></audio>
<!-- examples:end -->
