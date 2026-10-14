---
title: Band Delay
node: bands
group: effect
summary: Splits the sound into low, mid and high and gives each band its own echo, at times that fall into a harmonic series.
---

# Band Delay

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Splits the sound into low, mid and high and gives each band its own echo, at times that fall into a harmonic series.

| Control | Range | Default | What it does |
|---|---|---|---|
| Band Delay | on or off | on | Splits the sound into low, mid and high and gives each band its own echo, at times that fall into a harmonic series. |
| Mix | 0% to 100% | 0% | How much of the echoes is blended with the original sound. |
| Time | 10 ms to 2 s | 375 ms | The echo time of the low band, which the others are measured from; moving it bends the pitch. |
| Harmonic | 0% to 100% | 100% | How far apart the bands' times sit: all the same at zero, a half and a third of the low band's at full. |
| Feedback | 0% to 95% | 40% | How long each band's echoes keep going. |
| Split | 100 Hz to 5 kHz | 800 Hz | Where the low band meets the mid; the mid meets the high three octaves above. |
<!-- generated:end -->

## Using it

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Harmonic echoes: Mix 50%.*

<audio controls preload="none" src="audio/bands-1.m4a"></audio>

*Bent by time: Mix 60%, Feedback 70%, Time swept from 375 ms to 120 ms.*

<audio controls preload="none" src="audio/bands-2.m4a"></audio>
<!-- examples:end -->
