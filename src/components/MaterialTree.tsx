import { Plus } from 'lucide-react';
import type { MaterialsView } from '@/audio';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';

/**
 * Materials, the first category of the left-hand tree: the WAV files in the
 * document's pool. One plays at a time, marked with a dot, and both
 * generators read it. Click one to play it and show its octave and trim;
 * right-click to remove it.
 */
export function MaterialTree({
  pool,
  inspecting,
  onSelect,
  onAdd,
  onRemove,
}: {
  pool: MaterialsView;
  /** Whether the inspector shows the material's octave and trim. */
  inspecting: boolean;
  onSelect: (id: number) => void;
  onAdd: () => void;
  onRemove: (id: number) => void;
}) {
  return (
    <div className="py-2">
      <div className="flex items-center pr-2 pl-3 pb-1">
        <span className="flex-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Materials
        </span>
        <button
          type="button"
          title="Add WAV files"
          // Keep focus off the button, so Space still starts playback.
          onMouseDown={(e) => e.preventDefault()}
          onClick={onAdd}
          className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          <Plus size={13} />
        </button>
      </div>

      {pool.materials.length === 0 && (
        <p className="px-3 text-xs text-muted-foreground">No materials yet. Add WAVs with +.</p>
      )}

      {pool.materials.map((m) => {
        const active = pool.active === m.id;
        return (
          <ContextMenu key={m.id} modal={false}>
            <ContextMenuTrigger asChild>
              <button
                type="button"
                title={m.missing ? `Not found: ${m.path}` : m.path}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => onSelect(m.id)}
                className={`flex w-full items-baseline gap-2 px-3 py-1 text-left text-xs ${
                  active && inspecting
                    ? 'bg-accent text-foreground'
                    : active
                      ? 'text-foreground'
                      : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                <span className={`shrink-0 text-[8px] ${active ? '' : 'invisible'}`}>●</span>
                <span className="flex-1 truncate">{m.name}</span>
                {m.missing && (
                  <span className="shrink-0 text-[10px] text-destructive">missing</span>
                )}
              </button>
            </ContextMenuTrigger>
            <ContextMenuContent className="w-48">
              <ContextMenuItem onSelect={() => onRemove(m.id)}>Remove {m.name}</ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        );
      })}
    </div>
  );
}
