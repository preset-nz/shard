import { Icons } from '@preset.nz/ux-kit';
import { type ReactNode, useRef, useState } from 'react';
import type { PatchInfo } from '@/audio';
import { PlaysPicker } from '@/components/PlaysPicker';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';

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
 * what you hear. Return on the patch already selected renames it, as Return
 * on a selected item does in the Finder, and so does a double-click. Delete
 * takes the patch under the cursor, when it may go. Right-click a patch for
 * Rename, Duplicate and Delete.
 */
export function SceneView({
  patches,
  trackSelected,
  selectedPatch,
  onSelectTrack,
  onSelectPatch,
  onPlays,
  onNewPatch,
  renaming,
  onRenaming,
  onRename,
  onDuplicate,
  onDelete,
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
  /** The track now plays this patch (the *Plays* picker). */
  onPlays: (id: number) => void;
  onNewPatch: () => void;
  /** The patch whose name is being edited in place, by id. */
  renaming: number | null;
  /** Opens the name field on a patch, or closes it with null. */
  onRenaming: (id: number | null) => void;
  /** A patch's new name, as typed. Rust trims it and ignores an empty one. */
  onRename: (id: number, name: string) => void;
  onDuplicate: (id: number) => void;
  /** Only offered for a patch whose `deletable` is set. */
  onDelete: (id: number) => void;
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
    `flex w-full items-center gap-1.5 px-3 py-1 text-left text-xs ${
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
          const patch = here > 0 ? patches[here - 1] : undefined;
          // On the selection, Return renames; elsewhere it selects.
          if (patch && patch.id === selectedPatch && (cursor === null || cursor === selectedRow)) {
            setCursor(null);
            onRenaming(patch.id);
          } else {
            selectRow(here);
          }
        } else if (e.key === 'Backspace' || e.key === 'Delete') {
          const patch = here > 0 ? patches[here - 1] : undefined;
          if (!patch) return;
          // Ours, not the selected effect's: the window's handler would
          // take it out of its chain.
          e.preventDefault();
          e.stopPropagation();
          if (patch.deletable) {
            setCursor(null);
            onDelete(patch.id);
          }
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
        {/* The row is not one button: the name selects the track, and the
            picker beside it chooses the patch it plays. */}
        <div className={rowClass(0, trackSelected)}>
          <button
            type="button"
            title="Play this track's steps, through the arrangement"
            onMouseDown={focusPanel}
            onClick={() => selectRow(0)}
            className="min-w-0 flex-1 truncate text-left"
          >
            Track 1
          </button>
          <PlaysPicker patches={patches} onPick={onPlays} className="w-[5.5rem]" />
        </div>
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
            {renaming === p.id ? (
              <RenameField
                name={p.name}
                className={rowClass(i + 1, p.id === selectedPatch, p.current)}
                onDone={(name) => {
                  onRenaming(null);
                  navRef.current?.focus();
                  if (name !== null) onRename(p.id, name);
                }}
              />
            ) : (
              <ContextMenu>
                <ContextMenuTrigger
                  render={
                    <button
                      type="button"
                      title={
                        p.current
                          ? 'Shape this patch and loop it. Double-click to rename.'
                          : 'Switch to this patch and loop it. Double-click to rename.'
                      }
                      onMouseDown={focusPanel}
                      onClick={() => selectRow(i + 1)}
                      onDoubleClick={() => onRenaming(p.id)}
                      className={rowClass(i + 1, p.id === selectedPatch, p.current)}
                    />
                  }
                >
                  <span className="flex-1 truncate">{p.name}</span>
                </ContextMenuTrigger>
                {/* Closing leaves focus where it is, so Rename's field keeps it. */}
                <ContextMenuContent className="w-48" finalFocus={false}>
                  <ContextMenuItem onClick={() => onRenaming(p.id)}>Rename</ContextMenuItem>
                  <ContextMenuItem onClick={() => onDuplicate(p.id)}>
                    Duplicate {p.name}
                  </ContextMenuItem>
                  <ContextMenuSeparator />
                  <ContextMenuItem disabled={!p.deletable} onClick={() => onDelete(p.id)}>
                    Delete {p.name}
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenu>
            )}
            {p.current && patchChildren}
          </div>
        ))}
      </div>

      {materials}
    </nav>
  );
}

/**
 * A patch's name, being edited in place. Return or leaving the field keeps
 * it; Escape puts it back. Keys stay in the field, so Return doesn't select
 * a row and Delete doesn't delete one.
 */
function RenameField({
  name,
  className,
  onDone,
}: {
  name: string;
  className: string;
  /** The name typed, or null when cancelled. Called once. */
  onDone: (name: string | null) => void;
}) {
  const [draft, setDraft] = useState(name);
  const done = useRef(false);
  const finish = (value: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(value);
  };
  return (
    <div className={className}>
      <input
        aria-label="Patch name"
        // biome-ignore lint/a11y/noAutofocus: the field opens because the user asked to rename
        autoFocus
        onFocus={(e) => e.currentTarget.select()}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter') {
            e.preventDefault();
            finish(draft);
          } else if (e.key === 'Escape') {
            e.preventDefault();
            finish(null);
          }
        }}
        onBlur={() => finish(draft)}
        className="min-w-0 flex-1 rounded-sm border border-ring bg-background px-1 text-xs text-foreground outline-none"
      />
    </div>
  );
}
