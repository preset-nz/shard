import { ChevronRight } from 'lucide-react';
import type { ReactNode } from 'react';

/**
 * One parameter section, with a header that folds it away.
 *
 * The header lives here rather than in facets because a Shard section is
 * about to carry more than a title — on/off is next on the roadmap — and
 * facets should never learn what a bypass is. facets renders the rows; this
 * owns the section.
 *
 * Option-click folds or unfolds every section at once, the way a disclosure
 * triangle does in Finder.
 */
export function ParamSection({
  label,
  collapsed,
  onToggle,
  children,
}: {
  label: string;
  collapsed: boolean;
  /** `all` is true when every section should follow this one. */
  onToggle: (all: boolean) => void;
  children: ReactNode;
}) {
  return (
    <section className="border-b border-border last:border-b-0">
      <button
        type="button"
        aria-expanded={!collapsed}
        title="Option-click to fold or unfold every section"
        // A click must not leave focus on the header, or the next Space
        // presses it again instead of starting playback. Tab still reaches it.
        onMouseDown={(e) => e.preventDefault()}
        onClick={(e) => onToggle(e.altKey)}
        className="flex w-full items-center gap-1.5 px-3 py-2 text-left text-[11px] font-semibold uppercase tracking-wider text-muted-foreground hover:text-foreground"
      >
        <ChevronRight
          size={12}
          className={`shrink-0 transition-transform ${collapsed ? '' : 'rotate-90'}`}
        />
        {label}
      </button>
      {!collapsed && children}
    </section>
  );
}
