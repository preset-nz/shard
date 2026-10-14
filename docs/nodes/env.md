---
title: Envelope
node: env
group: effect
summary: Shapes how each note swells in and dies away; at its neutral setting it passes the sound untouched.
---

# Envelope

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Shapes how each note swells in and dies away; at its neutral setting it passes the sound untouched.

| Control | Range | Default | What it does |
|---|---|---|---|
| Envelope | on or off | on | Shapes how each note swells in and dies away; at its neutral setting it passes the sound untouched. |
| Mix | 0% to 100% | 100% | How much of the shaped sound replaces the unshaped one; zero leaves the sound untouched. |
| Attack | 0 ms to 4 s | 0 ms | How long the sound takes to rise from silence to full level. |
| Decay | 0 ms to 4 s | 0 ms | How long the sound takes to fall from full level to Sustain. |
| Sustain | 0% to 100% | 100% | The level held after the decay, until the note is released. |
| Release | 0 ms to 4 s | 0 ms | How long the sound takes to fade to silence once the note ends. |
<!-- generated:end -->

## Using it

The envelope is a section of the patch, not a chain effect. It shapes how each note swells in and dies away. At its neutral settings it leaves the sound untouched, so you only hear it once you move something.

- **Mix** is how much of the shaped sound replaces the unshaped one. Zero leaves the sound as it is.
- **Attack** is how long the sound takes to rise to full level. A long Attack is a slow swell, as in the Slow swell example.
- **Decay** is how long it takes to fall from full level to Sustain.
- **Sustain** is the level held until the note ends. At 0% the note dies after the Decay, giving plucks, as in Plucked words.
- **Release** is the fade after the note ends. A short one cuts off; a long one lets notes run into each other.

For generators that play from steps, such as FM, noise and dust, the envelope gives the note its length and shape.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Slow swell: Attack 3 s.*

<audio controls preload="none" src="audio/env-1.m4a"></audio>

*Plucked words: Decay 150 ms, Sustain 0%, played from a line.*

<audio controls preload="none" src="audio/env-2.m4a"></audio>
<!-- examples:end -->
