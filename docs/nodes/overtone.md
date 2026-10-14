---
title: Overtone
node: overtone
group: effect
summary: "Adds layers made from the sound itself: an octave down, an octave up and a fifth up."
---

# Overtone

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Adds layers made from the sound itself: an octave down, an octave up and a fifth up.

| Control | Range | Default | What it does |
|---|---|---|---|
| Overtone | on or off | on | Adds layers made from the sound itself: an octave down, an octave up and a fifth up. |
| Mix | 0% to 100% | 0% | How much of the added layers is blended with the original sound. |
| Sub | 0% to 100% | 50% | How loud the layer an octave down is. |
| Octave | 0% to 100% | 30% | How loud the layer an octave up is. |
| Fifth | 0% to 100% | 0% | How loud the layer a fifth up is. |
| Tone | 500 Hz to 16 kHz | 5 kHz | Where the highs of the layers are rolled off. |
<!-- generated:end -->

## Using it

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Sub under the voice: Mix 60%, Sub 80%, Octave 0%.*

<audio controls preload="none" src="audio/overtone-1.m4a"></audio>

*Organ: Mix 60%, Sub 30%, Octave 60%, Fifth 50%.*

<audio controls preload="none" src="audio/overtone-2.m4a"></audio>
<!-- examples:end -->
