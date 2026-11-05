---
title: Wear
node: wear
group: effect
category: distortion
summary: "Tape that has been looked after badly: pitch wobble, lurching, level dropouts and a dulled top end."
---

# Wear

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Tape that has been looked after badly: pitch wobble, lurching, level dropouts and a dulled top end.

| Control | Range | Default | What it does |
|---|---|---|---|
| Wear | on or off | on | Tape that has been looked after badly: pitch wobble, lurching, level dropouts and a dulled top end. |
| Mix | 0% to 100% | 0% | How much worn sound is blended with the clean. |
| Wow | 0% to 100% | 40% | Slow pitch bends, like a warped reel. |
| Flutter | 0% to 100% | 30% | A fast, small tremble in the pitch. |
| Unstable | 0% to 100% | 0% | Lurches the pitch along a chaotic path that never repeats. |
| Dropouts | 0% to 100% | 20% | How often and how deeply the level dips, as if the tape lost contact. |
| Dull | 0% to 100% | 40% | Closes a low-pass that follows the level, so quiet parts go muffled. |
<!-- generated:end -->

## Using it

Wear degrades the sound the way badly kept tape does: the pitch wobbles and lurches, the level dips and the top end dulls. It is for aged, unreliable material.

- **Mix** starts at 0%, so raise it first. A full Mix is the usual setting, since the pitch movement is what you hear.
- **Wow** is a slow pitch bend, like a warped reel.
- **Flutter** is a fast, small tremble in the pitch.
- **Unstable** lurches the pitch along a path that never repeats. It starts at 0%, and the sound sags and slides when it is raised.
- **Dropouts** is how often and how deeply the level dips.
- **Dull** darkens the sound, and quiet parts more than loud ones.

The worn cassette sound is the defaults at a full Mix. Raise Wow, Unstable and Dropouts for the Failing tape example below.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Worn cassette: Mix 100%.*

<audio controls preload="none" src="audio/wear-1.m4a"></audio>

*Failing tape: Mix 100%, Wow 80%, Unstable 70%, Dropouts 60%.*

<audio controls preload="none" src="audio/wear-2.m4a"></audio>
<!-- examples:end -->
