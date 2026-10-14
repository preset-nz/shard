---
title: LFO
node: lfo
group: modulator
summary: A slow wave that a control can follow, so it drifts, wobbles or jumps by itself.
---

# LFO

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
A slow wave that a control can follow, so it drifts, wobbles or jumps by itself.

| Control | Range | Default | What it does |
|---|---|---|---|
| Name | text | empty | What the LFO is called in the list of modulators. |
| Rate | 0.01 Hz to 20 Hz | 0.1 Hz | How many times a second the wave goes round. |
| Shape | sine, triangle, saw, square, smooth-random, quad-in, quad-out, quad-in-out, cubic-in, cubic-out, cubic-in-out, expo-in, expo-out, expo-in-out, elastic-in, elastic-out, elastic-in-out, bounce-in, bounce-out, bounce-in-out, random, noise | smooth-random | The wave it draws: smooth, stepped, random, or one of the curves. |
| Phase | 0 to 1 | 0 | Where in the wave it starts, as a fraction of one turn. |

A binding from it carries:

| Control | Range | Default | What it does |
|---|---|---|---|
| Low | 0 to 1 | 0 | Where the linked control sits when the modulator is at its lowest, from the bottom to the top of its range. |
| High | 0 to 1 | 1 | Where the linked control sits when the modulator is at its highest, from the bottom to the top of its range. |
<!-- generated:end -->

## Using it
