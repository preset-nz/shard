import type { FieldRendererProps } from '@preset.nz/facets';
import { Waves } from 'lucide-react';
import { denormalise, format, type ParamInfo } from '@/audio';
import { Slider } from '@/components/ui/slider';

/**
 * One parameter: label, value, slider, and a toggle that hands it to the
 * drift oscillator.
 *
 * Registered with facets as the custom kind `param`, so the panel still
 * knows nothing about grains. The extra props it reads arrive on the field
 * definition built in scope.ts.
 *
 * The toggle is the interesting part. A drifting parameter is not disabled,
 * it is *owned by something else* — the slider keeps showing where it is, it
 * just moves on its own. That distinction is why this is a toggle per row and
 * not a global mode.
 */
export interface ParamFieldExtras {
  def: ParamInfo;
  drifting: boolean;
  onDriftToggle: (id: string, on: boolean) => void;
}

export function ParamRow({ field, value, onChange }: FieldRendererProps) {
  const f = field as unknown as ParamFieldExtras & { label?: string; id: string };
  const { def, drifting, onDriftToggle } = f;
  const t = typeof value === 'number' ? value : 0;
  const real = denormalise(def, t);

  return (
    <div className="space-y-1">
      <div className="flex items-baseline gap-2">
        <span className="flex-1 truncate text-xs">{f.label ?? def.name}</span>
        <span className="shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums">
          {format(def, real)}
        </span>
        {def.can_drift && (
          <button
            type="button"
            title={
              drifting ? 'Drifting — click to take it back' : 'Hand this to the drift oscillator'
            }
            aria-pressed={drifting}
            onClick={() => onDriftToggle(def.id, !drifting)}
            className={`shrink-0 rounded p-0.5 transition-colors ${
              drifting ? 'text-primary' : 'text-muted-foreground/40 hover:text-muted-foreground'
            }`}
          >
            <Waves size={13} />
          </button>
        )}
      </div>
      <Slider
        min={0}
        max={1}
        step={0.001}
        value={[t]}
        disabled={drifting}
        onValueChange={([next]) => onChange?.(next)}
        className={drifting ? 'opacity-60' : undefined}
      />
    </div>
  );
}
