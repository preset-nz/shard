import type { MaterialsView } from '@/audio';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

/** Radix refuses an empty value, so nothing wired gets a name of its own. */
const NONE = 'none';

/**
 * Which material a generator reads (Georg, 2026-09-15: *"wire the material
 * into the player or the granular node"*). On the Sample and Granular cards
 * and in their inspector. Nothing is silence.
 */
export function MaterialPicker({
  node,
  pool,
  onWire,
  className,
}: {
  /** `material` for Sample, `grain` for Granular. */
  node: string;
  pool: MaterialsView;
  onWire: (node: string, material: number | null) => void;
  className?: string;
}) {
  const wired = node === 'grain' ? pool.wires.grain : pool.wires.material;
  return (
    <Select
      value={wired === null ? NONE : String(wired)}
      onValueChange={(v) => onWire(node, v === NONE ? null : Number(v))}
    >
      <SelectTrigger
        size="sm"
        aria-label="Material"
        title="The material this generator reads"
        className={`h-7 w-full text-xs ${className ?? ''}`}
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={NONE} className="text-xs">
          Nothing (silent)
        </SelectItem>
        {pool.materials.map((m) => (
          <SelectItem key={m.id} value={String(m.id)} disabled={m.missing} className="text-xs">
            {m.missing ? `${m.name} (missing)` : m.name}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
