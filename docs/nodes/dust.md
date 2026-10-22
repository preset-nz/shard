---
title: Dust
node: dust
group: generator
category: synthesis
summary: Sparse random clicks, from the odd tick to a dense crackle, like dust on a record.
---

# Dust

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
Sparse random clicks, from the odd tick to a dense crackle, like dust on a record.

| Control | Range | Default | What it does |
|---|---|---|---|
| Dust | on or off | off | Sparse random clicks, from the odd tick to a dense crackle, like dust on a record. |
| Gain | 0% to 200% | 100% | How loud the clicks are, from silence to double. |
| Density | 0.1 to 2000 | 8 | How many clicks fall each second on average, from a sparse tick to a dense crackle; where they land is random. |
| Tone | 0% to 100% | 50% | How bright each click is, from dark thuds to bright ticks. |
| Decay | 0.1 ms to 50 ms | 1 ms | How long each click lasts, from a needle tick to a short pop. |
| Spread | 0% to 100% | 50% | How widely the clicks scatter across the stereo field; none puts every click in the centre. |
<!-- generated:end -->

## Using it

Dust is a generator of sparse random clicks, from the odd tick to a dense crackle, like dust on a record. It sounds alongside the other generators and starts off.

- **Gain** is how loud the clicks are.
- **Density** is the average number of clicks per second. Low gives separate ticks; in the hundreds it becomes rain, and higher it blurs into a hiss. Where the clicks land is random.
- **Tone** is how bright each click is, from dark thuds to bright ticks.
- **Decay** is how long each click lasts, from a needle tick to a short pop.
- **Spread** scatters the clicks across the stereo field.

For a crackle under a voice, keep Gain low and Density in the tens, as in the first example below.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Dry: the sample with nothing on it.*

<audio controls preload="none" src="audio/dry.m4a"></audio>

*Crackle under the voice: Gain 40%, Density 30, Tone 70%.*

<audio controls preload="none" src="audio/dust-1.m4a"></audio>

*Rain: Density 400, Decay 5 ms.*

<audio controls preload="none" src="audio/dust-2.m4a"></audio>
<!-- examples:end -->
