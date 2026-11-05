---
title: Peaks
node: peaks
group: effect
category: filter
summary: Two narrow boosted bands that ring like resonators on noise and whistle when swept.
---

# Peaks

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Two narrow boosted bands that ring like resonators on noise and whistle when swept.

| Control | Range | Default | What it does |
|---|---|---|---|
| Peaks | on or off | on | Two narrow boosted bands that ring like resonators on noise and whistle when swept. |
| Mix | 0% to 100% | 0% | How much of the boosted bands is blended with the original sound. |
| Low | 40 Hz to 12 kHz | 400 Hz | The centre of the lower band. |
| High | 40 Hz to 12 kHz | 2.4 kHz | The centre of the upper band. |
| Boost | 0 dB to 36 dB | 18 dB | How far both bands are lifted, in decibels. |
| Width | 0% to 100% | 20% | How wide the bands are, from narrow and whistling to broad. |
<!-- generated:end -->

## Using it

Peaks adds two narrow boosted bands that ring like resonators. On noise they whistle and, when swept, they sound like wind or a vocal formant moving.

- **Mix** starts at 0%, so raise it first.
- **Low** and **High** are the centres of the two bands. Move them apart for two separate resonances, or together for one strong one.
- **Boost** is how far both bands are lifted. At high values the bands ring loudly.
- **Width** is how wide they are. Narrow is a whistle, broad is a gentle emphasis.

Peaks works best on sound that already has plenty in it, such as noise, a drum or a dense sample. Sweeping Low with an LFO is the Swept whistle example below.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Two resonances: Mix 70%, Low 800 Hz, High 2.5 kHz, Boost 24 dB, Width 10%.*

<audio controls preload="none" src="audio/peaks-1.m4a"></audio>

*Swept whistle: Mix 80%, Boost 30 dB, Width 5%, Low swept from 200 Hz to 3 kHz.*

<audio controls preload="none" src="audio/peaks-2.m4a"></audio>
<!-- examples:end -->
