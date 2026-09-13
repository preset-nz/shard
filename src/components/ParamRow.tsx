import type { FieldRendererProps } from '@preset.nz/facets';
import { denormalise, format, type ParamInfo } from '@/audio';
import { Slider } from '@/components/ui/slider';

/**
 * One parameter: label, value and slider.
 *
 * Registered with facets as the custom kind `param`, so the panel still
 * knows nothing about grains. The extra props it reads arrive on the field
 * definition built in scope.ts.
 */
export interface ParamFieldExtras {
  def: ParamInfo;
}

export function ParamRow({ field, value, onChange }: FieldRendererProps) {
  const f = field as unknown as ParamFieldExtras & { label?: string; id: string };
  const { def } = f;
  const t = typeof value === 'number' ? value : 0;
  const real = denormalise(def, t);

  return (
    <div className="space-y-1">
      <div className="flex items-baseline gap-2">
        <span className="flex-1 truncate text-xs">{f.label ?? def.name}</span>
        <span className="shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums">
          {format(def, real)}
        </span>
      </div>
      <Slider
        min={0}
        max={1}
        step={0.001}
        value={[t]}
        onValueChange={([next]) => onChange?.(next)}
      />
    </div>
  );
}
