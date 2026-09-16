import type { FieldRendererProps } from '@preset.nz/facets';
import { Fragment } from 'react';
import {
  denormalise,
  format,
  type KnobInfo,
  type MappingsView,
  type ModulationView,
  normalise,
  type ParamInfo,
} from '@/audio';
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
 * Knobs under a heading of "device · channel", which is the namespace these
 * boxes actually have: the LPD8's programs and the Launch Control's templates
 * are channels, and forty controls in one flat list say nothing about which
 * face of which box they are on. Order follows the list, so controls stay put
 * as they are learned.
 */
function groupKnobs(knobs: KnobInfo[]): [string, KnobInfo[]][] {
  const by = new Map<string, KnobInfo[]>();
  for (const k of knobs) {
    const where = `${k.device} · ${k.bank}`;
    const got = by.get(where);
    if (got) got.push(k);
    else by.set(where, [k]);
  }
  return [...by.entries()];
}

/**
 * One parameter: label, value and slider, and its link to an LFO.
 *
 * Registered with facets as the custom kind `param`, so the panel still
 * knows nothing about grains. The extra props it reads arrive on the field
 * definition built in scope.ts; the patch's links and what the engine heard
 * arrive on the panel's context, which changes every poll without rebuilding
 * the schema.
 *
 * Right-click links or unlinks. A linked row shows the LFO it follows, the two
 * ends of its sweep on a second slider, and a mark on the main slider where
 * the engine has moved the value to. The main slider stays the hand's, and is
 * what the value returns to when unlinked.
 *
 * Right-click also picks a MIDI knob, from the ones Settings knows, or learns
 * one by touch (Georg, 2026-09-14: picking is the main path). A mapped row
 * names its control, and while the knob is armed, waiting to pass through
 * the value, a hollow mark shows where the knob physically is so you can see
 * which way to turn.
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
  /** The active controller map. */
  midi: MappingsView;
  onLink: (id: string, lfo: number, lo: number, hi: number) => void;
  onUnlink: (id: string) => void;
  onLearn: (id: string) => void;
  onMapMidi: (id: string, control: number) => void;
  onForgetMidi: (id: string) => void;
}

/** A new link sweeps this far either side of where the hand is. */
const NEW_HALF_WIDTH = 0.125;

/** A sweep around `t`, kept inside the range. */
function around(t: number): [number, number] {
  const lo = Math.max(0, t - NEW_HALF_WIDTH);
  const hi = Math.min(1, t + NEW_HALF_WIDTH);
  return [lo, hi];
}

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
  const mapped = c.midi.mappings[def.id];
  const learning = c.midi.learning === def.id;

  return (
    <ContextMenu modal={false}>
      <ContextMenuTrigger asChild>
        <div className="space-y-1">
          <div className="flex items-baseline gap-2">
            <span className="flex-1 truncate text-xs">{f.label ?? def.name}</span>
            {learning && (
              <span className="shrink-0 animate-pulse text-[10px] text-primary">
                touch a control
              </span>
            )}
            {mapped && !learning && (
              <span
                className={`max-w-16 shrink truncate text-[10px] ${
                  mapped.armed ? 'text-muted-foreground' : 'text-primary'
                }`}
                title={
                  mapped.armed
                    ? `${mapped.control_name} is waiting: turn it past the value.`
                    : `${mapped.control_name} sets this. Right-click to forget.`
                }
              >
                ⦿ {mapped.control_name}
              </span>
            )}
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
            {mapped?.armed && mapped.knob !== null && (
              <div
                aria-hidden
                className="pointer-events-none absolute top-1/2 size-3 -translate-x-1/2 -translate-y-1/2 rounded-full border border-muted-foreground"
                style={{ left: `${mapped.knob * 100}%` }}
              />
            )}
          </div>
          {link && source && (
            <div className="flex items-center gap-2 pl-3">
              <span className="w-14 shrink-0 text-right font-mono text-[10px] text-muted-foreground tabular-nums">
                {format(def, denormalise(def, Math.min(link.lo, link.hi)))}
              </span>
              <Slider
                min={0}
                max={1}
                step={0.001}
                minStepsBetweenThumbs={1}
                value={[Math.min(link.lo, link.hi), Math.max(link.lo, link.hi)]}
                onValueChange={([lo, hi]) => c.onLink(def.id, link.lfo, lo, hi)}
                className="flex-1"
              />
              <span className="w-14 shrink-0 font-mono text-[10px] text-muted-foreground tabular-nums">
                {format(def, denormalise(def, Math.max(link.lo, link.hi)))}
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
                onSelect={() =>
                  source
                    ? c.onLink(def.id, l.id, link.lo, link.hi)
                    : c.onLink(def.id, l.id, ...around(t))
                }
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
        <ContextMenuSeparator />
        <ContextMenuSub>
          <ContextMenuSubTrigger>Control</ContextMenuSubTrigger>
          <ContextMenuSubContent className="max-h-80 overflow-y-auto">
            {groupKnobs(c.midi.knobs).map(([where, knobs]) => (
              <Fragment key={where}>
                <ContextMenuLabel className="text-[11px] font-normal text-muted-foreground">
                  {where}
                </ContextMenuLabel>
                {knobs.map((k) => (
                  <ContextMenuItem key={k.id} onSelect={() => c.onMapMidi(def.id, k.id)}>
                    {k.name}
                    {k.relative ? ' ∞' : ''}
                    {mapped?.control === k.id ? ' ✓' : ''}
                  </ContextMenuItem>
                ))}
              </Fragment>
            ))}
            {c.midi.knobs.length === 0 && (
              <ContextMenuLabel className="text-[11px] font-normal text-muted-foreground">
                No knobs known yet. Turn one, or name them in Settings.
              </ContextMenuLabel>
            )}
            <ContextMenuSeparator />
            <ContextMenuItem onSelect={() => c.onLearn(def.id)}>Learn by touch</ContextMenuItem>
            <ContextMenuItem disabled={!mapped} onSelect={() => c.onForgetMidi(def.id)}>
              None
            </ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
      </ContextMenuContent>
    </ContextMenu>
  );
}
