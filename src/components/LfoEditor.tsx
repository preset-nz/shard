import { useState } from 'react';
import {
  denormalise,
  format,
  type LfoLimits,
  type LfoRecord,
  normalise,
  type ParamInfo,
} from '@/audio';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Slider } from '@/components/ui/slider';

/** `quad-in-out` reads as "Quad in-out", `smooth-random` as "Smooth random". */
export function shapeLabel(name: string): string {
  const eased = name.match(/^(\w+)-(in-out|in|out)$/);
  const text = eased ? `${eased[1]} ${eased[2]}` : name.replace(/-/g, ' ');
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/**
 * The selected LFO's settings: name, rate, shape and phase.
 *
 * A rough sketch ahead of the panel redesign. Plain controls rather than a
 * facets scope, because an LFO is not a row in the parameter table. Every
 * change goes straight to Rust, which checks it and rebuilds the engine's set;
 * a running LFO keeps its place.
 *
 * The parent keys this by LFO id, so the name being typed never leaks into
 * another LFO.
 *
 * `compact` folds it to its name, rate and shape, for the Linked block under a
 * node; the name opens the full editor.
 */
export function LfoEditor({
  lfo,
  limits,
  linked = 0,
  compact = false,
  onOpen,
  onChange,
}: {
  lfo: LfoRecord;
  limits: LfoLimits;
  /** How many parameters follow this LFO. */
  linked?: number;
  compact?: boolean;
  /** Compact only: open this LFO in the inspector. */
  onOpen?: () => void;
  onChange: (next: LfoRecord) => void;
}) {
  const [name, setName] = useState(lfo.name);

  // Shaped like table rows so the same tapers and formatting apply.
  const rate: ParamInfo = {
    id: 'lfo.rate',
    name: 'Rate',
    min: limits.min_rate,
    max: limits.max_rate,
    default: 0.1,
    taper: 'exponential',
    steps: null,
    unit: 'Hz',
    smooth_ms: 0,
  };
  const phase: ParamInfo = {
    id: 'lfo.phase',
    name: 'Phase',
    min: 0,
    max: 1,
    default: 0,
    taper: 'linear',
    steps: null,
    unit: '%',
    smooth_ms: 0,
  };

  // Committed on Enter or on leaving the field, so a half-typed or empty
  // name never reaches Rust, which would refuse it.
  const commitName = () => {
    const trimmed = name.trim();
    if (trimmed && trimmed !== lfo.name) onChange({ ...lfo, name: trimmed });
    else setName(lfo.name);
  };

  const row = (def: ParamInfo, v: number, set: (next: number) => void) => (
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
        onValueChange={(v) => set(denormalise(def, Array.isArray(v) ? v[0] : (v as number)))}
      />
    </div>
  );

  const shapeSelect = (
    <div className="space-y-1">
      <span className="text-xs">Shape</span>
      <Select
        items={Object.fromEntries(limits.shapes.map((s) => [s, shapeLabel(s)]))}
        value={lfo.shape}
        onValueChange={(shape) => shape && onChange({ ...lfo, shape })}
      >
        <SelectTrigger className="h-7 w-full text-xs">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {limits.shapes.map((s) => (
            <SelectItem key={s} value={s} className="text-xs">
              {shapeLabel(s)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );

  if (compact) {
    return (
      <section className="space-y-3 px-3 pt-2 pb-3">
        <button
          type="button"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onOpen}
          title="Open this LFO"
          className="text-xs font-medium text-primary hover:underline"
        >
          ∿ {lfo.name}
        </button>
        {row(rate, lfo.rate, (v) => onChange({ ...lfo, rate: v }))}
        {shapeSelect}
      </section>
    );
  }

  return (
    <section className="space-y-3 px-3 pt-3 pb-3">
      <div className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        LFO
      </div>
      <Input
        aria-label="LFO name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        onBlur={commitName}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur();
          if (e.key === 'Escape') {
            setName(lfo.name);
            e.currentTarget.blur();
          }
        }}
        className="h-7 text-xs"
      />
      {row(rate, lfo.rate, (v) => onChange({ ...lfo, rate: v }))}
      {shapeSelect}
      {row(phase, lfo.phase, (v) => onChange({ ...lfo, phase: v }))}
      <p className="text-[11px] text-muted-foreground">
        {linked === 0
          ? 'Nothing follows it yet. Right-click a parameter to link it.'
          : `${linked} parameter${linked === 1 ? '' : 's'} follow${linked === 1 ? 's' : ''} it.`}
      </p>
    </section>
  );
}
