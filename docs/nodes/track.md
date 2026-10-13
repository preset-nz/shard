---
title: Track
node: track
group: arrangement
summary: A row of steps that plays one patch as a melody.
---

# Track

<!-- generated:begin — rhizome-docs rewrites this block; write outside it -->
A row of steps that plays one patch as a melody.

| Control | Range | Default | What it does |
|---|---|---|---|
| Level | 0 to 2 | 1 | The track's fader; 1 leaves the patch as it is. |
| Length | 4 steps to 64 steps | 16 steps | How many steps the track plays before it goes round again. |
| Mute | on or off | off | Silences the track. Kept in the file, not in the undo history. |
| Solo | on or off | off | Plays only the soloed tracks. Kept in the file, not in the undo history. |
| Duck depth | 0 to 1 | 0.75 | How far a full-strength step on the key track dips this track; 1 is silence. |
| Duck release | 10 ms to 1000 ms | 200 ms | How long the dip takes to come back. |
| Key only | on or off | off | Silences this track while its steps still dip whatever it ducks. |

References: `track.patch`, `track.duck`.
<!-- generated:end -->

## What it's for
