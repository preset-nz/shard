---
title: Reverb
node: reverb
group: effect
category: time
summary: A simulated space around the sound, from a small room to a long hall or plate tail.
---

# Reverb

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
A simulated space around the sound, from a small room to a long hall or plate tail.

| Control | Range | Default | What it does |
|---|---|---|---|
| Reverb | on or off | on | A simulated space around the sound, from a small room to a long hall or plate tail. |
| Mix | 0% to 100% | 0% | How much of the space is blended with the dry sound. |
| Type | Room, Hall, Plate | Room | The kind of space: Room, Hall or Plate. |
| Decay | 0.2 s to 20 s | 2.5 s | How long the tail lasts. |
| Size | 0% to 100% | 50% | How big the space feels. |
| Tone | 800 Hz to 16 kHz | 6 kHz | How quickly the highs die in the tail; lower is darker. |
| Pre-delay | 0 ms to 200 ms | 12 ms | The gap before the tail starts, which keeps the dry sound clear of it. |
<!-- generated:end -->

## Using it

Reverb puts the sound in a space. The tail comes after the sound and blurs into it, from a small room to a long hall or a plate.

- **Mix** starts at 0%, so raise it first. A little goes a long way; more pushes the sound further back.
- **Type** is the kind of space. Room is small and close, Hall is large, and Plate is smooth and bright.
- **Decay** is how long the tail lasts, up to 20 s.
- **Size** is how big the space feels.
- **Tone** is how quickly the highs die in the tail. Lower is darker.
- **Pre-delay** is the gap before the tail begins. A longer gap keeps the dry sound clear of it, as in the Cathedral example below.

Put the reverb late in the chain. An effect after it works on the tail as well.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*A small room: Mix 30%.*

<audio controls preload="none" src="audio/reverb-1.m4a"></audio>

*Cathedral: Mix 50%, Type Hall, Decay 8 s, Size 90%, Pre-delay 60 ms.*

<audio controls preload="none" src="audio/reverb-2.m4a"></audio>

*Bright plate: Mix 40%, Type Plate, Decay 3 s, Tone 12 kHz.*

<audio controls preload="none" src="audio/reverb-3.m4a"></audio>
<!-- examples:end -->
