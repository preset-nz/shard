---
title: Flanger
node: flanger
group: effect
category: modulation
summary: A very short swept delay mixed with the dry sound, so notches sweep through it with a jet-like whoosh.
---

# Flanger

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
A very short swept delay mixed with the dry sound, so notches sweep through it with a jet-like whoosh.

| Control | Range | Default | What it does |
|---|---|---|---|
| Flanger | on or off | on | A very short swept delay mixed with the dry sound, so notches sweep through it with a jet-like whoosh. |
| Mix | 0% to 100% | 0% | How much flanged sound is blended with the original. |
| Manual | 0.1 ms to 8 ms | 1.5 ms | The delay the sweep centres on: short is bright and thin, longer is deeper and hollower. |
| Rate | 0.02 Hz to 10 Hz | 0.25 Hz | How fast the sweep moves. |
| Depth | 0% to 100% | 70% | How far the sweep travels around its centre. |
| Feedback | -95% to 95% | 50% | Sends the output back in: negative gives a hollow comb, positive a ringing one. |
<!-- generated:end -->

## Using it

A flanger mixes the sound with a very short delay that sweeps, so notches move through the sound with a jet-like whoosh. Small delays are bright and thin; with feedback it rings like metal.

- **Mix** starts at 0%, so raise it first. The notches are deepest at full Mix.
- **Manual** is the delay the sweep centres on. Short is bright and thin, longer is deeper and hollower.
- **Rate** is the speed of the sweep. Slow is a jet passing; fast is a warble.
- **Depth** is how far the sweep travels. At a low Depth with a short Manual the delay barely moves and the flanger becomes a fixed comb filter.
- **Feedback** sends the output back in. Positive rings, negative sounds hollow, and either way it gets more extreme towards the ends.

See Slow jet and Metal comb below.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Slow jet: Mix 60%, Rate 0.15 Hz, Feedback 70%.*

<audio controls preload="none" src="audio/flanger-1.m4a"></audio>

*Metal comb: Mix 70%, Manual 0.4 ms, Depth 10%, Feedback -90%.*

<audio controls preload="none" src="audio/flanger-2.m4a"></audio>
<!-- examples:end -->
