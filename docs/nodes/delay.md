---
title: Delay
node: delay
group: effect
summary: Clean, faithful repeats of the sound, darkened only by Tone.
---

# Delay

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Clean, faithful repeats of the sound, darkened only by Tone.

| Control | Range | Default | What it does |
|---|---|---|---|
| Delay | on or off | on | Clean, faithful repeats of the sound, darkened only by Tone. |
| Mix | 0% to 100% | 0% | How much of the repeats is blended with the original sound. |
| Time | 1 ms to 2 s | 350 ms | The gap between repeats; moving it bends the pitch. |
| Feedback | 0% to 95% | 40% | How long the repeats keep going: higher feeds each back into the next for a longer trail. |
| Tone | 500 Hz to 20 kHz | 8 kHz | How dark the repeats get; lower closes the highs a little more on every pass. |
| Ping-pong | 0% to 100% | 0% | Sends each repeat to the opposite side from the last. |
<!-- generated:end -->

## Using it

Delay repeats the sound faithfully, with only Tone darkening the repeats. It is the clean one of the two; Echo is the worn one.

- **Mix** starts at 0%, so raise it first.
- **Time** is the gap between repeats. Moving it while sound plays bends the pitch of the repeats.
- **Feedback** is how long the repeats go on. At the top they build into a long trail.
- **Tone** darkens every pass, a little more each time. Lower it for repeats that sink into the background, as in the Dark repeats example below.
- **Ping-pong** sends each repeat to the opposite side from the last. At 100% they alternate fully between left and right.

Put the delay before a reverb, so the repeats sit in the space as well.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Ping-pong: Mix 40%, Time 375 ms, Feedback 50%, Ping-pong 100%.*

<audio controls preload="none" src="audio/delay-1.m4a"></audio>

*Dark repeats: Mix 50%, Time 600 ms, Feedback 70%, Tone 1.5 kHz.*

<audio controls preload="none" src="audio/delay-2.m4a"></audio>
<!-- examples:end -->
