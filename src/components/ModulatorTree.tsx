import { Plus } from 'lucide-react';
import type { LfoRecord } from '@/audio';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';

/**
 * Modulators, the left-hand tree's second category under Materials: the
 * patch's LFOs. Click one to edit it in the panel; right-click to remove it.
 *
 * A rough sketch ahead of the panel redesign and the node API's real tree.
 */
export function ModulatorTree({
  lfos,
  selected,
  linked,
  onSelect,
  onAdd,
  onRemove,
}: {
  lfos: LfoRecord[];
  selected: number | null;
  /** How many parameters follow each LFO, by id. */
  linked: Map<number, number>;
  onSelect: (id: number | null) => void;
  onAdd: () => void;
  onRemove: (id: number) => void;
}) {
  return (
    <div className="py-2">
      <div className="flex items-center pr-2 pl-3 pb-1">
        <span className="flex-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Modulators
        </span>
        <button
          type="button"
          title="Add an LFO"
          // Keep focus off the button, so Space still starts playback.
          onMouseDown={(e) => e.preventDefault()}
          onClick={onAdd}
          className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          <Plus size={13} />
        </button>
      </div>

      {lfos.length === 0 && (
        <p className="px-3 text-xs text-muted-foreground">No LFOs yet. Add one with +.</p>
      )}

      {lfos.map((l) => {
        const count = linked.get(l.id) ?? 0;
        return (
          <ContextMenu key={l.id} modal={false}>
            <ContextMenuTrigger asChild>
              <button
                type="button"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => onSelect(selected === l.id ? null : l.id)}
                className={`flex w-full items-baseline gap-2 px-3 py-1 text-left text-xs ${
                  selected === l.id
                    ? 'bg-accent text-foreground'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                <span className="flex-1 truncate">{l.name}</span>
                <span className="shrink-0 text-[10px] tabular-nums">
                  {count > 0 ? `${count} linked` : ''}
                </span>
              </button>
            </ContextMenuTrigger>
            <ContextMenuContent className="w-48">
              <ContextMenuItem onSelect={() => onRemove(l.id)}>Remove {l.name}</ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        );
      })}
    </div>
  );
}
