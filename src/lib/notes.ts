/**
 * Note names for MIDI numbers, middle C being 60 and called C4.
 *
 * A material's root note is a MIDI number; the tracker's pitches are semitones
 * from the sample, so root + pitch is the note a step sounds.
 */
const NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];

/** `60` is `C4`, `69` is `A4`. Numbers outside MIDI's range are clamped. */
export function noteName(midi: number): string {
  const n = Math.max(0, Math.min(127, Math.round(midi)));
  return `${NAMES[n % 12]}${Math.floor(n / 12) - 1}`;
}

/** Every note a root can be, lowest first: C0 to B8. */
export const ROOT_NOTES: number[] = Array.from({ length: 108 }, (_, i) => i + 12);
