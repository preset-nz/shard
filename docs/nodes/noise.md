---
title: Noise
node: noise
group: generator
summary: A continuous noise generator, from low rumble to bright hiss, with an optional pitched band for wind and whistles.
---

# Noise

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
A continuous noise generator, from low rumble to bright hiss, with an optional pitched band for wind and whistles.

| Control | Range | Default | What it does |
|---|---|---|---|
| Noise | on or off | off | A continuous noise generator, from low rumble to bright hiss, with an optional pitched band for wind and whistles. |
| Gain | 0% to 200% | 100% | How loud the noise is, from silence to double. |
| Colour | -1 to 1 | 0 | The kind of noise: brown rumble at the far left, then pink, white at the centre, blue, and violet hiss at the far right. |
| Band | 0% to 100% | 0% | How much of the noise is squeezed into a narrow band at Frequency, for wind, whistles and pitched hiss; zero leaves the plain noise. |
| Frequency | 20 Hz to 8 kHz | 440 Hz | The centre of the noise band; a step's pitch moves it. |
| Width | 0% to 100% | 30% | How wide the noise band is, from a thin whistle to a broad blow two octaves wide. |
| Run | Notes, Free | Notes | Whether the noise follows the notes (Notes) or sounds for as long as the transport plays, past the envelope (Free). |
<!-- generated:end -->

## Using it

Both glitch techniques this was built from start the same way: take noise, put it through something unstable, record what happens, and harvest the takes. Noise is the raw material that has every frequency in it, so whatever comes after it decides what you hear.

**Colour** is one slider so an LFO can sweep it. Left is brown, a dark rumble like distant surf. The centre is white, an even hiss. Right is violet, a thin fizz. The loudness holds as you sweep, so Colour changes the character, not the level.

**Band** turns hiss into a note. It squeezes the noise into a resonant band at Frequency, and a step's pitch moves Frequency the way it moves FM, so the noise plays the melody. Narrow **Width** whistles; wide Width blows, like breath across a bottle. The band is made up to the plain noise's level, so raising it doesn't make things louder.

**Run** decides when it sounds. Under Notes it is a generator like the others: each step opens it, the amp envelope shapes it. Under Free it keeps going while the transport plays, a bed under everything, still through the filter, the Ladder and the output. Free skips the patch's own effects, which come before the envelope; to put a Free bed through Repeat or the Shifter, add them on the arrangement's chain.

Each voice has its own noise, and left and right are independent, so it is wide and two tracks never hiss in step.

## Recipes

- **Hi-hat.** Colour towards violet, a very short amp envelope, a step on every eighth.
- **Wind.** Colour pink, Band about 60%, Width wide, Run Free, a slow Random LFO on Frequency.
- **Whistled melody.** Band full, Width near zero, steps at the pitches you want.
- **The glitch loop.** Noise into Peaks with one Random LFO on Low and inverted on High, then the Shifter with Feedback up. Record a long take while moving things, then take it apart.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Ticks: Colour 0.8; Envelope Decay 60 ms, Sustain 0%.*

<audio controls preload="none" src="audio/noise-1.m4a"></audio>

*Pitched wind: Band 90%, Width 10%.*

<audio controls preload="none" src="audio/noise-2.m4a"></audio>
<!-- examples:end -->
