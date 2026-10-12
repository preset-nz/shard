import { Slider } from '@/components/ui/slider';

/** A track's fader runs 0 to 200 %. `TRACK_GAIN_MAX` in Rust. */
const GAIN_MAX = 2;

/**
 * One track's card in the tracker view's Patches lane (epic 28): its name,
 * the patch it plays, and its fader in the mix, a mixer's channel strip.
 * Clicking the card selects the track. A drag is one undo step, "Set Gain".
 */
export function TrackFader({
  index,
  patchName,
  gain,
  selected,
  onSelect,
  onGain,
}: {
  index: number;
  patchName: string;
  gain: number;
  selected: boolean;
  onSelect: () => void;
  onGain: (gain: number) => void;
}) {
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: the title button is the keyboard path
    // biome-ignore lint/a11y/noStaticElementInteractions: see above
    <div
      data-node-card
      onClick={(e) => {
        if (!(e.target as Element).closest('[role="slider"]')) onSelect();
      }}
      className={`rounded-md border px-3 py-2 transition-colors ${
        selected ? 'border-primary bg-primary/5' : 'border-border'
      }`}
    >
      <button
        type="button"
        onMouseDown={(e) => e.preventDefault()}
        onClick={onSelect}
        title="Show this track's steps"
        className="flex w-full items-baseline gap-1.5 text-left"
      >
        <span className="text-xs font-medium text-foreground">Track {index + 1}</span>
        <span className="truncate text-[11px] text-muted-foreground">{patchName}</span>
      </button>
      <div className="mt-1.5 flex items-center gap-2 text-xs text-muted-foreground">
        <span className="w-8">Gain</span>
        <Slider
          min={0}
          max={GAIN_MAX}
          step={0.01}
          value={[gain]}
          onValueChange={(v) => onGain(Array.isArray(v) ? v[0] : (v as number))}
          aria-label={`Track ${index + 1} gain`}
          className="flex-1"
        />
        <span className="w-9 text-right font-mono text-[11px] tabular-nums">
          {Math.round(gain * 100)}%
        </span>
      </div>
    </div>
  );
}
