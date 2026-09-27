import { Plus } from 'lucide-react';
import type { Modulator } from '@/audio';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';

type Kind = Modulator['kind'];

/**
 * Modulators, the left-hand tree's second category under Materials: the
 * patch's LFOs and modulation envelopes. Click one to edit it in the panel;
 * right-click to remove it.
 *
 * A rough sketch ahead of the panel redesign and the node API's real tree.
 */
export function ModulatorTree({
  modulators,
  selected,
  linked,
  onSelect,
  onAdd,
  onRemove,
}: {
  /** LFOs first, then envelopes. */
  modulators: Modulator[];
  selected: { kind: Kind; id: number } | null;
  /** How many parameters follow each modulator, by id. */
  linked: Map<number, number>;
  onSelect: (next: { kind: Kind; id: number } | null) => void;
  onAdd: (kind: Kind) => void;
  onRemove: (m: Modulator) => void;
}) {
  const add = (kind: Kind, label: string, title: string) => (
    <button
      type="button"
      title={title}
      // Keep focus off the button, so Space still starts playback.
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => onAdd(kind)}
      className="flex items-center gap-0.5 rounded px-1 py-0.5 text-[10px] text-muted-foreground hover:bg-accent hover:text-foreground"
    >
      <Plus size={11} />
      {label}
    </button>
  );

  return (
    <div className="py-2">
      <div className="flex items-center gap-1 pr-2 pl-3 pb-1">
        <span className="flex-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Modulators
        </span>
        {add('lfo', 'LFO', 'Add an LFO')}
        {add('envelope', 'Env', 'Add a modulation envelope, started by each step')}
      </div>

      {modulators.length === 0 && (
        <p className="px-3 text-xs text-muted-foreground">
          No modulators yet. Add an LFO or an envelope with +.
        </p>
      )}

      {modulators.map((m) => {
        const count = linked.get(m.id) ?? 0;
        const isSelected = selected?.kind === m.kind && selected.id === m.id;
        return (
          <ContextMenu key={m.id} modal={false}>
            <ContextMenuTrigger asChild>
              <button
                type="button"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => onSelect(isSelected ? null : { kind: m.kind, id: m.id })}
                className={`flex w-full items-baseline gap-2 px-3 py-1 text-left text-xs ${
                  isSelected
                    ? 'bg-accent text-foreground'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                <span className="w-3 shrink-0 text-center" aria-hidden>
                  {m.kind === 'lfo' ? '∿' : '⌒'}
                </span>
                <span className="flex-1 truncate">{m.name}</span>
                <span className="shrink-0 text-[10px] tabular-nums">
                  {count > 0 ? `${count} linked` : ''}
                </span>
              </button>
            </ContextMenuTrigger>
            <ContextMenuContent className="w-48">
              <ContextMenuItem onSelect={() => onRemove(m)}>Remove {m.name}</ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        );
      })}
    </div>
  );
}
