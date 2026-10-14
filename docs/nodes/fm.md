---
title: FM
node: fm
group: generator
summary: "A two-operator FM voice: a pitched tone that one oscillator bends with another, from soft bell to harsh metal."
---

# FM

<!-- generated:begin — preset-docs rewrites this block; write outside it -->
A two-operator FM voice: a pitched tone that one oscillator bends with another, from soft bell to harsh metal.

| Control | Range | Default | What it does |
|---|---|---|---|
| FM | on or off | off | A two-operator FM voice: a pitched tone that one oscillator bends with another, from soft bell to harsh metal. |
| Gain | 0% to 200% | 100% | How loud the FM tone is, from silence to double. |
| Frequency | 20 Hz to 2 kHz | 110 Hz | The pitch of the FM tone; a step's pitch moves it. |
| Ratio | 0.25 to 16 | 2 | How the second oscillator's pitch relates to the first: whole numbers sound harmonic, anything else turns clangy and bell-like. |
| Index | 0 to 10 | 1.5 | How hard one oscillator bends the other: low is a soft tone, high is bright and buzzy. |
| Feedback | 0% to 100% | 0% | Feeds the modulating oscillator back into itself, thickening the tone to a rasp and, at the top, towards noise. |
| Drift | 0% to 100% | 0% | Lets each oscillator wander slightly off its pitch, from clinically exact at zero to loosely analogue. |
| Type | Phase, Linear, Exponential | Phase | How the oscillators bend each other: Phase holds its pitch at any Index, Linear drifts in pitch once Feedback is up, and Exponential goes sharp as Index rises, as an analogue VCO does. |
<!-- generated:end -->

## Using it

FM is a generator: a pitched tone where one oscillator bends another. It runs from a soft bell to harsh metal. It plays from the tracker's steps, so a step's pitch moves it, and it sounds alongside the other generators. It starts off.

- **Gain** is how loud the tone is.
- **Frequency** is the pitch. A step's pitch moves it.
- **Ratio** relates the second oscillator's pitch to the first. Whole numbers sound harmonic; anything else is clangy and bell-like.
- **Index** is how hard one oscillator bends the other. Low is a soft tone, high is bright and buzzy.
- **Feedback** thickens the tone to a rasp and, near the top, towards noise.
- **Drift** lets the oscillators wander off their pitch, from exact to loose.
- **Type** changes how the oscillators bend each other. Phase holds its pitch at any Index; Exponential goes sharp as Index rises.

Shape the note with the envelope: the Struck bell example needs a short Decay and no Sustain.

## Examples

<!-- examples:begin — `just examples` rewrites this block; write outside it -->
*Struck bell: Frequency 220 Hz, Ratio 3.5, Index 3; Envelope Decay 600 ms, Sustain 0%.*

<audio controls preload="none" src="audio/fm-1.m4a"></audio>

*Bass: Frequency 55 Hz, Ratio 1, Index 2, Feedback 30%.*

<audio controls preload="none" src="audio/fm-2.m4a"></audio>

*Harsh metal: Ratio 7.1, Index 8, Feedback 60%, Type Exponential.*

<audio controls preload="none" src="audio/fm-3.m4a"></audio>
<!-- examples:end -->
