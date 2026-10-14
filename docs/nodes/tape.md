---
title: Tape
node: tape
group: patch
summary: How the tape slows, stops or runs backwards when a brake or reverse is played.
---

# Tape

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
How the tape slows, stops or runs backwards when a brake or reverse is played.

| Control | Range | Default | What it does |
|---|---|---|---|
| Time | 20 ms to 3 s | 400 ms | How quickly the tape slows or speeds up for a brake or reverse: short is a snap, long is a slow wind. |
| Flick | 50 ms to 500 ms | 150 ms | How long a quick tap on reverse plays backwards; hold it longer and the reverse lasts until you let go. |
<!-- generated:end -->

## Using it

Tape is a section of the patch, not a chain effect. It has no sound of its own. It sets how the tape slows, stops or runs backwards when you play Brake or Reverse, and both are played by holding them.

- **Time** is how quickly the tape slows down or comes back up to speed. Short is a snap, as in the Reverse example; long is a slow wind, as in the Brake example.
- **Flick** is how long a quick tap on Reverse plays backwards. Hold Reverse for longer than Flick and it lasts until you let go.

Brake and Reverse slow every track together. Nothing happens in Tape until one of them is played.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Brake: Time 1.5 s, Brake held for 2.5 s mid-sentence.*

<audio controls preload="none" src="audio/tape-1.m4a"></audio>

*Reverse: Time 100 ms, Reverse held for 1.5 s mid-sentence.*

<audio controls preload="none" src="audio/tape-2.m4a"></audio>
<!-- examples:end -->
