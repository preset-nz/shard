import { useEffect, useState } from 'react';
import type { Track, Tracker } from '@/audio';
import { NodeSwitch } from '@/components/NodeCard';
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
 * Tempo and swing belong to the song, the steps to its one track, and none of
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
      <div className="flex items-center gap-2">
        <span className="text-[11px] font-semibold uppercase tracking-wider">Tracker</span>
        <NodeSwitch label="The track" on={track.on} onSwitch={(on) => setTrack({ on })} />
      </div>

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

      {hiddenSteps(track) > 0 && (
        <span title="Kept from a longer track. Lengthen it to see and change them.">
          +{hiddenSteps(track)} past step {track.length}
        </span>
      )}
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
