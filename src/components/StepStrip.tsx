import { listen } from '@tauri-apps/api/event';
import { type KeyboardEvent, useEffect, useRef, useState } from 'react';
import type { Step, Track, Tracker, TrackerEdit } from '@/audio';
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
  onEdit,
}: {
  tracker: Tracker;
  /** The step playing, from zero, or -1. */
  step: number;
  /** The Sample material's root note, a MIDI number, so pitches show as names. */
  root: number | null;
  onChange: (next: Tracker) => void;
  /** A whole-pattern edit: clear, paste or repeat a bar, rotate, transpose. */
  onEdit: (edit: TrackerEdit) => void;
}) {
  const track = tracker.tracks[0];
  const setTrack = (change: Partial<Track>) =>
    onChange({ ...tracker, tracks: [{ ...track, ...change }, ...tracker.tracks.slice(1)] });
  const stepAt = (i: number): Step =>
    track.steps[i] ?? { on: false, pitch: 0, hold: 0, velocity: VELOCITY_MAX };
  const setStep = (i: number, change: Partial<Step>) => {
    const steps = Array.from({ length: STEPS }, (_, j) => stepAt(j));
    steps[i] = { ...steps[i], ...change };
    setTrack({ steps });
  };
  const bars = Array.from({ length: Math.ceil(track.length / BAR) }, (_, b) => b);

  // Entering music from the keyboard, tracker-style. The octave the piano
  // keys sound in, a bar copied to paste, and the grid to move focus in.
  const [keyOctave, setKeyOctave] = useState(4);
  const clip = useRef<Step[] | null>(null);
  const grid = useRef<HTMLDivElement>(null);
  const focusStep = (row: string, i: number) =>
    grid.current?.querySelector<HTMLElement>(`[data-row="${row}"][data-step="${i}"]`)?.focus();

  // The step last focused, which the Pattern menu's commands act on.
  const cursor = useRef(0);

  /** One of the Pattern commands, on bar `bar`. The menu and the keys share it. */
  const pattern = (cmd: string, bar: number) => {
    switch (cmd) {
      case 'copy':
        clip.current = Array.from({ length: BAR }, (_, k) => stepAt(bar * BAR + k));
        break;
      case 'paste':
        if (clip.current) onEdit({ op: 'paste_bar', bar, steps: clip.current });
        break;
      case 'repeat':
        onEdit({ op: 'repeat_bar', from: bar });
        break;
      case 'clear':
        onEdit({ op: 'clear_bar', bar });
        break;
      case 'up':
        onEdit({ op: 'transpose', semis: 1, bar });
        break;
      case 'down':
        onEdit({ op: 'transpose', semis: -1, bar });
        break;
      case 'octave-up':
        onEdit({ op: 'transpose', semis: 12, bar });
        break;
      case 'octave-down':
        onEdit({ op: 'transpose', semis: -12, bar });
        break;
      case 'rotate-back':
        onEdit({ op: 'rotate', by: -1 });
        break;
      case 'rotate-forward':
        onEdit({ op: 'rotate', by: 1 });
        break;
      default:
        break;
    }
  };
  // Read through a ref, so the menu listener is registered once and still
  // sees the track as it is now.
  const patternRef = useRef(pattern);
  patternRef.current = pattern;
  useEffect(() => {
    const unlisten = listen<string>('menu', (e) => {
      if (e.payload.startsWith('pattern-')) {
        patternRef.current(e.payload.slice('pattern-'.length), Math.floor(cursor.current / BAR));
      }
    });
    return () => {
      void unlisten.then((off) => off());
    };
  }, []);

  const onGridKey = (e: KeyboardEvent<HTMLDivElement>) => {
    // A cell that handled the key itself (a pitch drag key, say) is done.
    if (e.defaultPrevented) return;
    const cell = (e.target as HTMLElement).closest<HTMLElement>('[data-step]');
    if (!cell) return;
    const i = Number(cell.dataset.step);
    const row = cell.dataset.row ?? 'steps';
    const bar = Math.floor(i / BAR);
    const handled = () => e.preventDefault();

    if (e.metaKey || e.ctrlKey) {
      // The menu's Pattern commands, by their keys. The menu owns these
      // accelerators on macOS and the keys never get here; this is for when
      // the menu could not be built.
      const keys: Record<string, string> = {
        c: 'copy',
        v: 'paste',
        d: 'repeat',
        backspace: 'clear',
        arrowup: e.shiftKey ? 'octave-up' : 'up',
        arrowdown: e.shiftKey ? 'octave-down' : 'down',
      };
      const cmd = keys[e.key.toLowerCase()];
      if (!cmd || (cmd === 'paste' && !clip.current)) return;
      pattern(cmd, bar);
      handled();
      return;
    }
    // Alt+arrows move the selected effect, not a step.
    if (e.altKey) return;

    if (e.key === 'ArrowLeft') {
      focusStep(row, Math.max(0, i - 1));
    } else if (e.key === 'ArrowRight') {
      focusStep(row, Math.min(track.length - 1, i + 1));
    } else if (e.key === '[' || e.key === ']') {
      onEdit({ op: 'rotate', by: e.key === ']' ? 1 : -1 });
    } else if (e.code === 'KeyZ') {
      setKeyOctave((o) => Math.max(0, o - 1));
    } else if (e.code === 'KeyX') {
      setKeyOctave((o) => Math.min(8, o + 1));
    } else if (e.code in PIANO) {
      // The key names a real note. Pitches are semitones from the sample, so
      // with a root note that is the note minus the root, and without one the
      // note is taken against middle C.
      const midi = 12 * (keyOctave + 1) + PIANO[e.code];
      setStep(i, { on: true, pitch: midi - (root ?? 60) });
      focusStep(row, Math.min(track.length - 1, i + 1));
    } else if (row === 'steps' && (e.key === 'Backspace' || e.key === 'Delete')) {
      setStep(i, { on: false, pitch: 0, hold: 0, velocity: VELOCITY_MAX });
    } else {
      return;
    }
    handled();
  };

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

      <span
        className="font-mono text-[10px] tabular-nums"
        title="Keys: A W S E D F T G Y H U J K play C to C in this octave and enter the note on the focused step, then move on. Z and X change octave. Arrows move between steps. [ and ] rotate the steps. The Pattern menu copies, pastes, repeats, clears and transposes the bar of the step last focused (⌥⌘C, ⌥⌘V, ⌘D, ⌥⌘⌫, ⌃⌘↑ and ⌃⌘↓), and rotates the track (⌘[ and ⌘]). Delete clears a step."
      >
        keys C{keyOctave}
      </span>

      {/* Steps over their pitches and holds, a column a sixteenth, like a
          tracker's columns under its triggers. A bar to a row. */}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: keys bubble up from the focusable cells */}
      <div
        ref={grid}
        onKeyDown={onGridKey}
        onFocus={(e) => {
          const at = (e.target as HTMLElement).closest<HTMLElement>('[data-step]');
          if (at) cursor.current = Number(at.dataset.step);
        }}
        className="flex min-w-0 flex-col gap-2"
      >
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
                      data-row="steps"
                      data-step={i}
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
                  <NumberCell
                    key={i}
                    kind={PITCH}
                    step={i}
                    value={stepAt(i).pitch}
                    root={root}
                    set={stepAt(i).on}
                    onChange={(pitch) => setStep(i, { pitch })}
                  />
                ))}
              </fieldset>
              <fieldset className="flex min-w-0 items-center gap-1" aria-label="Velocity">
                {cells.map((i) => (
                  <NumberCell
                    key={i}
                    kind={VELOCITY}
                    step={i}
                    value={stepAt(i).velocity}
                    root={root}
                    set={stepAt(i).on}
                    onChange={(velocity) => setStep(i, { velocity })}
                  />
                ))}
              </fieldset>
              <fieldset className="flex min-w-0 items-center gap-1" aria-label="Hold">
                {cells.map((i) => (
                  <NumberCell
                    key={i}
                    kind={HOLD}
                    step={i}
                    value={stepAt(i).hold}
                    root={root}
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

/**
 * The computer keyboard as a piano, in the tracker's layout: the home row is
 * the white keys and the row above the black ones. Semitones above C.
 */
const PIANO: Record<string, number> = {
  KeyA: 0,
  KeyW: 1,
  KeyS: 2,
  KeyE: 3,
  KeyD: 4,
  KeyF: 5,
  KeyT: 6,
  KeyG: 7,
  KeyY: 8,
  KeyH: 9,
  KeyU: 10,
  KeyJ: 11,
  KeyK: 12,
  KeyO: 13,
  KeyL: 14,
};

/** What a row of numbers under the steps is, so one cell can draw any of them. */
interface CellKind {
  /** `data-row`, and the name of the row's fieldset. */
  row: 'pitch' | 'hold' | 'velocity';
  label: string;
  min: number;
  max: number;
  /** The value that means "not set": drawn dim. */
  rest: number;
  /** How far a drag moves before the value moves by one, in pixels. */
  pxPerUnit: number;
  /** What Shift and the arrows move it by. */
  bigStep: number;
  /** What the cell shows, given the value and the material's root. */
  show: (value: number, root: number | null) => string;
  /** The tooltip. */
  title: (step: number, value: number, root: number | null) => string;
}

/** Semitones a step can be pitched, either way. `PITCH_RANGE` in Rust. */
const PITCH_RANGE = 24;
/** The longest a step can be held, in sixteenths. `HOLD_MAX` in Rust. */
const HOLD_MAX = 16;
/** A step at full velocity. `VELOCITY_MAX` in Rust. */
const VELOCITY_MAX = 127;

const PITCH: CellKind = {
  row: 'pitch',
  label: 'pitch',
  min: -PITCH_RANGE,
  max: PITCH_RANGE,
  rest: 0,
  pxPerUnit: 6,
  bigStep: 12,
  show: (v, root) => (root !== null ? noteName(root + v) : v > 0 ? `+${v}` : String(v)),
  title: (step, v, root) =>
    `Step ${step + 1}: ${root === null ? '' : `${noteName(root + v)}, `}${v > 0 ? '+' : ''}${v} semitones from the sample. Drag or use the arrows, Shift for an octave. Double-click to clear.`,
};

const HOLD: CellKind = {
  row: 'hold',
  label: 'hold',
  min: 0,
  max: HOLD_MAX,
  rest: 0,
  pxPerUnit: 8,
  bigStep: 4,
  show: (v) => (v === 0 ? '·' : String(v)),
  title: (step, v) =>
    v === 0
      ? `Step ${step + 1}: held for the patch's own Hold time. Drag or use the arrows to hold it for a number of sixteenths. Heard when the patch's Length is Hold.`
      : `Step ${step + 1}: held for ${v} sixteenth${v === 1 ? '' : 's'}. Double-click to go back to the patch's Hold.`,
};

const VELOCITY: CellKind = {
  row: 'velocity',
  label: 'velocity',
  min: 0,
  max: VELOCITY_MAX,
  rest: VELOCITY_MAX,
  pxPerUnit: 2,
  bigStep: 16,
  show: (v) => (v === VELOCITY_MAX ? '·' : String(v)),
  title: (step, v) =>
    v === VELOCITY_MAX
      ? `Step ${step + 1}: full velocity. Drag or use the arrows to play it softer.`
      : `Step ${step + 1}: velocity ${v} of ${VELOCITY_MAX}. Double-click to go back to full.`,
};

/**
 * One step's number in a row under the triggers: its pitch, hold or velocity.
 *
 * Drag up or down, or focus it and use the arrows, Shift for a bigger move.
 * Double-click, Backspace or 0 puts it back at rest. A focusable element
 * rather than a button or a number field, so Space still starts and stops
 * playback while a cell has focus.
 */
function NumberCell({
  kind,
  step,
  value,
  root,
  set,
  onChange,
}: {
  kind: CellKind;
  step: number;
  value: number;
  root: number | null;
  /** Whether the step above it is on; an off step's number is drawn quieter. */
  set: boolean;
  onChange: (value: number) => void;
}) {
  const drag = useRef<{ y: number; from: number } | null>(null);
  const commit = (next: number) => {
    const clamped = Math.max(kind.min, Math.min(kind.max, Math.round(next)));
    if (clamped !== value) onChange(clamped);
  };
  const tone =
    value === kind.rest
      ? 'text-muted-foreground/40'
      : set
        ? 'text-foreground'
        : 'text-muted-foreground';

  return (
    <div
      role="spinbutton"
      tabIndex={0}
      data-row={kind.row}
      data-step={step}
      aria-label={`Step ${step + 1} ${kind.label}`}
      aria-valuemin={kind.min}
      aria-valuemax={kind.max}
      aria-valuenow={value}
      title={kind.title(step, value, root)}
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        drag.current = { y: e.clientY, from: value };
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (d) commit(d.from + (d.y - e.clientY) / kind.pxPerUnit);
      }}
      onPointerUp={() => {
        drag.current = null;
      }}
      onPointerCancel={() => {
        drag.current = null;
      }}
      onDoubleClick={() => commit(kind.rest)}
      onKeyDown={(e) => {
        // ⌘, Ctrl and Alt are the strip's bar and effect shortcuts.
        if (e.metaKey || e.ctrlKey || e.altKey) return;
        const by = e.shiftKey ? kind.bigStep : 1;
        if (e.key === 'ArrowUp') commit(value + by);
        else if (e.key === 'ArrowDown') commit(value - by);
        else if (e.key === 'Backspace' || e.key === 'Delete' || e.key === '0') commit(kind.rest);
        else return;
        e.preventDefault();
      }}
      className={`flex h-5 w-6 shrink-0 cursor-ns-resize select-none items-center justify-center rounded-sm font-mono text-[10px] tabular-nums outline-none hover:bg-muted focus-visible:ring-1 focus-visible:ring-ring ${
        step % BAR > 0 && step % 4 === 0 ? 'ml-2' : ''
      } ${tone}`}
    >
      {kind.show(value, root)}
    </div>
  );
}

/** Steps that are on past the track's end. */
function hiddenSteps(track: Track): number {
  return track.steps.slice(track.length).filter((s) => s.on).length;
}
