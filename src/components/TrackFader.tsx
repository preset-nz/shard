import type { MixState } from '@/audio';
import { MixSwitch } from '@/components/MixSwitch';
import { Slider } from '@/components/ui/slider';

/** A track's fader runs 0 to 200 %. `TRACK_GAIN_MAX` in Rust. */
const GAIN_MAX = 2;

/**
 * One track's card in the tracker view's Patches lane (epic 28): its name,
 * the patch it plays, its mute and solo, and its fader in the mix, a mixer's
 * channel strip. Clicking the card selects the track. A drag is one undo
 * step, "Set Gain"; mute and solo are no steps and land on the next bar.
 */
export function TrackFader({
  index,
  patchName,
  gain,
  selected,
  mix,
  onSelect,
  onGain,
  onMute,
  onSolo,
}: {
  index: number;
  patchName: string;
  gain: number;
  selected: boolean;
  /** Its mute and solo, pressed and heard. */
  mix: MixState | undefined;
  onSelect: () => void;
  onGain: (gain: number) => void;
  onMute: () => void;
  onSolo: () => void;
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
      <div className="flex items-center gap-1">
        <button
          type="button"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onSelect}
          title="Show this track's steps"
          className="flex min-w-0 flex-1 items-baseline gap-1.5 text-left"
        >
          <span className="text-xs font-medium text-foreground">Track {index + 1}</span>
          <span className="truncate text-[11px] text-muted-foreground">{patchName}</span>
        </button>
        <MixSwitch
          letter="M"
          verb="Mute"
          track={index}
          on={mix?.mute ?? false}
          heard={mix?.heard_mute ?? false}
          chord="⌃M"
          onToggle={onMute}
        />
        <MixSwitch
          letter="S"
          verb="Solo"
          track={index}
          on={mix?.solo ?? false}
          heard={mix?.heard_solo ?? false}
          chord="⌃S"
          onToggle={onSolo}
        />
      </div>
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
