import type { KitLane, MaterialView } from '@/audio';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

/** The synthesised drums a lane can play, as `lane.kind` names them. */
const DRUMS: Record<string, string> = { kick: 'Kick', snare: 'Snare', hat: 'Hat' };
/** A material lane whose material has gone from the pool: silent. */
const GONE = 'gone';
const materialValue = (id: number) => `m:${id}`;

/**
 * What a drums lane plays (epic 49, story d): a synthesised drum or a
 * material from the pool, one-shot from its trim. One undo step, "Set
 * Plays". A stop-gap on the grid's row until the kit view (story e).
 */
export function LanePlays({
  lane,
  materials,
  onPick,
}: {
  lane: KitLane;
  materials: MaterialView[];
  /** A drum by its kind, or `material` and the material's id. */
  onPick: (kind: string, material: number | null) => void;
}) {
  const current =
    lane.kind !== 'material'
      ? lane.kind
      : lane.material !== null && materials.some((m) => m.id === lane.material)
        ? materialValue(lane.material)
        : GONE;
  return (
    <Select
      // The labels differ from the values, so the trigger needs the map.
      items={{
        ...DRUMS,
        [GONE]: 'No material',
        ...Object.fromEntries(materials.map((m) => [materialValue(m.id), m.name])),
      }}
      value={current}
      onValueChange={(v) => {
        if (v === null || v === current || v === GONE) return;
        const m = materials.find((m) => materialValue(m.id) === v);
        if (m) onPick('material', m.id);
        else onPick(v, null);
      }}
    >
      <SelectTrigger
        size="sm"
        aria-label={`${lane.label} plays`}
        title="What this lane plays: a synthesised drum or a material"
        className="h-4 w-24 px-1.5 text-[10px]"
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {Object.entries(DRUMS).map(([kind, name]) => (
          <SelectItem key={kind} value={kind} className="text-xs">
            {name}
          </SelectItem>
        ))}
        {materials.map((m) => (
          <SelectItem key={m.id} value={materialValue(m.id)} className="text-xs">
            {m.name}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
