import { listen } from '@tauri-apps/api/event';
import { type KeyboardEvent, useEffect, useRef, useState } from 'react';
import {
  type Kit,
  type PatchInfo,
  type Step,
  type Track,
  type Tracker,
  type TrackerEdit,
  trackName,
} from '@/audio';
import { PlaysPicker } from '@/components/PlaysPicker';
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
 * Tempo and swing belong to the arrangement, the steps to the track the
 * scene view has selected, and none of it to the patch, so none of it is a
 * parameter row. Every track plays while the transport runs; this strip
 * shows and edits one. One button per
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
 *
 * A drums track (epic 28, story d) shows its grid, a row per lane,
 * instead of steps (`DrumsStrip`), and a melody track no longer shows the
 * drums under it.
 */
export function StepStrip(props: StripProps) {
  const track = props.tracker.tracks[props.index] ?? props.tracker.tracks[0];
  return track?.drums ? <DrumsStrip {...props} kit={track.drums} /> : <MelodyStrip {...props} />;
}

interface StripProps {
  tracker: Tracker;
  /** The track shown and edited, by its place. */
  index: number;
  /** The step that track is playing, from zero, or -1. */
  step: number;
  /** The Sample material's root note, a MIDI number, so pitches show as names. */
  root: number | null;
  /** The session's patches, for the *Plays* picker. */
  patches: PatchInfo[];
  /** The track now plays this patch. */
  onPlays: (id: number) => void;
  onChange: (next: Tracker) => void;
  /** A drums track's lane selection and renaming, which the Track menu acts on. */
  lanes: LaneUi;
  /** A whole-pattern edit on this track: clear, paste or repeat a bar, rotate, transpose. */
  onEdit: (edit: TrackerEdit) => void;
}

/** Which lane of the shown drums track is selected, and whether its name is being edited. */
export interface LaneUi {
  selected: number | null;
  renaming: boolean;
  onSelect: (lane: number) => void;
  onRenameStart: (lane: number) => void;
  /** The new name, or null when Escape cancelled. */
  onRename: (lane: number, name: string | null) => void;
}

/** The arrangement's tempo and swing, on every track's strip. */
function TempoSwing({
  tracker,
  onChange,
}: {
  tracker: Tracker;
  onChange: (next: Tracker) => void;
}) {
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
    <>
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
          onValueChange={(v) =>
            onChange({ ...tracker, swing: fromPercent(Array.isArray(v) ? v[0] : (v as number)) })
          }
          aria-label="Swing"
          className="w-20"
        />
        <span className="w-8 font-mono text-[11px] tabular-nums">
          {Math.round(toPercent(tracker.swing))}%
        </span>
      </div>
    </>
  );
}

/** A track's length: 4, 8, 16, 32 or 64 steps. */
function LengthPicker({ length, onPick }: { length: number; onPick: (n: number) => void }) {
  return (
    <fieldset className="flex items-center gap-0.5" aria-label="Length">
      {LENGTHS.map((n) => (
        <button
          key={n}
          type="button"
          aria-pressed={length === n}
          title={`${n} steps`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onPick(n)}
          className={`rounded px-1.5 py-0.5 font-mono text-[10px] tabular-nums ${
            length === n ? 'bg-primary/15 text-primary' : 'hover:text-foreground'
          }`}
        >
          {n}
        </button>
      ))}
    </fieldset>
  );
}

/** Replaces track `index` of `tracker` with `change` applied. */
const withTrack = (tracker: Tracker, index: number, change: Partial<Track>): Tracker => ({
  ...tracker,
  tracks: tracker.tracks.map((t, i) => (i === index ? { ...t, ...change } : t)),
});

/**
 * A drums track's strip: its name, the tempo and swing, its length and its
 * grid. Its switch, fader, Tune and Decay are its card in the Patches lane.
 */
function DrumsStrip({ tracker, index, step, onChange, lanes, kit }: StripProps & { kit: Kit }) {
  const track = tracker.tracks[index];
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground">
      <span className="text-[11px] font-semibold uppercase tracking-wider">
        {trackName(tracker.tracks, index)}
      </span>
      <TempoSwing tracker={tracker} onChange={onChange} />
      <LengthPicker
        length={track.length}
        onPick={(length) => onChange(withTrack(tracker, index, { length }))}
      />
      <KitGrid
        kit={kit}
        length={track.length}
        name={trackName(tracker.tracks, index)}
        step={step}
        lanes={lanes}
        onChange={(drums) => onChange(withTrack(tracker, index, { drums }))}
      />
    </div>
  );
}

function MelodyStrip({
  tracker,
  index,
  step,
  root,
  patches,
  onPlays,
  onChange,
  onEdit,
}: StripProps) {
  const track = tracker.tracks[index] ?? tracker.tracks[0];
  const setTrack = (change: Partial<Track>) => onChange(withTrack(tracker, index, change));
  const stepAt = (i: number): Step =>
    track.steps[i] ?? { on: false, pitch: 0, hold: 0, velocity: VELOCITY_MAX, nudge: 0 };
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
      case 'duplicate':
        onEdit({ op: 'duplicate' });
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
  // The Pattern menu's items arrive as app-kit's `command` event, carrying
  // their id. Read through a ref, so the listener is registered once and
  // still sees the track as it is now.
  const patternRef = useRef(pattern);
  patternRef.current = pattern;
  useEffect(() => {
    const unlisten = listen<string>('command', (e) => {
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
        d: 'duplicate',
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
      setStep(i, { on: false, pitch: 0, hold: 0, velocity: VELOCITY_MAX, nudge: 0 });
    } else {
      return;
    }
    handled();
  };

  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground">
      {/* No switch: every track plays while the transport runs. */}
      <span className="text-[11px] font-semibold uppercase tracking-wider">
        {trackName(tracker.tracks, index)}
      </span>

      <div className="flex items-center gap-1.5">
        <span>Plays</span>
        <PlaysPicker patches={patches} value={track.patch} onPick={onPlays} />
      </div>

      <TempoSwing tracker={tracker} onChange={onChange} />

      <LengthPicker length={track.length} onPick={(length) => setTrack({ length })} />

      <span
        className="font-mono text-[10px] tabular-nums"
        title="Keys: A W S E D F T G Y H U J K play C to C in this octave and enter the note on the focused step, then move on. Z and X change octave. Arrows move between steps. [ and ] rotate the steps. The Pattern menu copies, pastes, repeats, clears and transposes the bar of the step last focused (⌥⌘C, ⌥⌘V, ⌥⌘D, ⌥⌘⌫, ⌃⌘↑ and ⌃⌘↓), duplicates the pattern to twice its length (⌘D), and rotates the track (⌘[ and ⌘]). Delete clears a step."
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
                  const playing = step === i;
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
              <fieldset className="flex min-w-0 items-center gap-1" aria-label="Nudge">
                {cells.map((i) => (
                  <NumberCell
                    key={i}
                    kind={NUDGE}
                    step={i}
                    value={stepAt(i).nudge}
                    root={root}
                    set={stepAt(i).on}
                    onChange={(nudge) => setStep(i, { nudge })}
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

/** A hit at full velocity, and the ghost note a second click leaves. `HIT_MAX` and `HIT_GHOST` in Rust. */
const HIT_MAX = 127;
const HIT_GHOST = 48;
/** What a click on a drum step does: off, then full, then a ghost note, then off. */
const nextHit = (v: number) => (v === 0 ? HIT_MAX : v === HIT_MAX ? HIT_GHOST : 0);

/**
 * A drums track's grid (Georg, 2026-10-03): a row per lane, in the kit's
 * order and named for it (epic 49), a column a sixteenth, and any number of
 * lanes on one step.
 * It runs on the tracks' clock with the track's own length, so a bar of
 * drums loops under a longer line. Its switch, level, tune and decay are the
 * track's card.
 */
function KitGrid({
  kit,
  length,
  name,
  step,
  lanes,
  onChange,
}: {
  kit: Kit;
  /** The track's length, in steps. */
  length: number;
  /** The track's name, for the cells' labels. */
  name: string;
  /** The kit's step playing, from zero, or -1. */
  step: number;
  lanes: LaneUi;
  onChange: (next: Kit) => void;
}) {
  const bars = Array.from({ length: Math.ceil(length / BAR) }, (_, b) => b);
  const setHit = (lane: number, i: number, v: number) => {
    const hits = Array.from({ length: STEPS }, (_, j) => kit.lanes[lane]?.hits[j] ?? 0);
    hits[i] = v;
    onChange({ ...kit, lanes: kit.lanes.map((l, n) => (n === lane ? { ...l, hits } : l)) });
  };
  return (
    <div className="flex basis-full flex-col gap-2">
      {bars.map((bar) => {
        const first = bar * BAR;
        const cells = Array.from({ length: Math.min(BAR, length - first) }, (_, k) => first + k);
        return (
          <div key={bar} className="flex min-w-0 flex-col gap-1">
            {kit.lanes.map((lane, n) => (
              <fieldset
                // biome-ignore lint/suspicious/noArrayIndexKey: the webview has no lane id, only its place
                key={n}
                className="flex min-w-0 items-center gap-1"
                aria-label={`${name}, ${lane.label}, bar ${bar + 1}`}
              >
                {cells.map((i) => {
                  const v = lane.hits[i] ?? 0;
                  const playing = step === i;
                  const label = `${lane.label}, step ${i + 1}`;
                  return (
                    <button
                      key={i}
                      type="button"
                      aria-pressed={v > 0}
                      aria-label={label}
                      title={
                        v === 0
                          ? `${label}. Click for a hit, again for a ghost note.`
                          : `${label}: velocity ${v}. Click to ${v === HIT_MAX ? 'make it a ghost note' : 'clear it'}.`
                      }
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => setHit(n, i, nextHit(v))}
                      className={`h-4 w-6 shrink-0 rounded-sm border transition-colors ${
                        i % BAR > 0 && i % 4 === 0 ? 'ml-2' : ''
                      } ${
                        v === 0
                          ? 'border-border hover:bg-muted'
                          : v >= HIT_MAX
                            ? 'border-primary bg-primary'
                            : 'border-primary bg-primary/40'
                      } ${playing ? 'ring-2 ring-foreground ring-offset-1 ring-offset-background' : ''}`}
                    />
                  );
                })}
                {bar === 0 && (
                  <LaneName
                    label={lane.label}
                    selected={lanes.selected === n}
                    renaming={lanes.selected === n && lanes.renaming}
                    onSelect={() => lanes.onSelect(n)}
                    onRenameStart={() => lanes.onRenameStart(n)}
                    onRename={(name) => lanes.onRename(n, name)}
                  />
                )}
              </fieldset>
            ))}
          </div>
        );
      })}
    </div>
  );
}

/**
 * A lane's name beside its row: click to select the lane, double-click to
 * rename it in place. Enter or leaving the field commits, Escape cancels.
 */
function LaneName({
  label,
  selected,
  renaming,
  onSelect,
  onRenameStart,
  onRename,
}: {
  label: string;
  selected: boolean;
  renaming: boolean;
  onSelect: () => void;
  onRenameStart: () => void;
  onRename: (name: string | null) => void;
}) {
  const cancelled = useRef(false);
  if (renaming) {
    return (
      <input
        // biome-ignore lint/a11y/noAutofocus: the field exists because the person asked to rename
        autoFocus
        defaultValue={label}
        aria-label="Lane name"
        onFocus={(e) => {
          cancelled.current = false;
          e.currentTarget.select();
        }}
        onBlur={(e) => onRename(cancelled.current ? null : e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur();
          if (e.key === 'Escape') {
            cancelled.current = true;
            e.currentTarget.blur();
          }
        }}
        className="w-24 rounded-sm border border-border bg-transparent px-1 text-[10px] text-foreground"
      />
    );
  }
  return (
    <button
      type="button"
      aria-pressed={selected}
      title="Click to select the lane. Double-click to rename it."
      onClick={onSelect}
      onDoubleClick={onRenameStart}
      className={`rounded-sm px-1 text-[10px] ${
        selected ? 'bg-primary/15 text-primary' : 'hover:text-foreground'
      }`}
    >
      {label}
    </button>
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
  row: 'pitch' | 'hold' | 'velocity' | 'nudge';
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
/** The most a step can be nudged, in ms. `NUDGE_MAX_MS` in Rust. */
const NUDGE_MAX = 50;
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

const NUDGE: CellKind = {
  row: 'nudge',
  label: 'nudge',
  min: -NUDGE_MAX,
  max: NUDGE_MAX,
  rest: 0,
  pxPerUnit: 3,
  bigStep: 10,
  show: (v) => (v === 0 ? '·' : v > 0 ? `+${v}` : String(v)),
  title: (step, v) =>
    v === 0
      ? `Step ${step + 1}: on the grid. Drag or use the arrows to play it early (down) or late (up), in milliseconds.`
      : `Step ${step + 1}: ${Math.abs(v)} ms ${v < 0 ? 'early' : 'late'}. Double-click to put it back on the grid. Held to a fifth of a step.`,
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
