import { Icons } from '@preset.nz/ux-kit';
import type { MaterialsView } from '@/audio';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';

/**
 * Materials, the first category of the left-hand tree: the WAV files in the
 * document's pool, and the built-in drone. Each generator reads the material
 * wired into it, picked on its card; a tag says which generators read which.
 * Click a material to edit its octave and trim; right-click to remove it.
 */
export function MaterialTree({
  pool,
  selected,
  onSelect,
  onAdd,
  onAddDrone,
  onRemove,
}: {
  pool: MaterialsView;
  /** The material the inspector shows, if any. */
  selected: number | null;
  onSelect: (id: number) => void;
  onAdd: () => void;
  onAddDrone: () => void;
  onRemove: (id: number) => void;
}) {
  const hasDrone = pool.materials.some((m) => m.path === null);

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
          <Icons.PlusIcon size={13} />
        </button>
      </div>

      {pool.materials.length === 0 && (
        <p className="px-3 text-xs text-muted-foreground">No materials. Add WAVs with +.</p>
      )}

      {pool.materials.map((m) => {
        const readers = [
          pool.wires.material === m.id ? { tag: 'S', name: 'Sample' } : null,
          pool.wires.grain === m.id ? { tag: 'G', name: 'Granular' } : null,
        ].filter((r) => r !== null);
        return (
          <ContextMenu key={m.id}>
            <ContextMenuTrigger
              render={
                <button
                  type="button"
                  title={
                    m.path === null
                      ? 'Built in, no file'
                      : m.missing
                        ? `Not found: ${m.path}`
                        : m.path
                  }
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => onSelect(m.id)}
                  className={`flex w-full items-baseline gap-1.5 px-3 py-1 text-left text-xs ${
                    selected === m.id
                      ? 'bg-accent text-foreground'
                      : readers.length > 0
                        ? 'text-foreground'
                        : 'text-muted-foreground hover:text-foreground'
                  }`}
                />
              }
            >
              <span className="flex-1 truncate">{m.name}</span>
              {m.missing && <span className="shrink-0 text-[10px] text-destructive">missing</span>}
              {readers.map((r) => (
                <span
                  key={r.tag}
                  title={`Read by ${r.name}`}
                  className="shrink-0 rounded bg-primary/15 px-1 text-[9px] font-semibold text-primary"
                >
                  {r.tag}
                </span>
              ))}
            </ContextMenuTrigger>
            <ContextMenuContent className="w-48">
              <ContextMenuItem onSelect={() => onRemove(m.id)}>Remove {m.name}</ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        );
      })}

      {!hasDrone && (
        <button
          type="button"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onAddDrone}
          className="px-3 pt-1 text-left text-[11px] text-muted-foreground hover:text-foreground"
        >
          Add the built-in drone
        </button>
      )}
    </div>
  );
}
