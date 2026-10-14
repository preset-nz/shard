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

The transient shaper changes the attack and the tail of a sound, and tells them apart by how fast the sound changes rather than how loud it is. It sharpens or softens hits, and brings out or dries up the tail.

- **Mix** starts at 0%, so raise it first. A full Mix is usual here.
- **Attack** sharpens the start of each sound when positive and softens it when negative.
- **Sustain** lifts the tail when positive and dries it up when negative.
- **Speed** is how long counts as the attack. Short catches only the snap; longer takes in more of the onset.

Use it on drums and spoken material to make them punchier or tighter. See the Sharper consonants and Dried up examples below.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Sharper consonants: Mix 100%, Attack 80%.*

<audio controls preload="none" src="audio/shaper-1.m4a"></audio>

*Dried up: Mix 100%, Sustain -80%.*

<audio controls preload="none" src="audio/shaper-2.m4a"></audio>
<!-- examples:end -->
