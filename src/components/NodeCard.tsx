import { denormalise, format, normalise, type ParamInfo, setParam } from '@/audio';
import { NodePresets } from '@/components/NodePresets';
import { Slider } from '@/components/ui/slider';
import { levelIdOf, type NodeInfo, type ParamValues, switchIdOf } from '@/scope';

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

/**
 * One node in the work area: its title, its level, and its switch.
 *
 * Generated from the table: the level is the node's first row by the
 * parameter pattern (Gain for a generator, Mix for an effect), so a new node
 * needs no card of its own. Click the title to show the node in the
 * inspector; right-click the card for its presets. Solo arrives with roadmap
 * row 14.
 */
export function NodeCard({
  node,
  defs,
  values,
  selected,
  onSelect,
  onError,
  onNote,
  onPresetApplied,
}: {
  node: NodeInfo;
  defs: ParamInfo[];
  values: ParamValues;
  selected: boolean;
  onSelect: () => void;
  onError: (message: string | null) => void;
  onNote: (message: string | null) => void;
  onPresetApplied: () => void;
}) {
  const sw = switchIdOf(defs, node);
  const levelId = levelIdOf(defs, node);
  const level = levelId ? defs.find((d) => d.id === levelId) : undefined;
  const on = sw ? (values[sw] ?? 1) >= 0.5 : true;

  const card = (
    <div
      data-node-card
      className={`rounded-md border px-2.5 py-2 transition-colors ${
        selected ? 'border-primary bg-primary/5' : 'border-border'
      } ${on ? '' : 'opacity-60'}`}
    >
      <div className="flex items-center gap-2">
        <button
          type="button"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onSelect}
          className="min-w-0 flex-1 truncate text-left text-xs font-medium"
        >
          {node.label}
        </button>
        {sw && (
          <NodeSwitch
            label={node.label}
            on={on}
            onSwitch={(next) => void setParam(sw, next ? 1 : 0)}
          />
        )}
      </div>
      {level && (
        <div className="mt-2 flex items-center gap-2">
          <Slider
            min={0}
            max={1}
            step={0.001}
            value={[normalise(level, values[level.id] ?? level.default)]}
            onValueChange={([t]) => void setParam(level.id, denormalise(level, t))}
            className="flex-1"
            aria-label={`${node.label} ${level.name}`}
          />
          <span className="w-10 shrink-0 text-right font-mono text-[10px] text-muted-foreground tabular-nums">
            {format(level, values[level.id] ?? level.default)}
          </span>
        </div>
      )}
    </div>
  );

  return node.table ? (
    <NodePresets
      node={node.table}
      label={node.label}
      className=""
      onError={onError}
      onNote={onNote}
      onApplied={onPresetApplied}
    >
      {card}
    </NodePresets>
  ) : (
    card
  );
}
