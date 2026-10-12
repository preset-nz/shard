import {
  DUCK_RELEASE_MAX,
  DUCK_RELEASE_MIN,
  type Duck,
  KIT_DECAY_MAX,
  KIT_DECAY_MIN,
  KIT_TUNE_RANGE,
  type Kit,
  type MixState,
} from '@/audio';
import { MixSwitch } from '@/components/MixSwitch';
import { NodeSwitch } from '@/components/NodeCard';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Slider } from '@/components/ui/slider';

/** A track's fader runs 0 to 200 %. `TRACK_GAIN_MAX` in Rust. */
const GAIN_MAX = 2;

/** Decay drawn on a log scale, so 100 % sits in the middle. */
const decayToSlider = (d: number) =>
  Math.log(d / KIT_DECAY_MIN) / Math.log(KIT_DECAY_MAX / KIT_DECAY_MIN);
const sliderToDecay = (t: number) => KIT_DECAY_MIN * (KIT_DECAY_MAX / KIT_DECAY_MIN) ** t;

/** Release drawn on a log scale, as Decay is. */
const releaseToSlider = (ms: number) =>
  Math.log(ms / DUCK_RELEASE_MIN) / Math.log(DUCK_RELEASE_MAX / DUCK_RELEASE_MIN);
const sliderToRelease = (t: number) =>
  DUCK_RELEASE_MIN * (DUCK_RELEASE_MAX / DUCK_RELEASE_MIN) ** t;

/** Depth as the dip in dB at full velocity: 0.75 is about −12 dB. */
const depthShown = (depth: number) =>
  depth >= 1 ? '−∞ dB' : `${(20 * Math.log10(1 - depth)).toFixed(1).replace('-', '−')} dB`;

const NONE = 'none';

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
 *
 * Every track has its sidechain (roadmap row 41): *Ducked by*, the track
 * whose steps dip its volume ("Set Ducked by"), and once ducked, Depth and
 * Release ("Set Depth", "Set Release"). Its K switch is Key only ("Set Key
 * only"): the track is silent but its steps still duck whatever it keys.
 */
export function TrackFader({
  name,
  detail,
  gain,
  selected,
  mix,
  kit,
  duck,
  keys,
  onSelect,
  onGain,
  onKit,
  onDuckedBy,
  onDuck,
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
  /** Its sidechain. */
  duck: Duck;
  /** The tracks that may duck it: every other one, by place and name. */
  keys: { place: number; name: string }[];
  onSelect: () => void;
  onGain: (gain: number) => void;
  /** A drums track's kit changed: its switch, Tune or Decay. */
  onKit?: (kit: Kit) => void;
  /** A *Ducked by* pick, by the key's place, or none. */
  onDuckedBy: (key: number | null) => void;
  /** Its Depth, Release or Key only changed. */
  onDuck: (duck: Duck) => void;
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
        // Not a slider, a switch or the picker, nor its list, whose clicks
        // bubble here through React from its portal.
        if (!t.closest('[role="slider"],[role="switch"],[role="combobox"],[role="listbox"]')) {
          onSelect();
        }
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
        <button
          type="button"
          role="switch"
          aria-checked={duck.key_only}
          aria-label={`${name} key only`}
          title={
            duck.key_only
              ? `${name} is Key only: silent, its steps still duck the tracks it keys`
              : `Key only: silence ${name} but keep its steps ducking the tracks it keys`
          }
          // Keep focus off the button, so Space still starts playback.
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onDuck({ ...duck, key_only: !duck.key_only })}
          className={`flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-sm text-[9px] font-semibold leading-none ${
            duck.key_only
              ? 'bg-primary text-primary-foreground'
              : 'text-muted-foreground hover:bg-accent hover:text-foreground'
          }`}
        >
          K
        </button>
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
      <div className="mt-1.5 flex items-center gap-2 text-xs text-muted-foreground">
        <span className="w-16 shrink-0">Ducked by</span>
        <Select
          items={{
            [NONE]: 'None',
            ...Object.fromEntries(keys.map((k) => [String(k.place), k.name])),
          }}
          value={duck.by === null ? NONE : String(duck.by)}
          onValueChange={(v) => {
            const key = v === NONE || v === null ? null : Number(v);
            if (key !== duck.by) onDuckedBy(key);
          }}
        >
          <SelectTrigger
            size="sm"
            aria-label={`${name} ducked by`}
            title="The track whose steps dip this one's volume"
            className="h-5 flex-1 px-1.5 text-[11px]"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE} className="text-xs">
              None
            </SelectItem>
            {keys.map((k) => (
              <SelectItem key={k.place} value={String(k.place)} className="text-xs">
                {k.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {duck.by !== null &&
        row(
          'Depth',
          <Slider
            min={0}
            max={1}
            step={0.01}
            value={[duck.depth]}
            onValueChange={(v) => onDuck({ ...duck, depth: one(v) })}
            aria-label={`${name} duck depth`}
            className="flex-1"
          />,
          depthShown(duck.depth),
        )}
      {duck.by !== null &&
        row(
          'Release',
          <Slider
            min={0}
            max={1}
            step={0.005}
            value={[releaseToSlider(duck.release)]}
            onValueChange={(v) => onDuck({ ...duck, release: sliderToRelease(one(v)) })}
            aria-label={`${name} duck release`}
            className="flex-1"
          />,
          `${Math.round(duck.release)} ms`,
        )}
    </div>
  );
}
