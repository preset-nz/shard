import { PropertyPanel } from '@preset.nz/facets';
import {
  type LfoLimits,
  type LfoRecord,
  type MaterialsView,
  type MaterialView,
  type ParamInfo,
  setParam,
} from '@/audio';
import { LfoEditor } from '@/components/LfoEditor';
import { MaterialEditor } from '@/components/MaterialEditor';
import { MaterialPicker } from '@/components/MaterialPicker';
import { NodeSwitch } from '@/components/NodeCard';
import { NodePresets } from '@/components/NodePresets';
import type { ParamRowContext } from '@/components/ParamRow';
import {
  nodeById,
  type ParamValues,
  READS_MATERIAL,
  rowsOf,
  scopeKeyFor,
  switchIdOf,
} from '@/scope';
import type { Selection } from '@/stores/selection';

/**
 * The property panel: whatever is selected, and nothing when nothing is.
 *
 * A node shows its rows, and under them a **Linked** block with every LFO
 * those rows follow, folded down to rate and shape, so a node and what moves
 * it can be tuned together (Georg, 2026-09-14). A generator also picks its
 * material. A material shows its octave and trim, and an LFO its full editor.
 */
export function Inspector({
  selection,
  defs,
  values,
  ctx,
  pool,
  onWire,
  onMaterialChange,
  limits,
  linkedCounts,
  onSelect,
  onLfoChange,
  onError,
  onNote,
  onPresetApplied,
}: {
  selection: Selection;
  defs: ParamInfo[];
  values: ParamValues;
  ctx: ParamRowContext;
  pool: MaterialsView;
  onWire: (node: string, material: number | null) => void;
  onMaterialChange: (next: MaterialView) => void;
  limits: LfoLimits | null;
  /** How many parameters follow each LFO, by id. */
  linkedCounts: Map<number, number>;
  onSelect: (next: Selection) => void;
  onLfoChange: (next: LfoRecord) => void;
  onError: (message: string | null) => void;
  onNote: (message: string | null) => void;
  onPresetApplied: () => void;
}) {
  if (!selection) return null;

  if (selection.kind === 'lfo') {
    const lfo = ctx.mod.lfos.find((l) => l.id === selection.id);
    if (!lfo || !limits) return null;
    return (
      <LfoEditor
        key={lfo.id}
        lfo={lfo}
        limits={limits}
        linked={linkedCounts.get(lfo.id) ?? 0}
        onChange={onLfoChange}
      />
    );
  }

  if (selection.kind === 'material') {
    const material = pool.materials.find((m) => m.id === selection.id);
    if (!material) return null;
    return (
      <MaterialEditor
        key={material.id}
        material={material}
        pool={pool}
        onChange={onMaterialChange}
      />
    );
  }

  const node = nodeById(selection.id);
  if (!node) return null;
  const sw = switchIdOf(defs, node);
  const on = sw ? (values[sw] ?? 1) >= 0.5 : true;

  // Every LFO this node's rows follow, once each, in the tree's order.
  const followed = new Set(
    rowsOf(defs, node)
      .map((p) => ctx.mod.links[p.id]?.lfo)
      .filter((id) => id !== undefined),
  );
  const linked = ctx.mod.lfos.filter((l) => followed.has(l.id));

  const body = (
    <section>
      <div className="flex items-center gap-2 px-3 pt-3">
        <span className="flex-1 truncate text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          {node.label}
        </span>
        {sw && (
          <NodeSwitch
            label={node.label}
            on={on}
            onSwitch={(next) => void setParam(sw, next ? 1 : 0)}
          />
        )}
      </div>
      {READS_MATERIAL.has(node.id) && (
        <div className="px-3 pt-2">
          <MaterialPicker node={node.id} pool={pool} onWire={onWire} />
        </div>
      )}
      {/* Dimmed when off, but still editable: set it up, then switch it in. */}
      <div className={on ? '' : 'opacity-50'}>
        <PropertyPanel key={node.id} scopeKey={scopeKeyFor(node.id)} selection={values} ctx={ctx} />
      </div>
    </section>
  );

  return (
    <>
      {node.table ? (
        <NodePresets
          node={node.table}
          label={node.label}
          onError={onError}
          onNote={onNote}
          onApplied={onPresetApplied}
        >
          {body}
        </NodePresets>
      ) : (
        body
      )}
      {linked.length > 0 && limits && (
        <div className="border-t border-border">
          <div className="px-3 pt-3 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Linked
          </div>
          {linked.map((l) => (
            <LfoEditor
              key={l.id}
              lfo={l}
              limits={limits}
              compact
              onOpen={() => onSelect({ kind: 'lfo', id: l.id })}
              onChange={onLfoChange}
            />
          ))}
        </div>
      )}
    </>
  );
}
