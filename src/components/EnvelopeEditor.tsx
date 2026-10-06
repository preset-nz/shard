import { useState } from 'react';
import {
  denormalise,
  type EnvelopeRecord,
  format,
  type LfoLimits,
  normalise,
  type ParamInfo,
} from '@/audio';
import { Input } from '@/components/ui/input';
import { Slider } from '@/components/ui/slider';

/**
 * The selected modulation envelope: name, attack, decay, sustain, release.
 *
 * It runs on the pass's clock, as the Envelope node does: a step starts it,
 * the release lands on the end of the pass, and between steps it rests. At
 * rest a linked row sits on its low end, at the peak on its high end.
 *
 * Plain controls rather than a facets scope, as the LFO editor is, because an
 * envelope is not a row in the parameter table. The parent keys this by id.
 *
 * `compact` is for the Linked block under a node: all four stages, without the
 * name field, and the name opens the full editor. An LFO folds further, but
 * an envelope's four stages are one shape and half of it is no use (Georg,
 * 2026-09-27).
 */
export function EnvelopeEditor({
  envelope,
  limits,
  linked = 0,
  compact = false,
  onOpen,
  onChange,
}: {
  envelope: EnvelopeRecord;
  limits: LfoLimits;
  /** How many parameters follow this envelope. */
  linked?: number;
  compact?: boolean;
  /** Compact only: open this envelope in the inspector. */
  onOpen?: () => void;
  onChange: (next: EnvelopeRecord) => void;
}) {
  const [name, setName] = useState(envelope.name);

  // Shaped like table rows so the same tapers and formatting apply. A stage
  // at 1 ms is as good as none, and an exponential taper needs a floor above
  // zero.
  const stage = (id: string, label: string): ParamInfo => ({
    id: `envelope.${id}`,
    name: label,
    min: 1,
    max: limits.max_stage_ms,
    default: 100,
    taper: 'exponential',
    steps: null,
    unit: 'ms',
    smooth_ms: 0,
  });
  const attack = stage('attack', 'Attack');
  const decay = stage('decay', 'Decay');
  const release = stage('release', 'Release');
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
    if (trimmed && trimmed !== envelope.name) onChange({ ...envelope, name: trimmed });
    else setName(envelope.name);
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

  const stages = (
    <>
      {row(attack, envelope.attack, (v) => onChange({ ...envelope, attack: v }))}
      {row(decay, envelope.decay, (v) => onChange({ ...envelope, decay: v }))}
      {row(sustain, envelope.sustain, (v) => onChange({ ...envelope, sustain: v }))}
      {row(release, envelope.release, (v) => onChange({ ...envelope, release: v }))}
    </>
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
        {stages}
      </section>
    );
  }

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
      {stages}
      <p className="text-[11px] text-muted-foreground">
        Each step starts it, and it rests between steps.{' '}
        {linked === 0
          ? 'Nothing follows it yet. Right-click a parameter to link it.'
          : `${linked} parameter${linked === 1 ? '' : 's'} follow${linked === 1 ? 's' : ''} it.`}
      </p>
    </section>
  );
}
