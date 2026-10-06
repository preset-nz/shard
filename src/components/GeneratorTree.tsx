import { AddMenu } from '@/components/AddMenu';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';
import type { NodeInfo } from '@/scope';

/**
 * Generators, in the left-hand tree: the ones in the patch, and + to add one
 * of a type (Georg, 2026-09-27: *"add generator of type"*).
 *
 * One of each type for now, because the parameter table has one set of rows
 * per generator. Adding switches that generator on and removing switches it
 * off, so a generator that is off is not in the patch. Several of one type
 * waits for the node API's instances.
 */
export function GeneratorTree({
  generators,
  selected,
  onSelect,
  onAdd,
  onRemove,
}: {
  /** Every generator type, in work-area order, and whether it is in the patch. */
  generators: Array<{ node: NodeInfo; on: boolean }>;
  selected: string | null;
  onSelect: (id: string | null) => void;
  onAdd: (id: string) => void;
  onRemove: (node: NodeInfo) => void;
}) {
  const present = generators.filter((g) => g.on);
  return (
    <div className="py-2">
      <div className="flex items-center pr-2 pl-3 pb-1">
        <span className="flex-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Generators
        </span>
        <AddMenu
          title="Add a generator"
          choices={generators.map((g) => ({
            kind: g.node.id,
            label: g.node.label,
            disabled: g.on ? 'in the patch' : undefined,
          }))}
          onAdd={onAdd}
        />
      </div>

      {present.length === 0 && (
        <p className="px-3 text-xs text-muted-foreground">Silent. Add a generator with +.</p>
      )}

      {present.map(({ node }) => (
        <ContextMenu key={node.id}>
          <ContextMenuTrigger
            render={
              <button
                type="button"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => onSelect(selected === node.id ? null : node.id)}
                className={`flex w-full items-baseline gap-2 px-3 py-1 text-left text-xs ${
                  selected === node.id
                    ? 'bg-accent text-foreground'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              />
            }
          >
            <span className="flex-1 truncate">{node.label}</span>
          </ContextMenuTrigger>
          <ContextMenuContent className="w-48">
            <ContextMenuItem onClick={() => onRemove(node)}>Remove {node.label}</ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      ))}
    </div>
  );
}
