import type { Curve } from '@preset.nz/math';
import {
  type Adsr,
  adsrCurve,
  adsrOf,
  type CurveConstraint,
  CurveEditor,
  constrainAdsr,
} from '@preset.nz/ux-kit/graphics';
import { useRef, useState } from 'react';
import {
  denormalise,
  type EnvelopeRecord,
  format,
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
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group';

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

/** The curve spans the note, so x is always 0 to 1. */
const X_DOMAIN: [number, number] = [0, 1];

const curveEnd = (curve: Curve) => curve.points[curve.points.length - 1]?.x ?? 0;

const formatX = (x: number) => `${Math.round(x * 100)}% of the note`;
const formatLevel = (y: number) => `${Math.round(y * 100)}%`;

type Stage = 'attack' | 'decay' | 'release';
const STAGES: Stage[] = ['attack', 'decay', 'release'];

/**
 * `from` with one stage set to `v`, the others giving way if the three no
 * longer fit in the note. They shrink in proportion and keep the sustain
 * level, as the Envelope node's stages are fitted to the pass, so any stage
 * can grow at once instead of waiting for the others to be made shorter
 * first (Georg, 2026-10-10).
 */
function makeRoom(from: Adsr, key: Stage, v: number): Adsr {
  const moved = Math.min(1, Math.max(0, v));
  const others = STAGES.filter((k) => k !== key);
  const rest = others.reduce((sum, k) => sum + from[k], 0);
  const next = { ...from, [key]: moved };
  if (moved + rest > 1 && rest > 0) {
    const k = (1 - moved) / rest;
    for (const o of others) next[o] = from[o] * k;
  }
  return next;
}

/**
 * The ADSR limits for a drag on the curve: the stage whose end moved takes
 * its new length, and the others make room as `makeRoom` does, from the
 * shape the drag started with, so dragging back gives them back.
 */
const fitAdsr: CurveConstraint = (next, prev) => {
  const shaped = constrainAdsr()(next, prev);
  const was = adsrOf(prev);
  const now = shaped && adsrOf(shaped);
  if (!shaped || !was || !now) return shaped;
  const key = STAGES.find((k) => Math.abs(now[k] - was[k]) > 1e-9);
  // Only the sustain level moved.
  if (!key) return shaped;
  return adsrCurve({ ...makeRoom(was, key, now[key]), sustain: now.sustain });
};

/**
 * The nearest ADSR to a free curve. The attack runs to the highest point;
 * the sustain level is the curve's sustain mark if it follows the peak, else
 * the point after the peak, else half. A curve with no points after the peak
 * gets short stages, which the user then drags to taste.
 */
function snapToAdsr(curve: Curve): Adsr {
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
  if (total > 1) {
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
 * The curve is drawn and edited with ux-kit's `CurveEditor`. Its x runs 0 to 1
 * across the note (or the pass), so it stretches with the step. At rest a
 * linked row sits on its low end, at the peak on its high end.
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
  linked = 0,
  compact = false,
  onOpen,
  onChange,
  gesture,
}: {
  envelope: EnvelopeRecord;
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
  const [askSnap, setAskSnap] = useState(false);

  const curve = toCurve(envelope.curve);
  const adsr = envelope.mode === 'adsr' ? adsrOf(curve) : null;

  const edit = (next: Partial<EnvelopeRecord>, coalesce = false) =>
    onChange({ ...envelope, curve, ...next }, coalesce);

  // Shaped like table rows so the same formatting applies. A stage is a
  // share of the note, linear.
  const stage = (id: string, label: string): ParamInfo => ({
    id: `envelope.${id}`,
    name: label,
    description: `How much of the note the ${label.toLowerCase()} stage takes.`,
    min: 0,
    max: 1,
    default: 0.1,
    taper: 'linear',
    steps: null,
    unit: '%',
    smooth_ms: 0,
  });
  const sustain: ParamInfo = {
    id: 'envelope.sustain',
    name: 'Sustain',
    description: 'The level held after the decay, until the note ends.',
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

  // The shape a slider drag started from, so the stages that made room get
  // it back when the slider returns.
  const slid = useRef<Adsr | null>(null);

  /** Write one ADSR field from a slider. The stages share one note, so the others make room. */
  const setStage = (key: keyof Adsr, v: number) => {
    if (!adsr) return;
    slid.current ??= adsr;
    const from = slid.current;
    const next =
      key === 'sustain'
        ? { ...adsr, sustain: v }
        : { ...makeRoom(from, key, v), sustain: adsr.sustain };
    edit({ curve: adsrCurve(next) }, true);
  };

  const row = (def: ParamInfo, key: keyof Adsr) => {
    if (!adsr) return null;
    const v = adsr[key];
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
          onValueCommitted={() => {
            slid.current = null;
          }}
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
      xDomain={X_DOMAIN}
      formatX={formatX}
      formatY={formatLevel}
      constrain={envelope.mode === 'adsr' ? fitAdsr : undefined}
      onGestureStart={() => gesture.begin(envelope.id)}
      onGestureEnd={(committed) => {
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
        Each step starts it, and it rests between steps. The curve spans the note, or one pass of
        the sample under Sample and Loop; under Hold the whole curve, release too, fits the held
        time.{' '}
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
                edit({ mode: 'adsr', curve: adsrCurve(snapToAdsr(curve)) });
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
