import { PropertyPanel } from '@preset.nz/facets';
import type {
  EnvelopeRecord,
  LfoLimits,
  LfoRecord,
  MaterialsView,
  MaterialView,
  ParamInfo,
} from '@/audio';
import { EnvelopeEditor } from '@/components/EnvelopeEditor';
import { LfoEditor } from '@/components/LfoEditor';
import { MaterialEditor } from '@/components/MaterialEditor';
import { isNodeOn, NodeHeader, WithNodePresets } from '@/components/NodeCard';
import type { ParamRowContext } from '@/components/ParamRow';
import { nodeById, type ParamValues, rowsOf, scopeKeyFor } from '@/scope';
import type { Selection } from '@/stores/selection';

/**
 * The property panel: whatever is selected, and nothing when nothing is.
 *
 * A node shows its rows, and under them a **Linked** block with every LFO or
 * envelope those rows follow, folded down, so a node and what moves it can be
 * tuned together (Georg, 2026-09-14). A generator's material picker is a row
 * of its scope. A material shows its octave and trim, and an LFO or envelope
 * its full editor.
 */
export function Inspector({
  selection,
  defs,
  values,
  ctx,
  pool,
  onMaterialChange,
  limits,
  linkedCounts,
  onSelect,
  onLfoChange,
  onEnvelopeChange,
  onError,
  onNote,
  onPresetApplied,
}: {
  selection: Selection;
  defs: ParamInfo[];
  values: ParamValues;
  ctx: ParamRowContext;
  pool: MaterialsView;
  onMaterialChange: (next: MaterialView) => void;
  limits: LfoLimits | null;
  /** How many parameters follow each modulator, by id. */
  linkedCounts: Map<number, number>;
  onSelect: (next: Selection) => void;
  onLfoChange: (next: LfoRecord) => void;
  onEnvelopeChange: (next: EnvelopeRecord) => void;
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

  if (selection.kind === 'envelope') {
    const envelope = ctx.mod.envelopes.find((e) => e.id === selection.id);
    if (!envelope || !limits) return null;
    return (
      <EnvelopeEditor
        key={envelope.id}
        envelope={envelope}
        limits={limits}
        linked={linkedCounts.get(envelope.id) ?? 0}
        onChange={onEnvelopeChange}
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
  const on = isNodeOn(defs, values, node);

  // Every modulator this node's rows follow, once each, in the tree's order.
  const followed = new Set(
    rowsOf(defs, node)
      .map((p) => ctx.mod.links[p.id]?.source)
      .filter((id) => id !== undefined),
  );
  const linked = ctx.mod.lfos.filter((l) => followed.has(l.id));
  const linkedEnvelopes = ctx.mod.envelopes.filter((e) => followed.has(e.id));

  return (
    <>
      <WithNodePresets
        node={node}
        onError={onError}
        onNote={onNote}
        onPresetApplied={onPresetApplied}
      >
        <section>
          <NodeHeader node={node} defs={defs} values={values} className="px-3 pt-3" />
          {/* Dimmed when off, but still editable: set it up, then switch it in. */}
          <div className={on ? '' : 'opacity-50'}>
            <PropertyPanel
              key={node.id}
              scopeKey={scopeKeyFor(node.id)}
              selection={values}
              ctx={ctx}
            />
          </div>
        </section>
      </WithNodePresets>
      {linked.length + linkedEnvelopes.length > 0 && limits && (
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
          {linkedEnvelopes.map((e) => (
            <EnvelopeEditor
              key={e.id}
              envelope={e}
              limits={limits}
              compact
              onOpen={() => onSelect({ kind: 'envelope', id: e.id })}
              onChange={onEnvelopeChange}
            />
          ))}
        </div>
      )}
    </>
  );
}
