import { Icons } from '@preset.nz/ux-kit';
import { type ReactNode, useRef, useState } from 'react';
import type { PatchInfo } from '@/audio';

/**
 * The scene view (epic 10): the left panel, in both what were the modes. It
 * lists the session's track, its patches and its materials, and what is
 * selected here decides what the middle shows and what plays: the track shows
 * the tracker and plays its steps, a patch shows its lanes and loops it.
 * Selecting is session state, never an edit.
 *
 * The current patch carries its own generators and modulators, nested under
 * its row; materials are the session's, so they sit apart. Those trees are
 * the caller's, passed in as `patchChildren` and `materials`.
 *
 * Keyboard: focus the panel (Tab, or click a row), then Up and Down move a
 * cursor through the track and the patches and Return selects the row under
 * it. Moving the cursor does not select, because selecting a patch changes
 * what you hear.
 */
export function SceneView({
  patches,
  trackSelected,
  selectedPatch,
  onSelectTrack,
  onSelectPatch,
  onNewPatch,
  patchChildren,
  materials,
}: {
  patches: PatchInfo[];
  /** The track is the selection, so the tracker shows. */
  trackSelected: boolean;
  /** The patch that is the selection, if one is. */
  selectedPatch: number | null;
  onSelectTrack: () => void;
  onSelectPatch: (id: number) => void;
  onNewPatch: () => void;
  /** The current patch's generators and modulators. */
  patchChildren: ReactNode;
  /** The materials section. */
  materials: ReactNode;
}) {
  const navRef = useRef<HTMLElement>(null);
  const [focused, setFocused] = useState(false);
  // Where Up and Down have moved to, among the track and the patches. Null
  // means "on the selection".
  const [cursor, setCursor] = useState<number | null>(null);

  const played = patches.find((p) => p.played);
  const rowCount = 1 + patches.length;
  const selectedRow = trackSelected
    ? 0
    : Math.max(0, patches.findIndex((p) => p.id === selectedPatch) + 1);
  const here = Math.min(cursor ?? selectedRow, rowCount - 1);

  const selectRow = (row: number) => {
    setCursor(null);
    if (row === 0) onSelectTrack();
    else {
      const patch = patches[row - 1];
      if (patch) onSelectPatch(patch.id);
    }
  };

  const rowClass = (row: number, selected: boolean, emphasised = false) =>
    `flex w-full items-baseline gap-1.5 px-3 py-1 text-left text-xs ${
      selected
        ? 'bg-accent text-foreground'
        : emphasised
          ? 'text-foreground hover:bg-accent/50'
          : 'text-muted-foreground hover:text-foreground'
    } ${focused && cursor !== null && cursor === row ? 'ring-1 ring-inset ring-ring' : ''}`;

  // Keep focus on the panel rather than the button, so Space still starts
  // playback; the panel is not a button, so the global keys pass through it.
  const focusPanel = (e: React.MouseEvent) => {
    e.preventDefault();
    navRef.current?.focus();
  };

  return (
    <nav
      ref={navRef}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: the panel takes focus so Up, Down and Return work
      tabIndex={0}
      aria-label="Scene"
      onFocus={() => setFocused(true)}
      onBlur={() => {
        setFocused(false);
        setCursor(null);
      }}
      onKeyDown={(e) => {
        if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
          e.preventDefault();
          setCursor(Math.max(0, Math.min(rowCount - 1, here + (e.key === 'ArrowDown' ? 1 : -1))));
        } else if (e.key === 'Enter') {
          e.preventDefault();
          selectRow(here);
        }
      }}
      className="w-48 shrink-0 overflow-y-auto border-r border-border outline-none"
    >
      <div className="py-2">
        <div className="flex items-center pr-2 pl-3 pb-1">
          <span className="flex-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Tracks
          </span>
        </div>
        <button
          type="button"
          title="Play this track's steps, through the arrangement"
          onMouseDown={focusPanel}
          onClick={() => selectRow(0)}
          className={rowClass(0, trackSelected)}
        >
          <span className="flex-1 truncate">Track 1</span>
          {played && (
            <span
              title={`Plays ${played.name}`}
              className="min-w-0 max-w-[55%] shrink-0 truncate text-[10px] text-muted-foreground"
            >
              {played.name}
            </span>
          )}
        </button>
      </div>

      <div className="py-2">
        <div className="flex items-center pr-2 pl-3 pb-1">
          <span className="flex-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Patches
          </span>
          <button
            type="button"
            title="New Patch (⇧⌘N)"
            // Keep focus off the button, so Space still starts playback.
            onMouseDown={(e) => e.preventDefault()}
            onClick={onNewPatch}
            className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <Icons.PlusIcon size={13} />
          </button>
        </div>
        {patches.map((p, i) => (
          <div key={p.id}>
            <button
              type="button"
              title={
                p.current ? 'Shape this patch and loop it' : 'Switch to this patch and loop it'
              }
              onMouseDown={focusPanel}
              onClick={() => selectRow(i + 1)}
              className={rowClass(i + 1, p.id === selectedPatch, p.current)}
            >
              <span className="flex-1 truncate">{p.name}</span>
            </button>
            {p.current && patchChildren}
          </div>
        ))}
      </div>

      {materials}
    </nav>
  );
}
