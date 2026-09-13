import type { FieldRendererProps } from '@preset.nz/facets';
import { denormalise, format, type ModulationView, normalise, type ParamInfo } from '@/audio';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';
import { Slider } from '@/components/ui/slider';

/**
 * One parameter: label, value and slider, and its link to an LFO.
 *
 * Registered with facets as the custom kind `param`, so the panel still
 * knows nothing about grains. The extra props it reads arrive on the field
 * definition built in scope.ts; the patch's links and what the engine heard
 * arrive on the panel's context, which changes every poll without rebuilding
 * the schema.
 *
 * Right-click links or unlinks. A linked row shows the LFO it follows, a depth
 * control, and a mark on the slider where the engine has moved the value to.
 * The slider itself stays the hand's.
 */
export interface ParamFieldExtras {
  def: ParamInfo;
}

/** What every row reads from the panel's context. */
export interface ParamRowContext {
  defs: ParamInfo[];
  mod: ModulationView;
  /** Each parameter as the engine last heard it, by id. */
  heard: Record<string, number>;
  onLink: (id: string, lfo: number, depth: number) => void;
  onUnlink: (id: string) => void;
}

/** Where a new link starts: enough to hear, not so much it lurches. */
const NEW_DEPTH = 0.25;

export function ParamRow({ field, value, onChange, ctx }: FieldRendererProps) {
  const f = field as unknown as ParamFieldExtras & { label?: string; id: string };
  const { def } = f;
  const c = ctx as ParamRowContext;
  const t = typeof value === 'number' ? value : 0;
  const real = denormalise(def, t);

  const link = c.mod.links[def.id];
  const source = link ? c.mod.lfos.find((l) => l.id === link.lfo) : undefined;
  const heard = c.heard[def.id];
  const mark = source && heard !== undefined ? normalise(def, heard) : null;

  return (
    <ContextMenu modal={false}>
      <ContextMenuTrigger asChild>
        <div className="space-y-1">
          <div className="flex items-baseline gap-2">
            <span className="flex-1 truncate text-xs">{f.label ?? def.name}</span>
            {link && (
              <span
                className={`max-w-24 shrink truncate text-[10px] ${
                  source ? 'text-primary' : 'text-destructive'
                }`}
                title={
                  source
                    ? `Follows ${source.name}. Right-click to change or unlink.`
                    : 'Follows an LFO this patch no longer has. Right-click to unlink.'
                }
              >
                ∿ {source ? source.name : 'missing LFO'}
              </span>
            )}
            <span className="shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums">
              {format(def, real)}
            </span>
          </div>
          <div className="relative">
            <Slider
              min={0}
              max={1}
              step={0.001}
              value={[t]}
              onValueChange={([next]) => onChange?.(next)}
            />
            {mark !== null && (
              <div
                aria-hidden
                className="pointer-events-none absolute top-1/2 h-3.5 w-0.5 -translate-x-1/2 -translate-y-1/2 rounded bg-primary"
                style={{ left: `${mark * 100}%` }}
              />
            )}
          </div>
          {link && source && (
            <div className="flex items-center gap-2 pl-3">
              <span className="shrink-0 text-[10px] text-muted-foreground">Depth</span>
              <Slider
                min={-1}
                max={1}
                step={0.01}
                value={[link.depth]}
                onValueChange={([d]) => c.onLink(def.id, link.lfo, d)}
                className="flex-1"
              />
              <span className="w-9 shrink-0 text-right font-mono text-[10px] text-muted-foreground tabular-nums">
                {`${link.depth > 0 ? '+' : ''}${Math.round(link.depth * 100)}%`}
              </span>
            </div>
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="w-52">
        <ContextMenuLabel>{def.name}</ContextMenuLabel>
        <ContextMenuSeparator />
        <ContextMenuSub>
          <ContextMenuSubTrigger disabled={c.mod.lfos.length === 0}>Link to</ContextMenuSubTrigger>
          <ContextMenuSubContent>
            {c.mod.lfos.map((l) => (
              <ContextMenuItem
                key={l.id}
                onSelect={() => c.onLink(def.id, l.id, source ? link.depth : NEW_DEPTH)}
              >
                {l.name}
                {link?.lfo === l.id ? ' ✓' : ''}
              </ContextMenuItem>
            ))}
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuItem disabled={!link} onSelect={() => c.onUnlink(def.id)}>
          Unlink
        </ContextMenuItem>
        {c.mod.lfos.length === 0 && (
          <ContextMenuLabel className="text-[11px] font-normal text-muted-foreground">
            Add an LFO under Modulators first.
          </ContextMenuLabel>
        )}
      </ContextMenuContent>
    </ContextMenu>
  );
}
