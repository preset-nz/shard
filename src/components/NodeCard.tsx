import { PropertyPanel } from '@preset.nz/facets';
import type { ReactNode } from 'react';
import { type ParamInfo, setParam } from '@/audio';
import { NodePresets } from '@/components/NodePresets';
import type { ParamRowContext } from '@/components/ParamRow';
import { type NodeInfo, type ParamValues, scopeKeyFor, switchIdOf } from '@/scope';

/** A node's on/off switch. Switching keeps every setting. */
export function NodeSwitch({
  label,
  on,
  onSwitch,
}: {
  label: string;
  on: boolean;
  onSwitch: (on: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      title={`${label} is ${on ? 'on' : 'off'}. Switching it keeps its settings.`}
      // Keep focus off the button, so the next Space starts playback.
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => onSwitch(!on)}
      className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wider transition-colors ${
        on
          ? 'bg-primary/15 text-primary'
          : 'border border-border text-muted-foreground hover:text-foreground'
      }`}
    >
      {on ? 'On' : 'Off'}
    </button>
  );
}

/** Whether a node is switched on. A node with no switch always is. */
export function isNodeOn(defs: ParamInfo[], values: ParamValues, node: NodeInfo): boolean {
  const sw = switchIdOf(defs, node);
  return sw ? (values[sw] ?? 1) >= 0.5 : true;
}

/**
 * A node's name and switch, the same on its card and in the inspector. With
 * `onSelect` the name is a button that opens the node; without, a heading.
 */
export function NodeHeader({
  node,
  defs,
  values,
  onSelect,
  className = '',
}: {
  node: NodeInfo;
  defs: ParamInfo[];
  values: ParamValues;
  onSelect?: () => void;
  className?: string;
}) {
  const sw = switchIdOf(defs, node);
  return (
    <div className={`flex items-center gap-2 ${className}`}>
      {onSelect ? (
        <button
          type="button"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onSelect}
          className="min-w-0 flex-1 truncate text-left text-xs font-medium"
        >
          {node.label}
        </button>
      ) : (
        <span className="flex-1 truncate text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          {node.label}
        </span>
      )}
      {sw && (
        <NodeSwitch
          label={node.label}
          on={isNodeOn(defs, values, node)}
          onSwitch={(next) => void setParam(sw, next ? 1 : 0)}
        />
      )}
    </div>
  );
}

/** Right-click for the node's presets, when it is a patch node with a table prefix. */
export function WithNodePresets({
  node,
  onError,
  onNote,
  onPresetApplied,
  className,
  children,
}: {
  node: NodeInfo;
  onError: (message: string | null) => void;
  onNote: (message: string | null) => void;
  onPresetApplied: () => void;
  /** Passed to the presets wrapper; its default rules a line under the node. */
  className?: string;
  children: ReactNode;
}) {
  // Presets are saved in the patch and applied to the patch's own rows, so
  // an arrangement node has none yet (`design/arrangement-layer.md`).
  if (!node.table || node.layer !== 'patch') return <>{children}</>;
  return (
    <NodePresets
      node={node.table}
      label={node.label}
      className={className}
      onError={onError}
      onNote={onNote}
      onApplied={onPresetApplied}
    >
      {children}
    </NodePresets>
  );
}

/**
 * One node in the work area: the operator folded down.
 *
 * The card is the node's summary view, not a control of its own: the same
 * header as the inspector, and below it the facets summary scope, which holds
 * the material a generator reads and the node's level (Gain for a generator,
 * Mix for an effect). So the level on a card links to an LFO, takes a MIDI
 * knob and shows where modulation has moved it, as the inspector's row does.
 * A new node needs no card of its own. Click the title to open the whole
 * node in the inspector; right-click the card for its presets. Solo arrives
 * with roadmap row 14.
 */
export function NodeCard({
  node,
  defs,
  values,
  ctx,
  selected,
  onSelect,
  onError,
  onNote,
  onPresetApplied,
}: {
  node: NodeInfo;
  defs: ParamInfo[];
  values: ParamValues;
  ctx: ParamRowContext;
  selected: boolean;
  onSelect: () => void;
  onError: (message: string | null) => void;
  onNote: (message: string | null) => void;
  onPresetApplied: () => void;
}) {
  const on = isNodeOn(defs, values, node);
  return (
    <WithNodePresets
      node={node}
      onError={onError}
      onNote={onNote}
      onPresetApplied={onPresetApplied}
      className=""
    >
      <div
        data-node-card
        className={`rounded-md border transition-colors ${
          selected ? 'border-primary bg-primary/5' : 'border-border'
        }`}
      >
        <NodeHeader
          node={node}
          defs={defs}
          values={values}
          onSelect={onSelect}
          className="px-3 pt-2"
        />
        {/* Dimmed when off, but still editable, as in the inspector. */}
        <div className={on ? '' : 'opacity-50'}>
          <PropertyPanel scopeKey={scopeKeyFor(node.id, 'summary')} selection={values} ctx={ctx} />
        </div>
      </div>
    </WithNodePresets>
  );
}
