---
title: Write tracks with the agent
summary: Type what you want, and the agent writes drums, bass, arpeggio and melody tracks into the session.
order: 20
---

# Write tracks with the agent

The agent panel writes tracks into the open session from what you type. Choose **View ▸ Agent** (⌥⌘A) to open it. It is off until it's set up: see [Set up the agent](agent-setup.md).

## Asking for tracks

Type what you want, the way you'd brief someone: a style, a key, a tempo, a feel. Then press Return.

> a dark industrial beat and a driving bass line in D minor, 125 bpm

The reply streams into the panel, and the tracks appear in the session when it finishes.

![The agent panel after a reply: the tool it called, its answer and the undo step it made](img/agent-reply.webp)

Each request reads the song as it stands, not the conversation before it. "Again, but faster" means nothing to it: say what you want in full.

## What it writes

**Drums** A whole drums track in a style: EBM straight or shuffled, four on the floor, half time, sparse or broken glitch. It replaces the track's kick, snare and hat hits and leaves a material lane alone.

**Bass** A bass line over a chord progression, on a new track with a bass sound of its own.

**Arpeggio** An arpeggio over a chord progression, with an arp sound of its own.

**Melody** A lead or any pitched line, note by note.

**Tempo and swing** The song's tempo in beats per minute, and its swing.

## Quick and Considered

The two buttons under the prompt set how long the model thinks before it writes.

**Quick** Answers in seconds.

**Considered** Reasons first and writes denser lines. It takes a minute or more.

## Stopping and undoing

Esc in the panel stops a reply and keeps what it wrote so far. **Edit ▸ Undo** takes back everything one reply wrote, in one step.

## See also

- [Set up the agent](agent-setup.md)
