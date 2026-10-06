import { PropertyPanel } from '@preset.nz/facets';
import { Icons } from '@preset.nz/ux-kit';
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
 * What a palette effect's card adds to its header: a place earlier, a place
 * later, and out of the chain. Taking it out keeps its settings. Each is also
 * a key (Alt+Up, Alt+Down, Delete), shown in the tooltip.
 */
export function ChainButtons({
  label,
  onMove,
  onRemove,
}: {
  label: string;
  onMove: (by: number) => void;
  onRemove: () => void;
}) {
  const button = 'rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground';
  return (
    <span data-chain-buttons className="flex shrink-0 items-center">
      <button
        type="button"
        title={`Move ${label} earlier in the chain (⌥↑)`}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => onMove(-1)}
        className={button}
      >
        <Icons.CaretUpIcon size={12} />
      </button>
      <button
        type="button"
        title={`Move ${label} later in the chain (⌥↓)`}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => onMove(1)}
        className={button}
      >
        <Icons.CaretDownIcon size={12} />
      </button>
      <button
        type="button"
        title={`Take ${label} out of the chain. Its settings are kept. (⌫)`}
        onMouseDown={(e) => e.preventDefault()}
        onClick={onRemove}
        className={button}
      >
        <Icons.XIcon size={12} />
      </button>
    </span>
  );
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
  actions,
  className = '',
}: {
  node: NodeInfo;
  defs: ParamInfo[];
  values: ParamValues;
  onSelect?: () => void;
  /** After the switch: a palette effect's move and remove buttons. */
  actions?: ReactNode;
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
      {actions}
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
 * A new node needs no card of its own. Click anywhere on the card to open the
 * whole node in the inspector; right-click it for its presets. Solo arrives
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
  onMove,
  onRemove,
}: {
  node: NodeInfo;
  defs: ParamInfo[];
  values: ParamValues;
  ctx: ParamRowContext;
  selected: boolean;
  onSelect: () => void;
  /** For a palette effect: move it a place, or take it out of the chain. */
  onMove?: (by: number) => void;
  onRemove?: () => void;
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
      {/* The title stays the keyboard's way in; the card is a larger target
          for the pointer. The switch only switches. */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: the title button is the keyboard path */}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: see above */}
      <div
        data-node-card
        onClick={(e) => {
          if (!(e.target as Element).closest('[role="switch"], [data-chain-buttons]')) onSelect();
        }}
        className={`rounded-md border transition-colors ${
          selected ? 'border-primary bg-primary/5' : 'border-border'
        }`}
      >
        <NodeHeader
          node={node}
          defs={defs}
          values={values}
          onSelect={onSelect}
          actions={
            onMove && onRemove ? (
              <ChainButtons label={node.label} onMove={onMove} onRemove={onRemove} />
            ) : undefined
          }
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
