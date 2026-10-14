---
title: Repeat
node: repeat
group: effect
summary: Grabs the last slice of sound and loops it, for stutters and freezes.
---

# Repeat

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Grabs the last slice of sound and loops it, for stutters and freezes.

| Control | Range | Default | What it does |
|---|---|---|---|
| Repeat | on or off | on | Grabs the last slice of sound and loops it, for stutters and freezes. |
| Mix | 0% to 100% | 0% | How much of the loop is blended with the incoming sound. |
| Hold | 0% to 100% | 0% | Above halfway, grabs the last slice and loops it; each new rise grabs afresh, so an LFO on it stutters. |
| Length | 5 ms to 2 s | 125 ms | How long the grabbed slice is. |
| Speed | -24 st to 24 st | 0 st | How fast the loop plays, in semitones, with its pitch moving along. |
| Reverse | 0 to 1 | 0 | Plays the loop backwards. |
<!-- generated:end -->

## Using it

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Freeze a word: Mix 100%, Length 400 ms, Hold raised for 2 s mid-sentence.*

<audio controls preload="none" src="audio/repeat-1.m4a"></audio>

*Stutter: Mix 100%, Length 60 ms, Hold raised twice a second through the middle.*

<audio controls preload="none" src="audio/repeat-2.m4a"></audio>

*Backwards and down: Mix 100%, Length 500 ms, Speed -12 st, Reverse on, Hold raised for 2.5 s late in the sentence.*

<audio controls preload="none" src="audio/repeat-3.m4a"></audio>
<!-- examples:end -->
