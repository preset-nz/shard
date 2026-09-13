import { type ReactNode, useState } from 'react';
import { applyPreset, presetNames, savePreset, updatePreset } from '@/audio';
import { Button } from '@/components/ui/button';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';

/**
 * A node's presets, on a right-click anywhere in its section.
 *
 * Presets live in the patch, so saving one is not the end of it: the patch
 * has to be saved too, and every message here says so. A preset holds the
 * section's values but never its on/off switch.
 *
 * The names are fetched each time the menu opens rather than held here, so a
 * patch loaded since the last right-click can never show stale presets.
 */
export function NodePresets({
  node,
  label,
  onError,
  onNote,
  onApplied,
  children,
}: {
  /** The section id, such as `grain`. */
  node: string;
  label: string;
  onError: (message: string | null) => void;
  onNote: (message: string | null) => void;
  /** After a preset applies, since it replaces the section's links too. */
  onApplied?: () => void;
  children: ReactNode;
}) {
  const [names, setNames] = useState<string[]>([]);
  const [naming, setNaming] = useState(false);
  const [name, setName] = useState('');
  const [problem, setProblem] = useState<string | null>(null);

  const refresh = async () => {
    try {
      setNames(await presetNames(node));
    } catch (e) {
      onError(String(e));
    }
  };

  const apply = async (preset: string) => {
    try {
      const report = await applyPreset(node, preset);
      onApplied?.();
      onError(null);
      const parts: string[] = [];
      if (report.unknown.length > 0) {
        parts.push(`${report.unknown.length} of its values no longer exist and were skipped`);
      }
      if (report.refused.length > 0) {
        parts.push(`${report.refused.length} of its links name an LFO this patch cannot use`);
      }
      onNote(parts.length > 0 ? `Applied “${preset}”, but ${parts.join(', and ')}.` : null);
    } catch (e) {
      onError(String(e));
    }
  };

  const update = async (preset: string) => {
    try {
      await updatePreset(node, preset);
      onError(null);
      onNote(`Updated ${label} preset “${preset}”. Save the patch to keep it.`);
    } catch (e) {
      onError(String(e));
    }
  };

  const save = async () => {
    try {
      await savePreset(node, name);
      setNaming(false);
      onError(null);
      onNote(`Saved ${label} preset “${name.trim()}”. Save the patch to keep it.`);
    } catch (e) {
      // Shown in the dialog, beside the name that caused it.
      setProblem(String(e));
    }
  };

  return (
    <>
      {/* Not modal, so choosing "Save as preset…" can hand focus straight to
          the dialog instead of fighting the menu for it as it closes. */}
      <ContextMenu modal={false} onOpenChange={(open) => open && void refresh()}>
        <ContextMenuTrigger asChild>
          <div className="border-b border-border last:border-b-0">{children}</div>
        </ContextMenuTrigger>
        <ContextMenuContent className="w-56">
          <ContextMenuLabel>{label} presets</ContextMenuLabel>
          <ContextMenuSeparator />
          <ContextMenuSub>
            <ContextMenuSubTrigger disabled={names.length === 0}>
              Apply preset
            </ContextMenuSubTrigger>
            <ContextMenuSubContent>
              {names.map((n) => (
                <ContextMenuItem key={n} onSelect={() => void apply(n)}>
                  {n}
                </ContextMenuItem>
              ))}
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuItem
            onSelect={() => {
              setName('');
              setProblem(null);
              setNaming(true);
            }}
          >
            Save as preset…
          </ContextMenuItem>
          <ContextMenuSub>
            <ContextMenuSubTrigger disabled={names.length === 0}>
              Update preset
            </ContextMenuSubTrigger>
            <ContextMenuSubContent>
              {names.map((n) => (
                <ContextMenuItem key={n} onSelect={() => void update(n)}>
                  {n}
                </ContextMenuItem>
              ))}
            </ContextMenuSubContent>
          </ContextMenuSub>
        </ContextMenuContent>
      </ContextMenu>

      <Dialog open={naming} onOpenChange={setNaming}>
        <DialogContent className="sm:max-w-sm">
          <form
            className="grid gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <DialogHeader>
              <DialogTitle>Save {label} preset</DialogTitle>
              <DialogDescription>
                Stores this section's current settings under a name, in this patch. Its on/off
                switch is not part of it. Save the patch to keep the preset.
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-1.5">
              <Input
                aria-label="Preset name"
                placeholder="Name"
                value={name}
                onChange={(e) => {
                  setName(e.target.value);
                  setProblem(null);
                }}
              />
              {problem && <p className="text-xs text-destructive">{problem}</p>}
            </div>
            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => setNaming(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={name.trim() === ''}>
                Save
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
