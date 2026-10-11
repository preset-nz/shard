import type { PatchInfo } from '@/audio';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

/**
 * The track's *Plays* picker (epic 10, story c): which patch the track plays
 * its steps on. Changing it is one undo step, "Set Plays". The caller sends
 * the pick and reads the patches again; this only draws the choice.
 */
export function PlaysPicker({
  patches,
  onPick,
  className = 'w-24',
}: {
  patches: PatchInfo[];
  onPick: (id: number) => void;
  className?: string;
}) {
  const played = patches.find((p) => p.played);
  if (!played) return null;
  return (
    <Select
      // The labels differ from the values, so the trigger needs the map.
      items={Object.fromEntries(patches.map((p) => [String(p.id), p.name]))}
      value={String(played.id)}
      onValueChange={(v) => {
        const id = Number(v);
        if (id !== played.id) onPick(id);
      }}
    >
      <SelectTrigger
        size="sm"
        aria-label="Plays"
        title="The patch this track plays"
        className={`h-5 px-1.5 text-[11px] ${className}`}
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {patches.map((p) => (
          <SelectItem key={p.id} value={String(p.id)} className="text-xs">
            {p.name}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
