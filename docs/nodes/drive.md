---
title: Drive
node: drive
group: effect
summary: Bends the sound out of shape with a curve, then darkens it with a tone filter, from warm overdrive to harsh fold.
---

# Drive

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Bends the sound out of shape with a curve, then darkens it with a tone filter, from warm overdrive to harsh fold.

| Control | Range | Default | What it does |
|---|---|---|---|
| Drive | on or off | on | Bends the sound out of shape with a curve, then darkens it with a tone filter, from warm overdrive to harsh fold. |
| Mix | 0% to 100% | 0% | How much driven sound is blended with the clean. |
| Amount | 0 to 48 | 24 | How hard the sound is pushed into the curve: gentle warmth at the bottom, heavy distortion at the top. |
| Tone | 200 Hz to 20 kHz | 8 kHz | Where the highs are rolled off after the curve, to tame fizz. |
| Type | Overdrive, Distortion, Fuzz, Fold | Overdrive | The shape of the curve: Overdrive is soft, Distortion is harder, Fuzz is ragged, and Fold wraps the wave back on itself. |
<!-- generated:end -->

## Using it

Drive pushes the sound into a curve that flattens and bends its peaks. Low settings add warmth and body; high ones turn the sound harsh and buzzy. It is the usual first move for making something sound heavier.

- **Mix** starts at 0%, so raise it first. Around 50% keeps the clean sound's shape under the grit; at 100% only the driven sound is heard.
- **Amount** is how hard the sound hits the curve. Low is a gentle warmth, high is heavy distortion.
- **Tone** darkens the result after the curve. Lower it when the distortion is fizzy; at the top the highs are left alone.
- **Type** changes the character: Overdrive is soft, Distortion harder, Fuzz ragged, and Fold wraps the wave back on itself for a metallic edge.

Put Drive before Crush, Ring or the Shifter and they get more harmonics to work on. See the Folded to metal example below.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Warm grit: Mix 50%, Amount 18.*

<audio controls preload="none" src="audio/drive-1.m4a"></audio>

*Folded to metal: Mix 100%, Amount 40, Tone 4 kHz, Type Fold.*

<audio controls preload="none" src="audio/drive-2.m4a"></audio>
<!-- examples:end -->
