---
title: Drums
node: drums
group: arrangement
summary: A drum kit played by a grid of kick, snare and hat hits.
---

# Drums

<!-- generated:begin — rhizome-docs rewrites this block; write outside it -->
A drum kit played by a grid of kick, snare and hat hits.

| Control | Range | Default | What it does |
|---|---|---|---|
| On | on or off | off | Whether the drum track sounds. |
| Level | 0 to 2 | 1 | The track's fader; 1 leaves the kit as it is. |
| Tune | -12 st to 12 st | 0 st | Moves the whole kit up or down in pitch. |
| Decay | 0.25 to 4 | 1 | How long each drum rings; below 1 is tighter, above is longer. |
| Length | 4 steps to 64 steps | 16 steps | How many steps the track plays before it goes round again. |
| Mute | on or off | off | Silences the track. Kept in the file, not in the undo history. |
| Solo | on or off | off | Plays only the soloed tracks. Kept in the file, not in the undo history. |
| Duck depth | 0 to 1 | 0.75 | How far a full-strength step on the key track dips this track; 1 is silence. |
| Duck release | 10 ms to 1 s | 200 ms | How long the dip takes to come back. |
| Key only | on or off | off | Silences this track while its steps still dip whatever it ducks. |

References: `drums.duck`.
<!-- generated:end -->

## Using it
