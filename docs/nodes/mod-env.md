---
title: Mod envelope
node: mod-env
group: modulator
summary: A curve drawn over each note that a control can follow, so it swells, dips or snaps back as the note plays.
---

# Mod envelope

<!-- generated:begin — rhizome-docs rewrites this block; write outside it -->
A curve drawn over each note that a control can follow, so it swells, dips or snaps back as the note plays.

| Control | Range | Default | What it does |
|---|---|---|---|
| Name | text | empty | What the envelope is called in the list of modulators. |
| Mode | adsr, free | adsr | ADSR draws the curve with four sliders; free lets you place the points yourself. |
| Curve | structured | {"points":[{"basis":"linear","x":0.0,"y":0.0},{"basis":"linear","x":0.02,"y":1.0},{"basis":"linear","x":0.4,"y":0.2},{"basis":"linear","x":1.0,"y":0.0}],"sustain":2} | The shape a linked control follows over the length of a note. |

A binding from it carries:

| Control | Range | Default | What it does |
|---|---|---|---|
| Low | 0 to 1 | 0 | Where the linked control sits when the modulator is at its lowest, from the bottom to the top of its range. |
| High | 0 to 1 | 1 | Where the linked control sits when the modulator is at its highest, from the bottom to the top of its range. |
<!-- generated:end -->

## Using it
