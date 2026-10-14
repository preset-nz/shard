---
title: Sample
node: material
group: generator
summary: The loaded sample played as it is, ahead of the effects; the other generators sound alongside it.
---

# Sample

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
The loaded sample played as it is, ahead of the effects; the other generators sound alongside it.

| Control | Range | Default | What it does |
|---|---|---|---|
| Material | on or off | on | The loaded sample played as it is, ahead of the effects; the other generators sound alongside it. |
| Gain | 0% to 200% | 100% | How loud the sample is going into the effects, from silence to double. |
| Scrub | 0 to 1 | 0 | Swaps normal playback for a head that glides to Position, so you can drag through the sample like a record. |
| Position | 0% to 100% | 0% | Where in the sample the scrub head sits, from the start at the left to the end at the right. |

References: `material.source`.
<!-- generated:end -->

## Using it

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Scrubbed: Scrub on, Position swept from 20% to 60% and back to 30%.*

<audio controls preload="none" src="audio/material-1.m4a"></audio>

*Played from a line: each step starts the sample at its own pitch.*

<audio controls preload="none" src="audio/material-2.m4a"></audio>
<!-- examples:end -->
