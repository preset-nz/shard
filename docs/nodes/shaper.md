---
title: Transient
node: shaper
group: effect
summary: Pushes the attack and the tail of a sound up or down, by how fast it changes rather than how loud it is.
---

# Transient

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Pushes the attack and the tail of a sound up or down, by how fast it changes rather than how loud it is.

| Control | Range | Default | What it does |
|---|---|---|---|
| Transient | on or off | on | Pushes the attack and the tail of a sound up or down, by how fast it changes rather than how loud it is. |
| Mix | 0% to 100% | 0% | How much shaped sound is blended with the original. |
| Attack | -100% to 100% | 0% | Positive sharpens the onset, negative softens it. |
| Sustain | -100% to 100% | 0% | Positive lifts the tail, negative dries it up. |
| Speed | 1 ms to 50 ms | 10 ms | How long counts as the attack, from snappy to slow. |
<!-- generated:end -->

## Using it

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Sharper consonants: Mix 100%, Attack 80%.*

<audio controls preload="none" src="audio/shaper-1.m4a"></audio>

*Dried up: Mix 100%, Sustain -80%.*

<audio controls preload="none" src="audio/shaper-2.m4a"></audio>
<!-- examples:end -->
