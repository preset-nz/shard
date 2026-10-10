import type { Curve } from '@preset.nz/math';
import { ToggleGroup, ToggleGroupItem } from '@preset.nz/ux-kit';
import {
  type Adsr,
  adsrCurve,
  adsrOf,
  CurveEditor,
  constrainAdsr,
} from '@preset.nz/ux-kit/graphics';
import { useState } from 'react';
import {
  denormalise,
  type EnvelopeRecord,
  format,
  type LfoLimits,
  normalise,
  type ParamInfo,
  toCurve,
} from '@/audio';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Slider } from '@/components/ui/slider';

/**
 * How a curve drag reaches Rust: one drag is one undo step, so the editor
 * begins it, applies every move, and ends it (or cancels it on Escape).
 */
export interface EnvelopeGesture {
  begin: (id: number) => void;
  apply: (next: EnvelopeRecord) => void;
  end: () => void;
  cancel: () => void;
}

/** The axis never shows less than this, so a short envelope is still a shape. */
const MIN_AXIS_S = 0.5;
/** Headroom past the curve's end, so its last point can still be dragged out. */
const HEADROOM = 1.2;

/** A tidy axis ceiling (1, 2, 5 times a power of ten) above `end` plus headroom. */
function tidyCeiling(end: number): number {
  const want = Math.max(MIN_AXIS_S, end * HEADROOM);
  const mag = 10 ** Math.floor(Math.log10(want));
  for (const m of [1, 2, 5, 10]) if (m * mag >= want - 1e-9) return m * mag;
  return 10 * mag;
}

const curveEnd = (curve: Curve) => curve.points[curve.points.length - 1]?.x ?? 0;

const formatSeconds = (x: number) => (x < 1 ? `${Math.round(x * 1000)} ms` : `${x.toFixed(2)} s`);
const formatFitted = (x: number) => `${Math.round(x * 100)}% of the note`;
const formatLevel = (y: number) => `${Math.round(y * 100)}%`;

/**
 * The nearest ADSR to a free curve. The attack runs to the highest point;
 * the sustain level is the curve's sustain mark if it follows the peak, else
 * the point after the peak, else half. A curve with no points after the peak
 * gets short stages, which the user then drags to taste.
 */
function snapToAdsr(curve: Curve, time: 'seconds' | 'fitted'): Adsr {
  const pts = curve.points;
  const fallback = 0.1;
  let peak = 0;
  pts.forEach((p, i) => {
    if (p.y > (pts[peak]?.y ?? 0)) peak = i;
  });
  const marked = curve.sustain != null && curve.sustain > peak ? curve.sustain : null;
  const susIndex = marked ?? (peak + 1 < pts.length ? peak + 1 : null);
  const sus = susIndex == null ? null : (pts[susIndex] ?? null);
  const attack = pts[peak]?.x ?? fallback;
  const decay = sus ? Math.max(0, sus.x - attack) : fallback;
  const release = sus ? Math.max(0, curveEnd(curve) - sus.x) : fallback;
  const snapped = { attack, decay, sustain: sus ? sus.y : 0.5, release };
  const total = attack + decay + release;
  if (time === 'fitted' && total > 1) {
    return {
      ...snapped,
      attack: attack / total,
      decay: decay / total,
      release: release / total,
    };
  }
  return snapped;
}

/**
 * The selected modulation envelope: name, a curve, and for an ADSR its four
 * sliders.
 *
 * The curve is drawn and edited with ux-kit's `CurveEditor`. In seconds its x
 * is real time from the step that starts it; fitted, x runs 0 to 1 across the
 * note (or the pass), so it stretches with the step. At rest a linked row sits
 * on its low end, at the peak on its high end.
 *
 * An ADSR is four points with the sustain mark on the third; the sliders and
 * the editor both write those points. Free is any curve. Plain controls
 * rather than a facets scope, as the LFO editor is, because an envelope is
 * not a row in the parameter table. The parent keys this by id.
 *
 * `compact` is for the Linked block under a node: the curve at a shorter
 * height, without the name field, sliders or switches, and the name opens the
 * full editor (Georg, 2026-09-27).
 */
export function EnvelopeEditor({
  envelope,
  limits,
  linked = 0,
  compact = false,
  onOpen,
  onChange,
  gesture,
}: {
  envelope: EnvelopeRecord;
  limits: LfoLimits;
  /** How many parameters follow this envelope. */
  linked?: number;
  compact?: boolean;
  /** Compact only: open this envelope in the inspector. */
  onOpen?: () => void;
  /** `coalesce` is true for a slider or keyboard nudge, false for a discrete edit. */
  onChange: (next: EnvelopeRecord, coalesce: boolean) => void;
  gesture: EnvelopeGesture;
}) {
  const [name, setName] = useState(envelope.name);
  // The axis ceiling held while a drag is in flight, so it does not jump
  // under the pointer as the curve's end moves.
  const [heldCeiling, setHeldCeiling] = useState<number | null>(null);
  const [askSnap, setAskSnap] = useState(false);

  const curve = toCurve(envelope.curve);
  const fitted = envelope.time === 'fitted';
  const maxStageS = limits.max_stage_ms / 1000;
  const ceiling = fitted ? 1 : (heldCeiling ?? tidyCeiling(curveEnd(curve)));
  const adsr = envelope.mode === 'adsr' ? adsrOf(curve) : null;

  const edit = (next: Partial<EnvelopeRecord>, coalesce = false) =>
    onChange({ ...envelope, curve, ...next }, coalesce);

  // Shaped like table rows so the same tapers and formatting apply. A stage
  // at 1 ms is as good as none, and an exponential taper needs a floor above
  // zero. Fitted stages are a share of the note, linear.
  const stage = (id: string, label: string): ParamInfo => ({
    id: `envelope.${id}`,
    name: label,
    min: fitted ? 0 : 1,
    max: fitted ? 1 : limits.max_stage_ms,
    default: 100,
    taper: fitted ? 'linear' : 'exponential',
    steps: null,
    unit: fitted ? '%' : 'ms',
    smooth_ms: 0,
  });
  const sustain: ParamInfo = {
    id: 'envelope.sustain',
    name: 'Sustain',
    min: 0,
    max: 1,
    default: 0.2,
    taper: 'linear',
    steps: null,
    unit: '%',
    smooth_ms: 0,
  };

  // Committed on Enter or on leaving the field, so a half-typed or empty
  // name never reaches Rust, which would refuse it.
  const commitName = () => {
    const trimmed = name.trim();
    if (trimmed && trimmed !== envelope.name) edit({ name: trimmed });
    else setName(envelope.name);
  };

  /** Write one ADSR field from a slider. Fitted stages share one note, so the moved one yields. */
  const setStage = (key: keyof Adsr, v: number) => {
    if (!adsr) return;
    const next = { ...adsr };
    if (key === 'sustain') next.sustain = v;
    else if (fitted) {
      const others = (['attack', 'decay', 'release'] as const)
        .filter((k) => k !== key)
        .reduce((sum, k) => sum + adsr[k], 0);
      next[key] = Math.max(0, Math.min(v, 1 - others));
    } else next[key] = v / 1000;
    edit({ curve: adsrCurve(next) }, true);
  };

  const row = (def: ParamInfo, key: keyof Adsr) => {
    if (!adsr) return null;
    const v = key === 'sustain' || fitted ? adsr[key] : adsr[key] * 1000;
    return (
      <div className="space-y-1">
        <div className="flex items-baseline gap-2">
          <span className="flex-1 text-xs">{def.name}</span>
          <span className="shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums">
            {format(def, v)}
          </span>
        </div>
        <Slider
          min={0}
          max={1}
          step={0.001}
          value={[normalise(def, v)]}
          onValueChange={(t) =>
            setStage(key, denormalise(def, Array.isArray(t) ? t[0] : (t as number)))
          }
        />
      </div>
    );
  };

  const editor = (
    <CurveEditor
      value={curve}
      mode="envelope"
      aria-label={`${envelope.name} envelope`}
      className={compact ? 'h-28' : undefined}
      xDomain={[0, ceiling]}
      formatX={fitted ? formatFitted : formatSeconds}
      formatY={formatLevel}
      constrain={envelope.mode === 'adsr' ? constrainAdsr(fitted ? 1 : 3 * maxStageS) : undefined}
      onGestureStart={() => {
        setHeldCeiling(ceiling);
        gesture.begin(envelope.id);
      }}
      onGestureEnd={(committed) => {
        setHeldCeiling(null);
        if (committed) gesture.end();
        else gesture.cancel();
      }}
      onChange={(next, kind) => {
        const record = { ...envelope, curve: next };
        if (kind === 'gesture') gesture.apply(record);
        else onChange(record, kind === 'nudge');
      }}
    />
  );

  if (compact) {
    return (
      <section className="space-y-3 px-3 pt-2 pb-3">
        <button
          type="button"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onOpen}
          title="Open this envelope"
          className="text-xs font-medium text-primary hover:underline"
        >
          ⌒ {envelope.name}
        </button>
        {editor}
      </section>
    );
  }

  const switchMode = (mode: EnvelopeRecord['mode']) => {
    if (mode === envelope.mode) return;
    // Free to ADSR keeps the curve when it already is one; otherwise ask.
    if (mode === 'adsr' && !adsrOf(curve)) setAskSnap(true);
    else edit({ mode });
  };

  // Seconds to fitted puts the end on 1; fitted to seconds reads x as seconds,
  // so a whole note becomes one second.
  const switchTime = (time: EnvelopeRecord['time']) => {
    if (time === envelope.time) return;
    const end = curveEnd(curve);
    const scale = time === 'fitted' ? (end > 0 ? 1 / end : 1) : 1;
    const scaled: Curve = {
      ...curve,
      points: curve.points.map((p) => ({ ...p, x: p.x * scale })),
    };
    edit({ time, curve: scaled });
  };

  const pick =
    <T extends string>(set: (v: T) => void) =>
    (v: string[]) => {
      if (v[0]) set(v[0] as T);
    };

  return (
    <section className="space-y-3 px-3 pt-3 pb-3">
      <div className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        Mod envelope
      </div>
      <Input
        aria-label="Envelope name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        onBlur={commitName}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur();
          if (e.key === 'Escape') {
            setName(envelope.name);
            e.currentTarget.blur();
          }
        }}
        className="h-7 text-xs"
      />
      {editor}
      <div className="flex flex-wrap items-center gap-2">
        <ToggleGroup
          aria-label="Envelope shape"
          variant="outline"
          size="sm"
          spacing={0}
          value={[envelope.mode]}
          onValueChange={pick<EnvelopeRecord['mode']>(switchMode)}
        >
          <ToggleGroupItem value="adsr">ADSR</ToggleGroupItem>
          <ToggleGroupItem value="free">Free</ToggleGroupItem>
        </ToggleGroup>
        <ToggleGroup
          aria-label="Envelope time base"
          variant="outline"
          size="sm"
          spacing={0}
          value={[envelope.time]}
          onValueChange={pick<EnvelopeRecord['time']>(switchTime)}
        >
          <ToggleGroupItem value="seconds">Seconds</ToggleGroupItem>
          <ToggleGroupItem value="fitted">Fitted</ToggleGroupItem>
        </ToggleGroup>
      </div>
      {adsr && (
        <>
          {row(stage('attack', 'Attack'), 'attack')}
          {row(stage('decay', 'Decay'), 'decay')}
          {row(sustain, 'sustain')}
          {row(stage('release', 'Release'), 'release')}
        </>
      )}
      <p className="text-[11px] text-muted-foreground">
        Each step starts it, and it rests between steps. Seconds runs in real time. Fitted spans the
        note, so it stretches with it; under Hold the whole curve, release too, fits the held time.{' '}
        {linked === 0
          ? 'Nothing follows it yet. Right-click a parameter to link it.'
          : `${linked} parameter${linked === 1 ? '' : 's'} follow${linked === 1 ? 's' : ''} it.`}
      </p>
      {/* The kit has no AlertDialog yet, so this is its Dialog. */}
      <Dialog open={askSnap} onOpenChange={setAskSnap}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Snap to an ADSR?</DialogTitle>
            <DialogDescription>
              This curve isn't an ADSR shape. Snap it to the nearest ADSR?
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setAskSnap(false)}>
              Cancel
            </Button>
            <Button
              onClick={() => {
                setAskSnap(false);
                edit({ mode: 'adsr', curve: adsrCurve(snapToAdsr(curve, envelope.time)) });
              }}
            >
              Snap
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
