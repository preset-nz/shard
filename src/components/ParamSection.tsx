import { ChevronRight } from 'lucide-react';
import type { ReactNode } from 'react';

/**
 * One parameter section, with a header that folds it away and, for an effect,
 * switches it off.
 *
 * The header lives here rather than in facets because a bypass is Shard's
 * idea, not a property panel's. facets renders the rows; this owns the section.
 *
 * Option-click on the title folds or unfolds every section at once, the way a
 * disclosure triangle does in Finder.
 */
export function ParamSection({
  label,
  collapsed,
  onToggle,
  on,
  onSwitch,
  children,
}: {
  label: string;
  collapsed: boolean;
  /** `all` is true when every section should follow this one. */
  onToggle: (all: boolean) => void;
  /** Present only for sections the table gives a switch. */
  on?: boolean;
  onSwitch?: (on: boolean) => void;
  children: ReactNode;
}) {
  const isOn = on ?? true;

  return (
    <section className="border-b border-border last:border-b-0">
      <div className="flex items-center pr-3">
        <button
          type="button"
          aria-expanded={!collapsed}
          title="Option-click to fold or unfold every section"
          // A click must not leave focus on the header, or the next Space
          // presses it again instead of starting playback. Tab still reaches it.
          onMouseDown={(e) => e.preventDefault()}
          onClick={(e) => onToggle(e.altKey)}
          className="flex min-w-0 flex-1 items-center gap-1.5 px-3 py-2 text-left text-[11px] font-semibold uppercase tracking-wider text-muted-foreground hover:text-foreground"
        >
          <ChevronRight
            size={12}
            className={`shrink-0 transition-transform ${collapsed ? '' : 'rotate-90'}`}
          />
          <span className="truncate">{label}</span>
        </button>
        {onSwitch && (
          <button
            type="button"
            role="switch"
            aria-checked={isOn}
            title={`${label} is ${isOn ? 'on' : 'off'}. Switching it keeps its settings.`}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => onSwitch(!isOn)}
            className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wider transition-colors ${
              isOn
                ? 'bg-primary/15 text-primary'
                : 'border border-border text-muted-foreground hover:text-foreground'
            }`}
          >
            {isOn ? 'On' : 'Off'}
          </button>
        )}
      </div>
      {/* Dimmed when off, but still editable: set it up, then switch it in. */}
      {!collapsed && <div className={isOn ? undefined : 'opacity-50'}>{children}</div>}
    </section>
  );
}
