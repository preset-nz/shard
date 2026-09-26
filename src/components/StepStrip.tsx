import { useEffect, useRef, useState } from 'react';
import type { Track, Tracker } from '@/audio';
import { Slider } from '@/components/ui/slider';

const LENGTHS = [4, 8, 16];
const TEMPO_MIN = 40;
const TEMPO_MAX = 240;
/**
 * Swing is stored as the delay of every second sixteenth, 0 to 0.5 of one,
 * and shown the drum-machine way, 50 % straight to 75 % heavy, so the 56 % in
 * `design/drum-programming.md` means what it says.
 */
const toPercent = (swing: number) => 50 + swing * 50;
const fromPercent = (percent: number) => (percent - 50) / 50;

/**
 * The tracker: the level above the patch (Georg, 2026-09-14).
 *
 * Tempo and swing belong to the arrangement, the steps to its one track, and none of
 * it to the patch, so none of it is a parameter row. One button per
 * sixteenth, the step playing ringed. Buttons keep a sixteenth's width
 * whatever the length, so four steps read as one beat rather than a bar
 * stretched out.
 *
 * Shortening a track keeps the steps past its new end, so lengthening it
 * again gives them back. They cannot be seen or reached while hidden, so the
 * strip says when there are some.
 *
 * Every change hands the whole tracker up; App shows it at once and Rust
 * answers with it brought into range.
 */
export function StepStrip({
  tracker,
  step,
  onChange,
}: {
  tracker: Tracker;
  /** The step playing, from zero, or -1. */
  step: number;
  onChange: (next: Tracker) => void;
}) {
  const track = tracker.tracks[0];
  const setTrack = (change: Partial<Track>) =>
    onChange({ ...tracker, tracks: [{ ...track, ...change }, ...tracker.tracks.slice(1)] });

  // Typed as text and committed on Enter or leaving the field, so typing 95
  // does not clamp to 40 on the way through the 9.
  const shown = String(Math.round(tracker.tempo));
  const [tempo, setTempo] = useState(shown);
  useEffect(() => setTempo(shown), [shown]);
  const commitTempo = () => {
    const v = Number(tempo);
    if (tempo.trim() === '' || !Number.isFinite(v)) {
      setTempo(shown);
      return;
    }
    const clamped = Math.min(TEMPO_MAX, Math.max(TEMPO_MIN, Math.round(v)));
    setTempo(String(clamped));
    if (clamped !== Math.round(tracker.tempo)) onChange({ ...tracker, tempo: clamped });
  };

  return (
    <div
      className={`flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground ${
        track.on ? '' : 'opacity-60'
      }`}
    >
      {/* No switch: the mode decides whether the steps play, and this strip
          only shows in tracker mode. */}
      <span className="text-[11px] font-semibold uppercase tracking-wider">Tracker</span>

      <label className="flex items-center gap-1">
        <input
          type="number"
          min={TEMPO_MIN}
          max={TEMPO_MAX}
          value={tempo}
          onChange={(e) => setTempo(e.target.value)}
          onBlur={commitTempo}
          onKeyDown={(e) => {
            if (e.key === 'Enter') e.currentTarget.blur();
            if (e.key === 'Escape') {
              setTempo(shown);
              e.currentTarget.blur();
            }
          }}
          aria-label="Tempo"
          className="w-12 rounded-sm border border-border bg-transparent px-1 py-0.5 text-right font-mono tabular-nums text-foreground"
        />
        bpm
      </label>

      <div className="flex items-center gap-2">
        <span>Swing</span>
        <Slider
          min={50}
          max={75}
          step={1}
          value={[toPercent(tracker.swing)]}
          onValueChange={([percent]) => onChange({ ...tracker, swing: fromPercent(percent) })}
          aria-label="Swing"
          className="w-20"
        />
        <span className="w-8 font-mono text-[11px] tabular-nums">
          {Math.round(toPercent(tracker.swing))}%
        </span>
      </div>

      <fieldset className="flex items-center gap-0.5" aria-label="Length">
        {LENGTHS.map((n) => (
          <button
            key={n}
            type="button"
            aria-pressed={track.length === n}
            title={`${n} steps`}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => setTrack({ length: n })}
            className={`rounded px-1.5 py-0.5 font-mono text-[10px] tabular-nums ${
              track.length === n ? 'bg-primary/15 text-primary' : 'hover:text-foreground'
            }`}
          >
            {n}
          </button>
        ))}
      </fieldset>

      {/* Steps over their pitches, a column a sixteenth, like a tracker's
          note column under its triggers. */}
      <div className="flex min-w-0 flex-col gap-1">
        <fieldset className="flex min-w-0 items-center gap-1" aria-label="Steps">
          {Array.from({ length: track.length }, (_, i) => {
            const set = ((track.pattern >> i) & 1) === 1;
            const playing = track.on && step === i;
            return (
              <button
                // biome-ignore lint/suspicious/noArrayIndexKey: a step is its index
                key={i}
                type="button"
                aria-pressed={set}
                aria-label={`Step ${i + 1}`}
                title={`Step ${i + 1}`}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => setTrack({ pattern: track.pattern ^ (1 << i) })}
                className={`h-6 w-6 shrink-0 rounded-sm border transition-colors ${
                  i > 0 && i % 4 === 0 ? 'ml-2' : ''
                } ${set ? 'border-primary bg-primary' : 'border-border hover:bg-muted'} ${
                  playing ? 'ring-2 ring-foreground ring-offset-1 ring-offset-background' : ''
                }`}
              />
            );
          })}
        </fieldset>
        <fieldset className="flex min-w-0 items-center gap-1" aria-label="Pitch">
          {Array.from({ length: track.length }, (_, i) => (
            <PitchCell
              // biome-ignore lint/suspicious/noArrayIndexKey: a step is its index
              key={i}
              step={i}
              semis={track.pitches[i] ?? 0}
              set={((track.pattern >> i) & 1) === 1}
              onChange={(semis) => {
                const pitches = Array.from({ length: 16 }, (_, j) => track.pitches[j] ?? 0);
                pitches[i] = semis;
                setTrack({ pitches });
              }}
            />
          ))}
        </fieldset>
      </div>

      {hiddenSteps(track) > 0 && (
        <span title="Kept from a longer track. Lengthen it to see and change them.">
          +{hiddenSteps(track)} past step {track.length}
        </span>
      )}
    </div>
  );
}

/** Semitones a step can be pitched, either way. `PITCH_RANGE` in Rust. */
const PITCH_RANGE = 24;
/** How far a drag moves before the pitch moves a semitone. */
const PX_PER_SEMITONE = 6;

/**
 * One step's pitch, in semitones. Varispeed, as Octave is: up plays the
 * sample faster and shorter.
 *
 * Drag up or down, or focus it and use the arrows, Shift for an octave.
 * Double-click, Backspace or 0 clears it. A focusable element rather than a
 * button or a number field, so Space still starts and stops playback while a
 * cell has focus.
 */
function PitchCell({
  step,
  semis,
  set,
  onChange,
}: {
  step: number;
  semis: number;
  /** Whether the step above it is on; an off step's pitch is drawn quieter. */
  set: boolean;
  onChange: (semis: number) => void;
}) {
  const drag = useRef<{ y: number; from: number } | null>(null);
  const commit = (next: number) => {
    const clamped = Math.max(-PITCH_RANGE, Math.min(PITCH_RANGE, Math.round(next)));
    if (clamped !== semis) onChange(clamped);
  };
  const tone =
    semis === 0 ? 'text-muted-foreground/40' : set ? 'text-foreground' : 'text-muted-foreground';

  return (
    <div
      role="spinbutton"
      tabIndex={0}
      aria-label={`Step ${step + 1} pitch`}
      aria-valuemin={-PITCH_RANGE}
      aria-valuemax={PITCH_RANGE}
      aria-valuenow={semis}
      title={`Step ${step + 1}: ${semis > 0 ? '+' : ''}${semis} semitones. Drag or use the arrows, Shift for an octave. Double-click to clear.`}
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        drag.current = { y: e.clientY, from: semis };
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (d) commit(d.from + (d.y - e.clientY) / PX_PER_SEMITONE);
      }}
      onPointerUp={() => {
        drag.current = null;
      }}
      onPointerCancel={() => {
        drag.current = null;
      }}
      onDoubleClick={() => commit(0)}
      onKeyDown={(e) => {
        const by = e.shiftKey ? 12 : 1;
        if (e.key === 'ArrowUp') commit(semis + by);
        else if (e.key === 'ArrowDown') commit(semis - by);
        else if (e.key === 'Backspace' || e.key === 'Delete' || e.key === '0') commit(0);
        else return;
        e.preventDefault();
      }}
      className={`flex h-5 w-6 shrink-0 cursor-ns-resize select-none items-center justify-center rounded-sm font-mono text-[10px] tabular-nums outline-none hover:bg-muted focus-visible:ring-1 focus-visible:ring-ring ${
        step > 0 && step % 4 === 0 ? 'ml-2' : ''
      } ${tone}`}
    >
      {semis > 0 ? `+${semis}` : semis}
    </div>
  );
}

/** Steps that are on past the track's end. */
function hiddenSteps(track: Track): number {
  let rest = track.pattern >>> track.length;
  let count = 0;
  while (rest > 0) {
    count += rest & 1;
    rest >>>= 1;
  }
  return count;
}
