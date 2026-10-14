---
title: Echo
node: echo
group: effect
summary: "The worn counterpart of the delay: each repeat comes back darker, dirtier and slightly wandering, like a loop of tape."
---

# Echo

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
The worn counterpart of the delay: each repeat comes back darker, dirtier and slightly wandering, like a loop of tape.

| Control | Range | Default | What it does |
|---|---|---|---|
| Echo | on or off | on | The worn counterpart of the delay: each repeat comes back darker, dirtier and slightly wandering, like a loop of tape. |
| Mix | 0% to 100% | 0% | How much of the echoes is blended with the original sound. |
| Time | 20 ms to 1.5 s | 300 ms | The gap between echoes. |
| Feedback | 0% to 95% | 45% | How long the echoes keep going: higher feeds each back into the next for a longer trail. |
| Tone | 400 Hz to 12 kHz | 3.5 kHz | How dark the echoes get; lower closes the highs a little more on every pass. |
| Wobble | 0% to 100% | 30% | How much the pitch of the echoes wanders. |
| Grit | 0% to 100% | 30% | How much saturation each echo picks up. |
<!-- generated:end -->

## Using it

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Tape echo: Mix 50%, Time 400 ms, Feedback 55%.*

<audio controls preload="none" src="audio/echo-1.m4a"></audio>

*Runaway: Mix 50%, Time 250 ms, Feedback 90%, Grit 70%.*

<audio controls preload="none" src="audio/echo-2.m4a"></audio>
<!-- examples:end -->
