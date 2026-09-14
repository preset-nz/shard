import { useRef } from 'react';
import { setParam } from '@/audio';
import { NodeSwitch } from '@/components/NodeCard';
import type { ParamValues } from '@/scope';

/** `seq.length` is stepped 0, 1, 2. */
const LENGTHS = [4, 8, 16];

/**
 * The step sequencer's steps: one button per sixteenth, the one playing lit.
 *
 * A step that is on rewinds the material's pass, and the envelope with it.
 * Buttons keep a sixteenth's width whatever the length, so four steps read as
 * one beat rather than a bar stretched out. The title selects the node, whose
 * tempo, length and swing are ordinary rows in the inspector.
 *
 * The pattern is one number in the patch, a bit per step. Values arrive by
 * poll, so two quick clicks would both flip bits of the same stale number and
 * the first would be lost; the last number written stands in until the poll
 * catches up with it.
 */
export function StepStrip({
  values,
  step,
  selected,
  onSelect,
}: {
  values: ParamValues;
  /** The step playing, from zero, or -1. */
  step: number;
  selected: boolean;
  onSelect: () => void;
}) {
  const on = (values['seq.on'] ?? 0) >= 0.5;
  const length = LENGTHS[Math.min(2, Math.max(0, Math.round(values['seq.length'] ?? 2)))];
  const heard = Math.max(0, Math.round(values['seq.pattern'] ?? 0));
  const written = useRef<number | null>(null);
  if (written.current === heard) written.current = null;
  const pattern = written.current ?? heard;

  const toggle = (i: number) => {
    const next = pattern ^ (1 << i);
    written.current = next;
    void setParam('seq.pattern', next);
  };

  return (
    <div className={`flex items-center gap-2 ${on ? '' : 'opacity-60'}`}>
      <button
        type="button"
        onMouseDown={(e) => e.preventDefault()}
        onClick={onSelect}
        title="Show the sequencer's tempo, length and swing"
        className={`w-20 shrink-0 text-left text-[11px] font-semibold uppercase tracking-wider ${
          selected ? 'text-primary' : 'text-muted-foreground hover:text-foreground'
        }`}
      >
        Steps
      </button>
      <NodeSwitch
        label="Sequencer"
        on={on}
        onSwitch={(next) => void setParam('seq.on', next ? 1 : 0)}
      />
      <fieldset className="flex min-w-0 items-center gap-1" aria-label="Steps">
        {Array.from({ length }, (_, i) => {
          const set = ((pattern >> i) & 1) === 1;
          const playing = on && step === i;
          return (
            <button
              // biome-ignore lint/suspicious/noArrayIndexKey: a step is its index
              key={i}
              type="button"
              aria-pressed={set}
              aria-label={`Step ${i + 1}`}
              title={`Step ${i + 1}`}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => toggle(i)}
              className={`h-6 w-6 shrink-0 rounded-sm border transition-colors ${
                i > 0 && i % 4 === 0 ? 'ml-2' : ''
              } ${set ? 'border-primary bg-primary' : 'border-border hover:bg-muted'} ${
                playing ? 'ring-2 ring-foreground ring-offset-1 ring-offset-background' : ''
              }`}
            />
          );
        })}
      </fieldset>
    </div>
  );
}
