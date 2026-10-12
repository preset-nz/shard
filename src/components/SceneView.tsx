import { Icons } from '@preset.nz/ux-kit';
import { type ReactNode, useRef, useState } from 'react';
import {
  audible,
  MAX_DRUMS_TRACKS,
  MAX_MELODY_TRACKS,
  type MixState,
  type PatchInfo,
  type Track,
  trackName,
} from '@/audio';
import { AddMenu } from '@/components/AddMenu';
import { MixSwitch } from '@/components/MixSwitch';
import { PlaysPicker } from '@/components/PlaysPicker';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';

/**
 * The scene view (epics 10 and 28): the left panel. It lists the session's
 * tracks, its patches and its materials, and what is selected here decides
 * what the middle shows: a track shows the tracker on its steps, a patch its
 * lanes and inspector. Every track plays whatever is selected; a patch no
 * track plays loops on its own over them while it is selected. Selecting is
 * session state, never an edit.
 *
 * The current patch carries its own generators and modulators, nested under
 * its row; the global LFOs (roadmap row 43), which any patch may follow, and
 * the materials are the session's, so they sit apart. Those trees are the
 * caller's, passed in as `patchChildren`, `globals` and `materials`.
 *
 * Keyboard: focus the panel (Tab, or click a row), then Up and Down move a
 * cursor through the tracks and the patches and Return selects the row under
 * it. Moving the cursor does not select, because selecting a patch changes
 * what you hear. Return on the patch already selected renames it, as Return
 * on a selected item does in the Finder, and so does a double-click. Delete
 * takes the track or patch under the cursor, when it may go. M and S mute
 * and solo the track under it. Right-click a patch for Rename, Duplicate and
 * Delete, a track for Mute, Solo and Remove.
 *
 * Each track has M and S switches (story c). A press lands on the next bar
 * while playing, and blinks until it does; a track not heard is dimmed. They
 * are session state, not edits.
 *
 * Tracks are of two kinds in one list (story d): a melody track with its
 * *Plays* picker, and a drums track, marked "kit", which plays no patch.
 * The + offers both, as Track › Add Track does.
 */
export function SceneView({
  tracks,
  patches,
  selectedTrack,
  selectedPatch,
  onSelectTrack,
  onSelectPatch,
  onPlays,
  onAddTrack,
  onRemoveTrack,
  mix,
  onMute,
  onSolo,
  onNewPatch,
  renaming,
  onRenaming,
  onRename,
  onDuplicate,
  onDelete,
  patchChildren,
  globals,
  materials,
}: {
  tracks: Track[];
  patches: PatchInfo[];
  /** The track that is the selection, by place, so the tracker shows. */
  selectedTrack: number | null;
  /** The patch that is the selection, if one is. */
  selectedPatch: number | null;
  onSelectTrack: (track: number) => void;
  onSelectPatch: (id: number) => void;
  /** Track `track` now plays this patch (its *Plays* picker). */
  onPlays: (track: number, id: number) => void;
  /** Add Track › Melody, or › Drums with `drums`. */
  onAddTrack: (drums: boolean) => void;
  /** Only offered for a drums track, or a melody track not the last. */
  onRemoveTrack: (track: number) => void;
  /** Every track's mute and solo, pressed and heard, in order. */
  mix: MixState[];
  /** Toggle track `track`'s mute or solo. */
  onMute: (track: number) => void;
  onSolo: (track: number) => void;
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
  /** The Global section: the global LFOs. */
  globals: ReactNode;
  /** The materials section. */
  materials: ReactNode;
}) {
  const navRef = useRef<HTMLElement>(null);
  const [focused, setFocused] = useState(false);
  // Where Up and Down have moved to, among the track and the patches. Null
  // means "on the selection".
  const [cursor, setCursor] = useState<number | null>(null);

  // Rows: the tracks, then the patches.
  const t = tracks.length;
  const rowCount = t + patches.length;
  const selectedRow =
    selectedTrack !== null
      ? Math.min(selectedTrack, t - 1)
      : Math.max(t, t + patches.findIndex((p) => p.id === selectedPatch));
  const here = Math.min(cursor ?? selectedRow, rowCount - 1);
  const melodyCount = tracks.filter((x) => x.drums == null).length;
  const drumsCount = tracks.length - melodyCount;
  const removable = (i: number) => tracks[i]?.drums != null || melodyCount > 1;

  const selectRow = (row: number) => {
    setCursor(null);
    if (row < t) onSelectTrack(row);
    else {
      const patch = patches[row - t];
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
        // M and S on a track row, only with the panel itself focused: never
        // from a field or a picker inside it.
        if ((e.code === 'KeyM' || e.code === 'KeyS') && e.target === navRef.current) {
          if (here >= t) return;
          e.preventDefault();
          e.stopPropagation();
          if (e.repeat) return;
          if (e.code === 'KeyM') onMute(here);
          else onSolo(here);
          return;
        }
        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
          e.preventDefault();
          setCursor(Math.max(0, Math.min(rowCount - 1, here + (e.key === 'ArrowDown' ? 1 : -1))));
        } else if (e.key === 'Enter') {
          e.preventDefault();
          const patch = here >= t ? patches[here - t] : undefined;
          // On the selection, Return renames; elsewhere it selects.
          if (patch && patch.id === selectedPatch && (cursor === null || cursor === selectedRow)) {
            setCursor(null);
            onRenaming(patch.id);
          } else {
            selectRow(here);
          }
        } else if (e.key === 'Backspace' || e.key === 'Delete') {
          // Ours, not the selected effect's: the window's handler would
          // take it out of its chain.
          e.preventDefault();
          e.stopPropagation();
          if (here < t) {
            if (removable(here)) {
              setCursor(null);
              onRemoveTrack(here);
            }
            return;
          }
          const patch = patches[here - t];
          if (patch?.deletable) {
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
          <AddMenu
            title="Add a track"
            choices={[
              {
                kind: 'melody',
                label: 'Melody ⌥⌘N',
                disabled:
                  melodyCount >= MAX_MELODY_TRACKS ? `${MAX_MELODY_TRACKS} at most` : undefined,
              },
              {
                kind: 'drums',
                label: 'Drums ⌥⌘U',
                disabled:
                  drumsCount >= MAX_DRUMS_TRACKS ? `${MAX_DRUMS_TRACKS} at most` : undefined,
              },
            ]}
            onAdd={(kind) => onAddTrack(kind === 'drums')}
          />
        </div>
        {/* A row is not one button: the name selects the track, and the
            picker beside it chooses the patch it plays. */}
        {tracks.map((track, i) => {
          const name = trackName(tracks, i);
          const drums = track.drums != null;
          return (
            <ContextMenu
              // biome-ignore lint/suspicious/noArrayIndexKey: a track is its place
              key={i}
            >
              <ContextMenuTrigger render={<div className={rowClass(i, selectedTrack === i)} />}>
                <button
                  type="button"
                  title={`Show this track's ${drums ? 'drums' : 'steps'}${
                    audible(mix, i) ? '' : '. Not heard: muted, or another track is soloed.'
                  }`}
                  onMouseDown={focusPanel}
                  onClick={() => selectRow(i)}
                  className={`min-w-0 flex-1 truncate text-left ${audible(mix, i) ? '' : 'opacity-50'}`}
                >
                  {name}
                </button>
                <MixSwitch
                  letter="M"
                  verb="Mute"
                  name={name}
                  on={mix[i]?.mute ?? false}
                  heard={mix[i]?.heard_mute ?? false}
                  chord="⌃M"
                  onMouseDown={focusPanel}
                  onToggle={() => onMute(i)}
                />
                <MixSwitch
                  letter="S"
                  verb="Solo"
                  name={name}
                  on={mix[i]?.solo ?? false}
                  heard={mix[i]?.heard_solo ?? false}
                  chord="⌃S"
                  onMouseDown={focusPanel}
                  onToggle={() => onSolo(i)}
                />
                {drums ? (
                  // No patch: the kit is its own sound. Where the picker would be.
                  <span
                    title="A drums track: a synthesised kick, snare and hat, no patch"
                    className="w-[4.75rem] truncate pl-1 text-[10px] text-muted-foreground"
                  >
                    kit
                  </span>
                ) : (
                  <PlaysPicker
                    patches={patches}
                    value={track.patch}
                    onPick={(id) => onPlays(i, id)}
                    className="w-[4.75rem]"
                  />
                )}
              </ContextMenuTrigger>
              <ContextMenuContent className="w-48" finalFocus={false}>
                <ContextMenuItem onClick={() => onMute(i)}>
                  {mix[i]?.mute ? 'Unmute' : 'Mute'} {name}
                </ContextMenuItem>
                <ContextMenuItem onClick={() => onSolo(i)}>
                  {mix[i]?.solo ? 'Unsolo' : 'Solo'} {name}
                </ContextMenuItem>
                <ContextMenuSeparator />
                <ContextMenuItem disabled={!removable(i)} onClick={() => onRemoveTrack(i)}>
                  Remove {name}
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
          );
        })}
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
                className={rowClass(t + i, p.id === selectedPatch, p.current)}
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
                        p.played
                          ? 'Shape this patch; the tracks keep playing. Double-click to rename.'
                          : 'Shape this patch; no track plays it, so it loops on its own while selected. Double-click to rename.'
                      }
                      onMouseDown={focusPanel}
                      onClick={() => selectRow(t + i)}
                      onDoubleClick={() => onRenaming(p.id)}
                      className={rowClass(t + i, p.id === selectedPatch, p.current)}
                    />
                  }
                >
                  <span className="flex-1 truncate">{p.name}</span>
                  {p.auditioned && (
                    <span
                      title="Looping on its own over the mix: no track plays it"
                      className="text-[10px] text-muted-foreground"
                    >
                      loop
                    </span>
                  )}
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

      {globals}
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
