import { useEffect, useRef, useState } from 'react';
import type { Step, Track, Tracker } from '@/audio';
import { Slider } from '@/components/ui/slider';
import { noteName } from '@/lib/notes';

const LENGTHS = [4, 8, 16, 32, 64];
/** Steps in a bar: the strip is drawn a bar to a row. */
const BAR = 16;
/** Every step a track holds. `STEPS` in Rust. */
const STEPS = 64;
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
 * stretched out, and a longer track is drawn a bar of sixteen to a row.
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
  root,
  onChange,
}: {
  tracker: Tracker;
  /** The step playing, from zero, or -1. */
  step: number;
  /** The Sample material's root note, a MIDI number, so pitches show as names. */
  root: number | null;
  onChange: (next: Tracker) => void;
}) {
  const track = tracker.tracks[0];
  const setTrack = (change: Partial<Track>) =>
    onChange({ ...tracker, tracks: [{ ...track, ...change }, ...tracker.tracks.slice(1)] });
  const stepAt = (i: number): Step => track.steps[i] ?? { on: false, pitch: 0, hold: 0 };
  const setStep = (i: number, change: Partial<Step>) => {
    const steps = Array.from({ length: STEPS }, (_, j) => stepAt(j));
    steps[i] = { ...steps[i], ...change };
    setTrack({ steps });
  };
  const bars = Array.from({ length: Math.ceil(track.length / BAR) }, (_, b) => b);

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

      {/* Steps over their pitches and holds, a column a sixteenth, like a
          tracker's columns under its triggers. A bar to a row. */}
      <div className="flex min-w-0 flex-col gap-2">
        {bars.map((bar) => {
          const first = bar * BAR;
          const cells = Array.from(
            { length: Math.min(BAR, track.length - first) },
            (_, k) => first + k,
          );
          return (
            <div key={bar} className="flex min-w-0 flex-col gap-1">
              <fieldset
                className="flex min-w-0 items-center gap-1"
                aria-label={`Steps, bar ${bar + 1}`}
              >
                {cells.map((i) => {
                  const set = stepAt(i).on;
                  const playing = track.on && step === i;
                  return (
                    <button
                      key={i}
                      type="button"
                      aria-pressed={set}
                      aria-label={`Step ${i + 1}`}
                      title={`Step ${i + 1}`}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => setStep(i, { on: !set })}
                      className={`h-6 w-6 shrink-0 rounded-sm border transition-colors ${
                        i % BAR > 0 && i % 4 === 0 ? 'ml-2' : ''
                      } ${set ? 'border-primary bg-primary' : 'border-border hover:bg-muted'} ${
                        playing ? 'ring-2 ring-foreground ring-offset-1 ring-offset-background' : ''
                      }`}
                    />
                  );
                })}
              </fieldset>
              <fieldset className="flex min-w-0 items-center gap-1" aria-label="Pitch">
                {cells.map((i) => (
                  <PitchCell
                    key={i}
                    step={i}
                    semis={stepAt(i).pitch}
                    root={root}
                    set={stepAt(i).on}
                    onChange={(pitch) => setStep(i, { pitch })}
                  />
                ))}
              </fieldset>
              <fieldset className="flex min-w-0 items-center gap-1" aria-label="Hold">
                {cells.map((i) => (
                  <HoldCell
                    key={i}
                    step={i}
                    sixteenths={stepAt(i).hold}
                    set={stepAt(i).on}
                    onChange={(hold) => setStep(i, { hold })}
                  />
                ))}
              </fieldset>
            </div>
          );
        })}
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
  root,
  set,
  onChange,
}: {
  step: number;
  semis: number;
  /** The material's root note, so the cell can show the note. */
  root: number | null;
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
      title={`Step ${step + 1}: ${root === null ? '' : `${noteName(root + semis)}, `}${semis > 0 ? '+' : ''}${semis} semitones from the sample. Drag or use the arrows, Shift for an octave. Double-click to clear.`}
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
        step % BAR > 0 && step % 4 === 0 ? 'ml-2' : ''
      } ${tone}`}
    >
      {root !== null ? noteName(root + semis) : semis > 0 ? `+${semis}` : semis}
    </div>
  );
}

/** The longest a step can be held, in sixteenths. `HOLD_MAX` in Rust. */
const HOLD_MAX = 16;
/** How far a drag moves before the hold moves a sixteenth. */
const PX_PER_SIXTEENTH = 8;

/**
 * One step's hold, in sixteenths: how long its note is held before it
 * releases. A dot is the patch's own Hold time. It is heard only while the
 * patch's Length is Hold; under Loop and Sample nothing is gated.
 *
 * Edited like the pitch above it: drag, or focus and use the arrows;
 * Backspace, 0 or a double-click clears it. Focusable but not a button, so
 * Space still starts and stops playback.
 */
function HoldCell({
  step,
  sixteenths,
  set,
  onChange,
}: {
  step: number;
  sixteenths: number;
  set: boolean;
  onChange: (sixteenths: number) => void;
}) {
  const drag = useRef<{ y: number; from: number } | null>(null);
  const commit = (next: number) => {
    const clamped = Math.max(0, Math.min(HOLD_MAX, Math.round(next)));
    if (clamped !== sixteenths) onChange(clamped);
  };
  const tone =
    sixteenths === 0
      ? 'text-muted-foreground/40'
      : set
        ? 'text-foreground'
        : 'text-muted-foreground';

  return (
    <div
      role="spinbutton"
      tabIndex={0}
      aria-label={`Step ${step + 1} hold`}
      aria-valuemin={0}
      aria-valuemax={HOLD_MAX}
      aria-valuenow={sixteenths}
      title={
        sixteenths === 0
          ? `Step ${step + 1}: held for the patch's own Hold time. Drag or use the arrows to hold it for a number of sixteenths. Heard when the patch's Length is Hold.`
          : `Step ${step + 1}: held for ${sixteenths} sixteenth${sixteenths === 1 ? '' : 's'}. Double-click to go back to the patch's Hold.`
      }
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        drag.current = { y: e.clientY, from: sixteenths };
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (d) commit(d.from + (d.y - e.clientY) / PX_PER_SIXTEENTH);
      }}
      onPointerUp={() => {
        drag.current = null;
      }}
      onPointerCancel={() => {
        drag.current = null;
      }}
      onDoubleClick={() => commit(0)}
      onKeyDown={(e) => {
        const by = e.shiftKey ? 4 : 1;
        if (e.key === 'ArrowUp') commit(sixteenths + by);
        else if (e.key === 'ArrowDown') commit(sixteenths - by);
        else if (e.key === 'Backspace' || e.key === 'Delete' || e.key === '0') commit(0);
        else return;
        e.preventDefault();
      }}
      className={`flex h-5 w-6 shrink-0 cursor-ns-resize select-none items-center justify-center rounded-sm font-mono text-[10px] tabular-nums outline-none hover:bg-muted focus-visible:ring-1 focus-visible:ring-ring ${
        step % BAR > 0 && step % 4 === 0 ? 'ml-2' : ''
      } ${tone}`}
    >
      {sixteenths === 0 ? '·' : sixteenths}
    </div>
  );
}

/** Steps that are on past the track's end. */
function hiddenSteps(track: Track): number {
  return track.steps.slice(track.length).filter((s) => s.on).length;
}
