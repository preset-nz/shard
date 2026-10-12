import { KIT_DECAY_MAX, KIT_DECAY_MIN, KIT_TUNE_RANGE, type Kit, type MixState } from '@/audio';
import { MixSwitch } from '@/components/MixSwitch';
import { NodeSwitch } from '@/components/NodeCard';
import { Slider } from '@/components/ui/slider';

/** A track's fader runs 0 to 200 %. `TRACK_GAIN_MAX` in Rust. */
const GAIN_MAX = 2;

/** Decay drawn on a log scale, so 100 % sits in the middle. */
const decayToSlider = (d: number) =>
  Math.log(d / KIT_DECAY_MIN) / Math.log(KIT_DECAY_MAX / KIT_DECAY_MIN);
const sliderToDecay = (t: number) => KIT_DECAY_MIN * (KIT_DECAY_MAX / KIT_DECAY_MIN) ** t;

const one = (v: number | readonly number[]) => (Array.isArray(v) ? v[0] : (v as number));

/**
 * One track's card in the tracker view's Patches lane (epic 28): its name,
 * the patch it plays, its mute and solo, and its fader in the mix, a mixer's
 * channel strip. Clicking the card selects the track. A drag is one undo
 * step, "Set Gain"; mute and solo are no steps and land on the next bar.
 *
 * A drums track's card (story d, `kit` set) replaces the Drums card the
 * arrangement had: its switch in the header, and Tune and Decay under the
 * fader, each drag one step ("Set Tune", "Set Decay").
 */
export function TrackFader({
  name,
  detail,
  gain,
  selected,
  mix,
  kit,
  onSelect,
  onGain,
  onKit,
  onMute,
  onSolo,
}: {
  /** "Track 2", "Drums 1". */
  name: string;
  /** What it plays: a melody track's patch. */
  detail?: string;
  gain: number;
  selected: boolean;
  /** Its mute and solo, pressed and heard. */
  mix: MixState | undefined;
  /** A drums track's kit. */
  kit?: Kit | null;
  onSelect: () => void;
  onGain: (gain: number) => void;
  /** A drums track's kit changed: its switch, Tune or Decay. */
  onKit?: (kit: Kit) => void;
  onMute: () => void;
  onSolo: () => void;
}) {
  const row = (label: string, slider: React.ReactNode, shown: string) => (
    <div className="mt-1.5 flex items-center gap-2 text-xs text-muted-foreground">
      <span className="w-10">{label}</span>
      {slider}
      <span className="w-10 text-right font-mono text-[11px] tabular-nums">{shown}</span>
    </div>
  );
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: the title button is the keyboard path
    // biome-ignore lint/a11y/noStaticElementInteractions: see above
    <div
      data-node-card
      onClick={(e) => {
        const t = e.target as Element;
        if (!t.closest('[role="slider"]') && !t.closest('[role="switch"]')) onSelect();
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
          title={kit ? "Show this track's drums" : "Show this track's steps"}
          className="flex min-w-0 flex-1 items-baseline gap-1.5 text-left"
        >
          <span className="text-xs font-medium text-foreground">{name}</span>
          {detail && <span className="truncate text-[11px] text-muted-foreground">{detail}</span>}
        </button>
        {kit && onKit && (
          <NodeSwitch label={name} on={kit.on} onSwitch={(on) => onKit({ ...kit, on })} />
        )}
        <MixSwitch
          letter="M"
          verb="Mute"
          name={name}
          on={mix?.mute ?? false}
          heard={mix?.heard_mute ?? false}
          chord="⌃M"
          onToggle={onMute}
        />
        <MixSwitch
          letter="S"
          verb="Solo"
          name={name}
          on={mix?.solo ?? false}
          heard={mix?.heard_solo ?? false}
          chord="⌃S"
          onToggle={onSolo}
        />
      </div>
      {row(
        'Gain',
        <Slider
          min={0}
          max={GAIN_MAX}
          step={0.01}
          value={[gain]}
          onValueChange={(v) => onGain(one(v))}
          aria-label={`${name} gain`}
          className="flex-1"
        />,
        `${Math.round(gain * 100)}%`,
      )}
      {kit &&
        onKit &&
        row(
          'Tune',
          <Slider
            min={-KIT_TUNE_RANGE}
            max={KIT_TUNE_RANGE}
            step={0.1}
            value={[kit.tune]}
            onValueChange={(v) => onKit({ ...kit, tune: one(v) })}
            aria-label={`${name} tune`}
            className="flex-1"
          />,
          `${kit.tune > 0 ? '+' : ''}${kit.tune.toFixed(1)} st`,
        )}
      {kit &&
        onKit &&
        row(
          'Decay',
          <Slider
            min={0}
            max={1}
            step={0.005}
            value={[decayToSlider(kit.decay)]}
            onValueChange={(v) => onKit({ ...kit, decay: sliderToDecay(one(v)) })}
            aria-label={`${name} decay`}
            className="flex-1"
          />,
          `${Math.round(kit.decay * 100)}%`,
        )}
    </div>
  );
}
